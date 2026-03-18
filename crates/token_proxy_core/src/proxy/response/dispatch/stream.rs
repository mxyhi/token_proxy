use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

use super::super::super::{
    antigravity_compat, codex_compat, gemini_compat, http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai_compat::FormatTransform,
    redact::redact_query_param_value,
    server_helpers::log_debug_headers_body,
    token_rate::RequestTokenTracker,
};
use super::super::{
    anthropic_to_responses, chat_to_responses, kiro_to_anthropic, responses_to_anthropic,
    responses_to_chat, streaming, upstream_stream, PROVIDER_ANTIGRAVITY, PROVIDER_CODEX,
    PROVIDER_GEMINI, PROVIDER_OPENAI, PROVIDER_OPENAI_RESPONSES,
};

type UpstreamBytesStream = futures_util::stream::BoxStream<
    'static,
    Result<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>,
>;
type ResponseStream = futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>;
const DEBUG_BODY_LOG_LIMIT_BYTES: usize = usize::MAX;

pub(super) async fn build_stream_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    upstream_no_data_timeout: Duration,
) -> Response {
    let mut context = context;
    let upstream = match prepare_upstream_stream(
        status,
        &headers,
        upstream_res,
        &mut context,
        &log,
        upstream_no_data_timeout,
    )
    .await
    {
        Ok(stream) => stream,
        Err(response) => return response,
    };
    log_debug_headers_body(
        "upstream.response.headers",
        Some(&headers),
        None,
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    let upstream = if context.provider == PROVIDER_ANTIGRAVITY {
        antigravity_compat::stream_antigravity_to_gemini(upstream).boxed()
    } else {
        upstream
    };
    let upstream = log_upstream_stream_if_debug(upstream);

    let stream = stream_for_transform(
        response_transform,
        upstream,
        context,
        log,
        request_tracker,
        estimated_input_tokens,
        model_override,
    );
    log_debug_headers_body(
        "outbound.response.headers",
        Some(&headers),
        None,
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    let stream = log_response_stream_if_debug(stream);
    let body = Body::from_stream(stream);
    http::build_response(status, headers, body)
}

fn stream_for_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
    model_override: Option<&str>,
) -> ResponseStream {
    if is_simple_transform(transform) {
        return stream_for_simple_transform(
            transform,
            upstream,
            context,
            log,
            request_tracker,
            model_override,
            estimated_input_tokens,
        );
    }
    stream_for_composed_transform(transform, upstream, context, log, request_tracker)
}

fn is_simple_transform(transform: FormatTransform) -> bool {
    matches!(
        transform,
        FormatTransform::None
            | FormatTransform::ResponsesToChat
            | FormatTransform::ChatToResponses
            | FormatTransform::ResponsesToAnthropic
            | FormatTransform::AnthropicToResponses
            | FormatTransform::GeminiToChat
            | FormatTransform::ChatToGemini
            | FormatTransform::KiroToAnthropic
            | FormatTransform::CodexToChat
            | FormatTransform::CodexToResponses
            | FormatTransform::ChatToCodex
            | FormatTransform::ResponsesToCodex
    )
}

fn stream_for_simple_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> ResponseStream {
    match transform {
        FormatTransform::None
        | FormatTransform::ResponsesToChat
        | FormatTransform::ChatToResponses
        | FormatTransform::ResponsesToAnthropic
        | FormatTransform::AnthropicToResponses => stream_for_basic_transform(
            transform,
            upstream,
            context,
            log,
            request_tracker,
            model_override,
        ),
        _ => stream_for_simple_extended(
            transform,
            upstream,
            context,
            log,
            request_tracker,
            estimated_input_tokens,
        ),
    }
}

fn stream_for_basic_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    model_override: Option<&str>,
) -> ResponseStream {
    match transform {
        FormatTransform::None => stream_with_optional_model_override(
            upstream,
            context,
            log,
            request_tracker,
            model_override,
        ),
        FormatTransform::ResponsesToChat => {
            responses_to_chat::stream_responses_to_chat(upstream, context, log, request_tracker)
                .boxed()
        }
        FormatTransform::ChatToResponses => {
            chat_to_responses::stream_chat_to_responses(upstream, context, log, request_tracker)
                .boxed()
        }
        FormatTransform::ResponsesToAnthropic => {
            responses_to_anthropic::stream_responses_to_anthropic(
                upstream,
                context,
                log,
                request_tracker,
            )
            .boxed()
        }
        FormatTransform::AnthropicToResponses => {
            anthropic_to_responses::stream_anthropic_to_responses(
                upstream,
                context,
                log,
                request_tracker,
            )
            .boxed()
        }
        _ => streaming::stream_with_logging(upstream, context, log, request_tracker).boxed(),
    }
}

fn stream_for_simple_extended(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
) -> ResponseStream {
    match transform {
        FormatTransform::GeminiToChat => {
            gemini_compat::stream_gemini_to_chat(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::ChatToGemini => {
            gemini_compat::stream_chat_to_gemini(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::KiroToAnthropic => kiro_to_anthropic::stream_kiro_to_anthropic(
            upstream,
            context,
            log,
            request_tracker,
            estimated_input_tokens,
        )
        .boxed(),
        FormatTransform::CodexToChat => {
            codex_compat::stream_codex_to_chat(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::CodexToResponses => {
            codex_compat::stream_codex_to_responses(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::ChatToCodex | FormatTransform::ResponsesToCodex => {
            streaming::stream_with_logging(upstream, context, log, request_tracker).boxed()
        }
        _ => streaming::stream_with_logging(upstream, context, log, request_tracker).boxed(),
    }
}

fn stream_for_composed_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    match transform {
        FormatTransform::ChatToAnthropic => {
            stream_chat_to_anthropic(upstream, context, log, request_tracker)
        }
        FormatTransform::AnthropicToChat => {
            stream_anthropic_to_chat(upstream, context, log, request_tracker)
        }
        FormatTransform::GeminiToAnthropic => {
            stream_gemini_to_anthropic(upstream, context, log, request_tracker)
        }
        FormatTransform::AnthropicToGemini => {
            stream_anthropic_to_gemini(upstream, context, log, request_tracker)
        }
        FormatTransform::ResponsesToGemini => {
            stream_responses_to_gemini(upstream, context, log, request_tracker)
        }
        FormatTransform::GeminiToResponses => {
            stream_gemini_to_responses(upstream, context, log, request_tracker)
        }
        FormatTransform::CodexToAnthropic => {
            stream_codex_to_anthropic(upstream, context, log, request_tracker)
        }
        _ => streaming::stream_with_logging(upstream, context, log, request_tracker).boxed(),
    }
}

fn stream_chat_to_anthropic(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let responses_stream = chat_to_responses::stream_chat_to_responses(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    responses_to_anthropic::stream_responses_to_anthropic(
        responses_stream,
        context,
        log,
        request_tracker,
    )
    .boxed()
}

fn stream_anthropic_to_chat(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let responses_stream = anthropic_to_responses::stream_anthropic_to_responses(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    responses_to_chat::stream_responses_to_chat(responses_stream, context, log, request_tracker)
        .boxed()
}

fn stream_codex_to_anthropic(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let responses_stream = codex_compat::stream_codex_to_responses(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    responses_to_anthropic::stream_responses_to_anthropic(
        responses_stream,
        context,
        log,
        request_tracker,
    )
    .boxed()
}

fn stream_gemini_to_anthropic(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let first_log = Arc::new(LogWriter::new(None));
    let first_tracker = RequestTokenTracker::disabled();
    let chat_stream =
        gemini_compat::stream_gemini_to_chat(upstream, context.clone(), first_log, first_tracker)
            .boxed();
    let second_log = Arc::new(LogWriter::new(None));
    let second_tracker = RequestTokenTracker::disabled();
    let responses_stream = chat_to_responses::stream_chat_to_responses(
        chat_stream,
        context.clone(),
        second_log,
        second_tracker,
    )
    .boxed();
    responses_to_anthropic::stream_responses_to_anthropic(
        responses_stream,
        context,
        log,
        request_tracker,
    )
    .boxed()
}

fn stream_anthropic_to_gemini(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let first_log = Arc::new(LogWriter::new(None));
    let first_tracker = RequestTokenTracker::disabled();
    let responses_stream = anthropic_to_responses::stream_anthropic_to_responses(
        upstream,
        context.clone(),
        first_log,
        first_tracker,
    )
    .boxed();
    let second_log = Arc::new(LogWriter::new(None));
    let second_tracker = RequestTokenTracker::disabled();
    let chat_stream = responses_to_chat::stream_responses_to_chat(
        responses_stream,
        context.clone(),
        second_log,
        second_tracker,
    )
    .boxed();
    gemini_compat::stream_chat_to_gemini(chat_stream, context, log, request_tracker).boxed()
}

fn stream_responses_to_gemini(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let chat_stream = responses_to_chat::stream_responses_to_chat(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    gemini_compat::stream_chat_to_gemini(chat_stream, context, log, request_tracker).boxed()
}

fn stream_gemini_to_responses(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let chat_stream = gemini_compat::stream_gemini_to_chat(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    chat_to_responses::stream_chat_to_responses(chat_stream, context, log, request_tracker).boxed()
}

async fn prepare_upstream_stream(
    status: StatusCode,
    headers: &HeaderMap,
    upstream_res: reqwest::Response,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    upstream_no_data_timeout: Duration,
) -> Result<
    futures_util::stream::BoxStream<
        'static,
        Result<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>,
    >,
    Response,
> {
    let mut upstream =
        upstream_stream::with_idle_timeout(upstream_res.bytes_stream(), upstream_no_data_timeout);
    let first = upstream.next().await;
    match first {
        Some(Ok(chunk)) => Ok(chain_first_chunk(chunk, upstream, context)),
        Some(Err(err)) => Err(stream_error_response(
            err,
            context,
            log,
            upstream_no_data_timeout,
        )),
        None => Err(http::build_response(status, headers.clone(), Body::empty())),
    }
}

fn chain_first_chunk(
    chunk: Bytes,
    upstream: UpstreamBytesStream,
    context: &mut LogContext,
) -> UpstreamBytesStream {
    if context.ttfb_ms.is_none() {
        context.ttfb_ms = Some(context.start.elapsed().as_millis());
    }
    futures_util::stream::iter(vec![Ok::<
        Bytes,
        upstream_stream::UpstreamStreamError<reqwest::Error>,
    >(chunk)])
    .chain(upstream)
    .boxed()
}

fn stream_error_response(
    err: upstream_stream::UpstreamStreamError<reqwest::Error>,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    upstream_no_data_timeout: Duration,
) -> Response {
    let (status, message) = match err {
        upstream_stream::UpstreamStreamError::IdleTimeout(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            format!(
                "Upstream response timed out after {}s.",
                upstream_no_data_timeout.as_secs()
            ),
        ),
        upstream_stream::UpstreamStreamError::Upstream(err) => {
            let raw = err.to_string();
            let message = if context.provider == PROVIDER_GEMINI {
                redact_query_param_value(&raw, "key")
            } else {
                raw
            };
            (
                StatusCode::BAD_GATEWAY,
                format!("Failed to read upstream response: {message}"),
            )
        }
    };

    context.status = status.as_u16();
    let empty_usage = UsageSnapshot {
        usage: None,
        cached_tokens: None,
        usage_json: None,
    };
    let entry = build_log_entry(context, empty_usage, Some(message.clone()));
    log.clone().write_detached(entry);
    http::error_response(status, message)
}

fn log_upstream_stream_if_debug(upstream: UpstreamBytesStream) -> UpstreamBytesStream {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return upstream;
    }
    upstream
        .map(|item| {
            if let Ok(chunk) = &item {
                let text = String::from_utf8_lossy(chunk);
                tracing::debug!(
                    stage = "upstream.response.chunk",
                    bytes = chunk.len(),
                    body = %text,
                    "debug dump"
                );
            } else if let Err(err) = &item {
                tracing::debug!(stage = "upstream.response.chunk.error", error = %err, "debug dump");
            }
            item
        })
        .boxed()
}

fn log_response_stream_if_debug(stream: ResponseStream) -> ResponseStream {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return stream;
    }
    stream
        .map(|item| {
            if let Ok(chunk) = &item {
                let text = String::from_utf8_lossy(chunk);
                tracing::debug!(
                    stage = "outbound.response.chunk",
                    bytes = chunk.len(),
                    body = %text,
                    "debug dump"
                );
            } else if let Err(err) = &item {
                tracing::debug!(stage = "outbound.response.chunk.error", error = %err, "debug dump");
            }
            item
        })
        .boxed()
}

fn stream_with_optional_model_override<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    model_override: Option<&str>,
) -> futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    if let Some(model_override) = model_override {
        if should_rewrite_sse_model(&context.provider) {
            return streaming::stream_with_logging_and_model_override(
                upstream,
                context,
                log,
                model_override.to_string(),
                request_tracker,
            )
            .boxed();
        }
    }
    streaming::stream_with_logging(upstream, context, log, request_tracker).boxed()
}

// 只对 data-only SSE 的提供商做行级重写，避免破坏带 event: 行的流。
fn should_rewrite_sse_model(provider: &str) -> bool {
    provider == PROVIDER_OPENAI
        || provider == PROVIDER_OPENAI_RESPONSES
        || provider == PROVIDER_GEMINI
        || provider == PROVIDER_ANTIGRAVITY
        || provider == PROVIDER_CODEX
}
