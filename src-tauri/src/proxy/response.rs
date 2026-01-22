use axum::{body::Bytes, response::Response};
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    http,
    log::{LogContext, LogWriter},
    openai_compat::FormatTransform,
    token_rate::TokenRateTracker,
    request_detail::RequestDetailSnapshot,
    RequestMeta,
};

const PROVIDER_OPENAI: &str = "openai";
const PROVIDER_OPENAI_RESPONSES: &str = "openai-response";
const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_ANTIGRAVITY: &str = "antigravity";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_CODEX: &str = "codex";
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
        dispatch::build_stream_response(
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
        dispatch::build_buffered_response(
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
    dispatch::build_buffered_response(
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

#[cfg(test)]
fn stream_chat_to_responses<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: super::token_rate::RequestTokenTracker,
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

mod chat_to_responses;
mod anthropic_to_responses;
mod responses_to_chat;
mod responses_to_anthropic;
mod kiro_to_anthropic;
mod kiro_to_responses;
mod kiro_to_responses_helpers;
mod kiro_to_responses_stream;
mod dispatch;
mod streaming;
mod token_count;
mod upstream_read;
mod upstream_stream;

#[cfg(test)]
mod tests;
