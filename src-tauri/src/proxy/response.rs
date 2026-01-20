use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use super::{redact::redact_query_param_value, UPSTREAM_NO_DATA_TIMEOUT};
use super::{
    gemini_compat,
    http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    model,
    openai_compat::{transform_response_body, FormatTransform},
    token_rate::{RequestTokenTracker, TokenRateTracker},
    usage::extract_usage_from_response,
    request_detail::RequestDetailSnapshot,
    RequestMeta,
};

const PROVIDER_OPENAI: &str = "openai";
const PROVIDER_OPENAI_RESPONSES: &str = "openai-response";
const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const RESPONSE_ERROR_LIMIT_BYTES: usize = 256 * 1024;

pub(super) async fn build_proxy_response(
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    inbound_path: &str,
    upstream_res: reqwest::Response,
    log: Arc<LogWriter>,
    token_rate: Arc<TokenRateTracker>,
    start: Instant,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> Response {
    let status = upstream_res.status();
    let mut response_headers = http::filter_response_headers(upstream_res.headers());
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers, detail.request_body))
        .unwrap_or((None, None));
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: http::extract_request_id(upstream_res.headers()),
        request_headers,
        request_body,
        ttfb_ms: None,
        start,
    };
    let model_override = meta.model_override();
    if response_transform != FormatTransform::None {
        // The body will change; let hyper recalculate the content length.
        response_headers.remove(axum::http::header::CONTENT_LENGTH);
    }
    let model_for_tokens = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref())
        .map(|value| value.to_string());
    let request_tracker = token_rate
        .register(model_for_tokens, meta.estimated_input_tokens)
        .await;
    let should_stream = meta.stream && !status.is_client_error() && !status.is_server_error();
    if should_stream {
        build_stream_response(
            status,
            upstream_res,
            response_headers,
            context,
            log,
            request_tracker,
            response_transform,
            model_override,
            meta.estimated_input_tokens,
        )
        .await
    } else {
        build_buffered_response(
            status,
            upstream_res,
            response_headers,
            context,
            log,
            request_tracker,
            response_transform,
            model_override,
            meta.estimated_input_tokens,
        )
        .await
    }
}

pub(super) async fn build_proxy_response_buffered(
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    inbound_path: &str,
    upstream_res: reqwest::Response,
    log: Arc<LogWriter>,
    token_rate: Arc<TokenRateTracker>,
    start: Instant,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> Response {
    let status = upstream_res.status();
    let mut response_headers = http::filter_response_headers(upstream_res.headers());
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers, detail.request_body))
        .unwrap_or((None, None));
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: http::extract_request_id(upstream_res.headers()),
        request_headers,
        request_body,
        ttfb_ms: None,
        start,
    };
    let model_override = meta.model_override();
    if response_transform != FormatTransform::None {
        response_headers.remove(axum::http::header::CONTENT_LENGTH);
    }
    let model_for_tokens = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref())
        .map(|value| value.to_string());
    let request_tracker = token_rate
        .register(model_for_tokens, meta.estimated_input_tokens)
        .await;
    build_buffered_response(
        status,
        upstream_res,
        response_headers,
        context,
        log,
        request_tracker,
        response_transform,
        model_override,
        meta.estimated_input_tokens,
    )
    .await
}

async fn build_stream_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> Response {
    let mut context = context;
    let mut upstream = upstream_stream::with_idle_timeout(upstream_res.bytes_stream());
    let first = upstream.next().await;

    let upstream = match first {
        Some(Ok(chunk)) => {
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
        Some(Err(err)) => {
            let (status, message) = match err {
                upstream_stream::UpstreamStreamError::IdleTimeout(_) => (
                    StatusCode::GATEWAY_TIMEOUT,
                    format!(
                        "Upstream response timed out after {}s.",
                        UPSTREAM_NO_DATA_TIMEOUT.as_secs()
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
            let entry = build_log_entry(&context, empty_usage, Some(message.clone()));
            log.clone().write_detached(entry);
            return http::error_response(status, message);
        }
        None => {
            // 上游无 body：避免在下游转换链路里“空跑”并丢日志，这里直接返回空体。
            // 这个分支理论上很少出现（SSE/streaming 通常至少会输出一段数据）。
            return http::build_response(status, headers, Body::empty());
        }
    };

    let stream = match response_transform {
        FormatTransform::None => {
            if let Some(model_override) = model_override {
                if should_rewrite_sse_model(&context.provider) {
                    streaming::stream_with_logging_and_model_override(
                        upstream,
                        context,
                        log,
                        model_override.to_string(),
                        request_tracker,
                    )
                    .boxed()
                } else {
                    streaming::stream_with_logging(
                        upstream,
                        context,
                        log,
                        request_tracker,
                    )
                        .boxed()
                }
            } else {
                streaming::stream_with_logging(
                    upstream,
                    context,
                    log,
                    request_tracker,
                )
                    .boxed()
            }
        }
        FormatTransform::ResponsesToChat => {
            responses_to_chat::stream_responses_to_chat(
                upstream,
                context,
                log,
                request_tracker,
            )
                .boxed()
        }
        FormatTransform::ChatToResponses => {
            stream_chat_to_responses(
                upstream,
                context,
                log,
                request_tracker,
            )
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
        FormatTransform::ChatToAnthropic => {
            // Two-stage conversion: OpenAI Chat stream -> OpenAI Responses stream -> Claude stream.
            // Only the final stage writes logs / token usage to avoid duplication.
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
        FormatTransform::AnthropicToChat => {
            // Two-stage conversion: Claude stream -> OpenAI Responses stream -> OpenAI Chat stream.
            // Only the final stage writes logs / token usage to avoid duplication.
            let intermediate_log = Arc::new(LogWriter::new(None));
            let intermediate_tracker = RequestTokenTracker::disabled();
            let responses_stream = anthropic_to_responses::stream_anthropic_to_responses(
                upstream,
                context.clone(),
                intermediate_log,
                intermediate_tracker,
            )
            .boxed();
            responses_to_chat::stream_responses_to_chat(
                responses_stream,
                context,
                log,
                request_tracker,
            )
                .boxed()
        }
        FormatTransform::GeminiToAnthropic => {
            // Three-stage conversion: Gemini stream -> OpenAI Chat stream -> OpenAI Responses stream -> Claude stream.
            // Only the final stage writes logs / token usage to avoid duplication.
            let first_log = Arc::new(LogWriter::new(None));
            let first_tracker = RequestTokenTracker::disabled();
            let chat_stream = gemini_compat::stream_gemini_to_chat(
                upstream,
                context.clone(),
                first_log,
                first_tracker,
            )
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
        FormatTransform::AnthropicToGemini => {
            // Three-stage conversion: Claude stream -> OpenAI Responses stream -> OpenAI Chat stream -> Gemini stream.
            // Only the final stage writes logs / token usage to avoid duplication.
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
        FormatTransform::GeminiToChat => {
            gemini_compat::stream_gemini_to_chat(
                upstream,
                context,
                log,
                request_tracker,
            )
                .boxed()
        }
        FormatTransform::ChatToGemini => {
            gemini_compat::stream_chat_to_gemini(
                upstream,
                context,
                log,
                request_tracker,
            )
                .boxed()
        }
        FormatTransform::ResponsesToGemini => {
            // Two-stage conversion: Responses stream -> Chat stream -> Gemini stream.
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
        FormatTransform::GeminiToResponses => {
            // Two-stage conversion: Gemini stream -> Chat stream -> Responses stream.
            let intermediate_log = Arc::new(LogWriter::new(None));
            let intermediate_tracker = RequestTokenTracker::disabled();
            let chat_stream = gemini_compat::stream_gemini_to_chat(
                upstream,
                context.clone(),
                intermediate_log,
                intermediate_tracker,
            )
            .boxed();
            chat_to_responses::stream_chat_to_responses(chat_stream, context, log, request_tracker)
                .boxed()
        }
        FormatTransform::KiroToResponses => {
            kiro_to_responses::stream_kiro_to_responses(
                upstream,
                context,
                log,
                request_tracker,
                estimated_input_tokens,
            )
            .boxed()
        }
        FormatTransform::KiroToChat => {
            let intermediate_log = Arc::new(LogWriter::new(None));
            let intermediate_tracker = RequestTokenTracker::disabled();
            let responses_stream = kiro_to_responses::stream_kiro_to_responses(
                upstream,
                context.clone(),
                intermediate_log,
                intermediate_tracker,
                estimated_input_tokens,
            )
            .boxed();
            responses_to_chat::stream_responses_to_chat(
                responses_stream,
                context,
                log,
                request_tracker,
            )
            .boxed()
        }
        FormatTransform::KiroToAnthropic => {
            kiro_to_anthropic::stream_kiro_to_anthropic(
                upstream,
                context,
                log,
                request_tracker,
                estimated_input_tokens,
            )
            .boxed()
        }
    };
    let body = Body::from_stream(stream);
    http::build_response(status, headers, body)
}

async fn build_buffered_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> Response {
    let mut context = context;
    let bytes = match upstream_read::read_upstream_bytes_with_ttfb(upstream_res, &mut context).await {
        Ok(bytes) => bytes,
        Err(err) => {
            let (status, message) = match err {
                upstream_stream::UpstreamStreamError::IdleTimeout(_) => (
                    StatusCode::GATEWAY_TIMEOUT,
                    format!(
                        "Upstream response timed out after {}s.",
                        UPSTREAM_NO_DATA_TIMEOUT.as_secs()
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
            let entry = build_log_entry(&context, empty_usage, Some(message.clone()));
            log.clone().write_detached(entry);
            return http::error_response(status, message);
        }
    };
    let mut usage = extract_usage_from_response(&bytes);
    let response_error = if status.is_client_error() || status.is_server_error() {
        Some(response_error_text(&bytes))
    } else {
        None
    };
    let output = if status.is_success() {
        match response_transform {
            FormatTransform::KiroToResponses => {
                let converted = match kiro_to_responses::convert_kiro_response(
                    &bytes,
                    context.model.as_deref(),
                    estimated_input_tokens,
                ) {
                    Ok(converted) => converted,
                    Err(message) => {
                        let error_message = format!("Failed to transform upstream response: {message}");
                        context.status = StatusCode::BAD_GATEWAY.as_u16();
                        let entry = build_log_entry(&context, usage, Some(error_message.clone()));
                        log.clone().write_detached(entry);
                        return http::error_response(StatusCode::BAD_GATEWAY, error_message);
                    }
                };
                usage = resolve_kiro_usage(
                    &bytes,
                    &converted,
                    context.model.as_deref(),
                    estimated_input_tokens,
                );
                converted
            }
            FormatTransform::KiroToChat => {
                let responses = match kiro_to_responses::convert_kiro_response(
                    &bytes,
                    context.model.as_deref(),
                    estimated_input_tokens,
                ) {
                    Ok(converted) => converted,
                    Err(message) => {
                        let error_message = format!("Failed to transform upstream response: {message}");
                        context.status = StatusCode::BAD_GATEWAY.as_u16();
                        let entry = build_log_entry(&context, usage, Some(error_message.clone()));
                        log.clone().write_detached(entry);
                        return http::error_response(StatusCode::BAD_GATEWAY, error_message);
                    }
                };
                usage = resolve_kiro_usage(
                    &bytes,
                    &responses,
                    context.model.as_deref(),
                    estimated_input_tokens,
                );
                match transform_response_body(FormatTransform::ResponsesToChat, &responses, context.model.as_deref()) {
                    Ok(converted) => converted,
                    Err(message) => {
                        let error_message = format!("Failed to transform upstream response: {message}");
                        context.status = StatusCode::BAD_GATEWAY.as_u16();
                        let entry = build_log_entry(&context, usage, Some(error_message.clone()));
                        log.clone().write_detached(entry);
                        return http::error_response(StatusCode::BAD_GATEWAY, error_message);
                    }
                }
            }
            FormatTransform::KiroToAnthropic => {
                let converted = match kiro_to_anthropic::convert_kiro_response(
                    &bytes,
                    context.model.as_deref(),
                    estimated_input_tokens,
                ) {
                    Ok(converted) => converted,
                    Err(message) => {
                        let error_message = format!("Failed to transform upstream response: {message}");
                        context.status = StatusCode::BAD_GATEWAY.as_u16();
                        let entry = build_log_entry(&context, usage, Some(error_message.clone()));
                        log.clone().write_detached(entry);
                        return http::error_response(StatusCode::BAD_GATEWAY, error_message);
                    }
                };
                usage = resolve_kiro_usage(
                    &bytes,
                    &converted,
                    context.model.as_deref(),
                    estimated_input_tokens,
                );
                converted
            }
            _ if response_transform != FormatTransform::None => {
                match transform_response_body(response_transform, &bytes, context.model.as_deref()) {
                    Ok(converted) => converted,
                    Err(message) => {
                        let error_message = format!("Failed to transform upstream response: {message}");
                        context.status = StatusCode::BAD_GATEWAY.as_u16();
                        let entry = build_log_entry(&context, usage, Some(error_message.clone()));
                        log.clone().write_detached(entry);
                        return http::error_response(StatusCode::BAD_GATEWAY, error_message);
                    }
                }
            }
            _ => bytes,
        }
    } else {
        bytes
    };

    let entry = build_log_entry(&context, usage, response_error);
    log.clone().write_detached(entry);

    let output = maybe_override_response_model(output, model_override);
    let provider_for_tokens = match response_transform {
        FormatTransform::KiroToResponses => "openai-response",
        FormatTransform::KiroToChat => "openai",
        FormatTransform::KiroToAnthropic => "anthropic",
        _ => context.provider.as_str(),
    };
    token_count::apply_output_tokens_from_response(&request_tracker, provider_for_tokens, &output).await;

    http::build_response(status, headers, Body::from(output))
}

fn resolve_kiro_usage(
    raw_bytes: &Bytes,
    responses_bytes: &Bytes,
    model: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> UsageSnapshot {
    let usage = extract_usage_from_response(responses_bytes);
    if usage.usage.is_none() && usage.cached_tokens.is_none() && usage.usage_json.is_none() {
        if let Some(fallback) =
            kiro_to_responses::extract_kiro_usage_snapshot(raw_bytes, model, estimated_input_tokens)
        {
            return fallback;
        }
    }
    usage
}

// 只对 data-only SSE 的提供商做行级重写，避免破坏带 event: 行的流。
fn should_rewrite_sse_model(provider: &str) -> bool {
    provider == PROVIDER_OPENAI
        || provider == PROVIDER_OPENAI_RESPONSES
        || provider == PROVIDER_GEMINI
}

fn maybe_override_response_model(bytes: Bytes, model_override: Option<&str>) -> Bytes {
    let Some(model_override) = model_override else {
        return bytes;
    };
    model::rewrite_response_model(&bytes, model_override).unwrap_or(bytes)
}

fn stream_chat_to_responses<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    chat_to_responses::stream_chat_to_responses(upstream, context, log, token_tracker)
}

fn responses_event_sse(event: Value) -> Bytes {
    Bytes::from(format!("data: {}\n\n", event.to_string()))
}

fn anthropic_event_sse(event_type: &str, event: Value) -> Bytes {
    Bytes::from(format!("event: {event_type}\ndata: {}\n\n", event.to_string()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn response_error_text(bytes: &Bytes) -> String {
    let slice = bytes.as_ref();
    if slice.len() <= RESPONSE_ERROR_LIMIT_BYTES {
        return String::from_utf8_lossy(slice).to_string();
    }
    let truncated = &slice[..RESPONSE_ERROR_LIMIT_BYTES];
    format!("{}... (truncated)", String::from_utf8_lossy(truncated))
}

mod chat_to_responses;
mod anthropic_to_responses;
mod responses_to_chat;
mod responses_to_anthropic;
mod kiro_to_anthropic;
mod kiro_to_responses;
mod kiro_to_responses_helpers;
mod kiro_to_responses_stream;
mod streaming;
mod token_count;
mod upstream_read;
mod upstream_stream;

#[cfg(test)]
mod tests;
