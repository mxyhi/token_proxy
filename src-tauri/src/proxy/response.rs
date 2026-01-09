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

use super::{
    http,
    log::{build_log_entry, LogContext, LogWriter},
    model,
    openai_compat::{transform_response_body, FormatTransform},
    token_rate::{RequestTokenTracker, TokenRateTracker},
    usage::extract_usage_from_response,
    RequestMeta,
};

const PROVIDER_OPENAI: &str = "openai";
const PROVIDER_OPENAI_RESPONSES: &str = "openai-response";
const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";

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
) -> Response {
    let status = upstream_res.status();
    let mut response_headers = http::filter_response_headers(upstream_res.headers());
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: http::extract_request_id(upstream_res.headers()),
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
    if meta.stream {
        build_stream_response(
            status,
            upstream_res,
            response_headers,
            context,
            log,
            request_tracker,
            response_transform,
            model_override,
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
) -> Response {
    let status = upstream_res.status();
    let mut response_headers = http::filter_response_headers(upstream_res.headers());
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: http::extract_request_id(upstream_res.headers()),
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
) -> Response {
    let stream = match response_transform {
        FormatTransform::None => {
            if let Some(model_override) = model_override {
                if should_rewrite_sse_model(&context.provider) {
                    streaming::stream_with_logging_and_model_override(
                        upstream_res.bytes_stream(),
                        context,
                        log,
                        model_override.to_string(),
                        request_tracker,
                    )
                    .boxed()
                } else {
                    streaming::stream_with_logging(
                        upstream_res.bytes_stream(),
                        context,
                        log,
                        request_tracker,
                    )
                        .boxed()
                }
            } else {
                streaming::stream_with_logging(
                    upstream_res.bytes_stream(),
                    context,
                    log,
                    request_tracker,
                )
                    .boxed()
            }
        }
        FormatTransform::ResponsesToChat => {
            responses_to_chat::stream_responses_to_chat(
                upstream_res.bytes_stream(),
                context,
                log,
                request_tracker,
            )
                .boxed()
        }
        FormatTransform::ChatToResponses => {
            stream_chat_to_responses(upstream_res.bytes_stream(), context, log, request_tracker)
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
) -> Response {
    let bytes = match upstream_res.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            return http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read upstream response: {err}"),
            )
        }
    };
    let usage = extract_usage_from_response(&bytes);
    let entry = build_log_entry(&context, usage);
    log.write(&entry).await;

    let output = if response_transform != FormatTransform::None && status.is_success() {
        match transform_response_body(response_transform, &bytes, context.model.as_deref()) {
            Ok(converted) => converted,
            Err(message) => {
                return http::error_response(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to transform upstream response: {message}"),
                )
            }
        }
    } else {
        bytes
    };

    let output = maybe_override_response_model(output, model_override);
    apply_output_tokens_from_response(&request_tracker, &context.provider, &output).await;

    http::build_response(status, headers, Body::from(output))
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

fn stream_chat_to_responses(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, reqwest::Error>>
        + Unpin
        + Send
        + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    chat_to_responses::stream_chat_to_responses(upstream, context, log, token_tracker)
}

fn responses_event_sse(event: Value) -> Bytes {
    Bytes::from(format!("data: {}\n\n", event.to_string()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn apply_output_tokens_from_response(
    tracker: &RequestTokenTracker,
    provider: &str,
    bytes: &Bytes,
) {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let mut texts = Vec::new();

    match provider {
        PROVIDER_OPENAI | PROVIDER_OPENAI_RESPONSES => {
            if let Some(choices) = value.get("choices").and_then(Value::as_array) {
                for choice in choices {
                    if let Some(content) = choice.get("message").and_then(|message| message.get("content")) {
                        if let Some(text) = content.as_str() {
                            texts.push(text.to_string());
                        } else if let Some(parts) = content.as_array() {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    texts.push(text.to_string());
                                }
                            }
                        }
                    }
                    if let Some(text) = choice.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
            if texts.is_empty() {
                if let Some(output) = value.get("output").and_then(Value::as_array) {
                    collect_responses_output(output, &mut texts);
                }
            }
        }
        PROVIDER_ANTHROPIC => {
            if let Some(content) = value.get("content").and_then(Value::as_array) {
                for item in content {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
        }
        PROVIDER_GEMINI => {
            if let Some(candidates) = value.get("candidates").and_then(Value::as_array) {
                collect_gemini_output(candidates, &mut texts);
            }
        }
        _ => {}
    }
    if texts.is_empty() {
        return;
    }
    for text in texts {
        tracker.add_output_text(&text).await;
    }
}

fn collect_responses_output(output: &[Value], texts: &mut Vec<String>) {
    for item in output {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    texts.push(text.to_string());
                }
            }
        }
    }
}

fn collect_gemini_output(candidates: &[Value], texts: &mut Vec<String>) {
    for candidate in candidates {
        if let Some(content) = candidate.get("content") {
            if let Some(parts) = content.get("parts").and_then(Value::as_array) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }
}

mod chat_to_responses;
mod responses_to_chat;
mod streaming;

#[cfg(test)]
mod tests;
