use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use std::sync::Arc;
use std::time::Duration;

use super::super::super::{
    antigravity_compat, codex_compat, http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    model,
    openai_compat::{transform_response_body, FormatTransform},
    redact::redact_query_param_value,
    request_body::ReplayableBody,
    server_helpers::{log_debug_headers_body, truncate_for_log},
    token_rate::RequestTokenTracker,
    usage::extract_usage_from_response,
};
use super::super::{
    kiro_to_anthropic, kiro_to_responses, token_count, upstream_read, upstream_stream,
    PROVIDER_ANTIGRAVITY, PROVIDER_GEMINI, RESPONSE_ERROR_LIMIT_BYTES,
};

const DEBUG_BODY_LOG_LIMIT_BYTES: usize = usize::MAX;
const ANTIGRAVITY_ERROR_LOG_LIMIT_BYTES: usize = 8 * 1024;

pub(super) async fn build_buffered_response(
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
    let response_headers = upstream_res.headers().clone();
    let bytes =
        match read_upstream_bytes(upstream_res, &mut context, &log, upstream_no_data_timeout).await
        {
            Ok(bytes) => bytes,
            Err(response) => return response,
        };
    log_debug_headers_body(
        "upstream.response.raw",
        Some(&response_headers),
        Some(&ReplayableBody::from_bytes(bytes.clone())),
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    if context.provider == PROVIDER_ANTIGRAVITY && !status.is_success() {
        log_antigravity_error_body(status, &bytes);
    }
    let bytes = if context.provider == PROVIDER_ANTIGRAVITY && status.is_success() {
        match antigravity_compat::unwrap_response(&bytes) {
            Ok(unwrapped) => unwrapped,
            Err(message) => {
                return http::error_response(StatusCode::BAD_GATEWAY, message);
            }
        }
    } else {
        bytes
    };
    if context.provider == PROVIDER_ANTIGRAVITY {
        log_debug_headers_body(
            "upstream.response.unwrapped",
            Some(&response_headers),
            Some(&ReplayableBody::from_bytes(bytes.clone())),
            DEBUG_BODY_LOG_LIMIT_BYTES,
        )
        .await;
    }
    let mut usage = extract_usage_from_response(&bytes);
    let response_error = response_error_for_status(status, &bytes);
    let request_body = context.request_body.clone();
    let output = if status.is_success() {
        match convert_success_body(
            response_transform,
            &bytes,
            &mut context,
            usage,
            log.clone(),
            estimated_input_tokens,
            request_body.as_deref(),
        ) {
            Ok(converted) => {
                usage = converted.usage;
                converted.output
            }
            Err(response) => return response,
        }
    } else {
        bytes
    };

    let entry = build_log_entry(&context, usage, response_error);
    log.clone().write_detached(entry);

    let output = maybe_override_response_model(output, model_override);
    log_debug_headers_body(
        "outbound.response",
        Some(&headers),
        Some(&ReplayableBody::from_bytes(output.clone())),
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    let provider_for_tokens = provider_for_tokens(response_transform, context.provider.as_str());
    token_count::apply_output_tokens_from_response(&request_tracker, provider_for_tokens, &output)
        .await;

    http::build_response(status, headers, Body::from(output))
}

struct ConvertedBody {
    output: Bytes,
    usage: UsageSnapshot,
}

fn convert_success_body(
    transform: FormatTransform,
    bytes: &Bytes,
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
    estimated_input_tokens: Option<u64>,
    request_body: Option<&str>,
) -> Result<ConvertedBody, Response> {
    match transform {
        FormatTransform::KiroToAnthropic => {
            convert_kiro_to_anthropic_body(bytes, context, usage, log, estimated_input_tokens)
        }
        FormatTransform::CodexToChat => {
            convert_codex_to_chat_body(bytes, context, usage, log, request_body)
        }
        FormatTransform::CodexToResponses => {
            convert_codex_to_responses_body(bytes, context, usage, log, request_body)
        }
        FormatTransform::CodexToAnthropic => {
            convert_codex_to_anthropic_body(bytes, context, usage, log, request_body)
        }
        _ if transform != FormatTransform::None => {
            convert_generic_body(transform, bytes, context, usage, log)
        }
        _ => Ok(ConvertedBody {
            output: bytes.clone(),
            usage,
        }),
    }
}

fn log_antigravity_error_body(status: StatusCode, bytes: &Bytes) {
    let body_text = String::from_utf8_lossy(bytes);
    let truncated = truncate_for_log(&body_text, ANTIGRAVITY_ERROR_LOG_LIMIT_BYTES);
    // 仅在错误时记录，避免日志噪音与性能影响。
    tracing::warn!(
        status = %status,
        body = %truncated,
        "antigravity upstream error body"
    );
}

fn convert_kiro_to_anthropic_body(
    bytes: &Bytes,
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
    estimated_input_tokens: Option<u64>,
) -> Result<ConvertedBody, Response> {
    let converted = match kiro_to_anthropic::convert_kiro_response(
        bytes,
        context.model.as_deref(),
        estimated_input_tokens,
    ) {
        Ok(converted) => converted,
        Err(message) => {
            return Err(respond_transform_error(context, usage, log, message));
        }
    };
    let usage = resolve_kiro_usage(
        bytes,
        &converted,
        context.model.as_deref(),
        estimated_input_tokens,
    );
    Ok(ConvertedBody {
        output: converted,
        usage,
    })
}

fn convert_codex_to_chat_body(
    bytes: &Bytes,
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
    request_body: Option<&str>,
) -> Result<ConvertedBody, Response> {
    let converted = match codex_compat::codex_response_to_chat(bytes, request_body) {
        Ok(converted) => converted,
        Err(message) => {
            return Err(respond_transform_error(context, usage, log, message));
        }
    };
    Ok(ConvertedBody {
        output: converted,
        usage,
    })
}

fn convert_codex_to_responses_body(
    bytes: &Bytes,
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
    request_body: Option<&str>,
) -> Result<ConvertedBody, Response> {
    let converted = match codex_compat::codex_response_to_responses(bytes, request_body) {
        Ok(converted) => converted,
        Err(message) => {
            return Err(respond_transform_error(context, usage, log, message));
        }
    };
    Ok(ConvertedBody {
        output: converted,
        usage,
    })
}

fn convert_codex_to_anthropic_body(
    bytes: &Bytes,
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
    request_body: Option<&str>,
) -> Result<ConvertedBody, Response> {
    let responses = match codex_compat::codex_response_to_responses(bytes, request_body) {
        Ok(converted) => converted,
        Err(message) => {
            return Err(respond_transform_error(context, usage, log, message));
        }
    };
    let anthropic = match transform_response_body(
        FormatTransform::ResponsesToAnthropic,
        &responses,
        context.model.as_deref(),
    ) {
        Ok(converted) => converted,
        Err(message) => {
            return Err(respond_transform_error(context, usage, log, message));
        }
    };
    Ok(ConvertedBody {
        output: anthropic,
        usage,
    })
}

fn convert_generic_body(
    transform: FormatTransform,
    bytes: &Bytes,
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
) -> Result<ConvertedBody, Response> {
    let converted = match transform_response_body(transform, bytes, context.model.as_deref()) {
        Ok(converted) => converted,
        Err(message) => {
            return Err(respond_transform_error(context, usage, log, message));
        }
    };
    Ok(ConvertedBody {
        output: converted,
        usage,
    })
}

async fn read_upstream_bytes(
    upstream_res: reqwest::Response,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    upstream_no_data_timeout: Duration,
) -> Result<Bytes, Response> {
    let bytes = match upstream_read::read_upstream_bytes_with_ttfb(
        upstream_res,
        context,
        upstream_no_data_timeout,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(err) => {
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
            return Err(http::error_response(status, message));
        }
    };
    Ok(bytes)
}

fn respond_transform_error(
    context: &mut LogContext,
    usage: UsageSnapshot,
    log: Arc<LogWriter>,
    message: String,
) -> Response {
    let error_message = format!("Failed to transform upstream response: {message}");
    context.status = StatusCode::BAD_GATEWAY.as_u16();
    let entry = build_log_entry(context, usage, Some(error_message.clone()));
    log.clone().write_detached(entry);
    http::error_response(StatusCode::BAD_GATEWAY, error_message)
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

fn maybe_override_response_model(bytes: Bytes, model_override: Option<&str>) -> Bytes {
    let Some(model_override) = model_override else {
        return bytes;
    };
    model::rewrite_response_model(&bytes, model_override).unwrap_or(bytes)
}

fn response_error_text(bytes: &Bytes) -> String {
    let slice = bytes.as_ref();
    if slice.len() <= RESPONSE_ERROR_LIMIT_BYTES {
        return String::from_utf8_lossy(slice).to_string();
    }
    let truncated = &slice[..RESPONSE_ERROR_LIMIT_BYTES];
    format!("{}... (truncated)", String::from_utf8_lossy(truncated))
}

fn response_error_for_status(status: StatusCode, bytes: &Bytes) -> Option<String> {
    if status.is_client_error() || status.is_server_error() {
        Some(response_error_text(bytes))
    } else {
        None
    }
}

fn provider_for_tokens(transform: FormatTransform, provider: &str) -> &str {
    match transform {
        FormatTransform::KiroToAnthropic => "anthropic",
        FormatTransform::CodexToChat => "openai",
        FormatTransform::CodexToResponses => "openai-response",
        FormatTransform::CodexToAnthropic => "anthropic",
        _ if provider == PROVIDER_ANTIGRAVITY => PROVIDER_GEMINI,
        _ => provider,
    }
}
