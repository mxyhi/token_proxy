use axum::{body::Bytes, response::Response};
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

use super::{
    http,
    log::{LogContext, LogWriter},
    openai_compat::FormatTransform,
    request_detail::RequestDetailSnapshot,
    token_rate::TokenRateTracker,
    RequestMeta,
};

const PROVIDER_OPENAI: &str = "openai";
const PROVIDER_OPENAI_RESPONSES: &str = "openai-response";
const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_CODEX: &str = "codex";
const RESPONSE_ERROR_LIMIT_BYTES: usize = 256 * 1024;
const GEMINI_UPLOAD_URL_HEADER: &str = "x-goog-upload-url";
const GEMINI_PROXY_UPLOAD_TARGET_QUERY: &str = "tp_upload_target";
const GEMINI_UPLOAD_PROXY_PATH: &str = "/upload/v1beta/files";
const GEMINI_API_KEY_QUERY: &str = "key";

#[derive(Clone)]
pub(super) struct RetryableStreamResponse {
    pub(super) message: String,
    pub(super) should_cooldown: bool,
}

pub(super) async fn build_proxy_response(
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    account_id: Option<String>,
    inbound_path: &str,
    upstream_res: reqwest::Response,
    log: Arc<LogWriter>,
    token_rate: Arc<TokenRateTracker>,
    start: Instant,
    proxy_base_url: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    upstream_no_data_timeout: Duration,
) -> Response {
    let status = upstream_res.status();
    let mut response_headers = http::filter_response_headers(upstream_res.headers());
    maybe_rewrite_gemini_upload_url(
        provider,
        &mut response_headers,
        proxy_base_url,
        client_gemini_api_key,
    );
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers, detail.request_body))
        .unwrap_or((None, None));
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        account_id,
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
            upstream_no_data_timeout,
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
            upstream_no_data_timeout,
        )
        .await
    }
}

pub(super) async fn build_proxy_response_buffered(
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    account_id: Option<String>,
    inbound_path: &str,
    upstream_res: reqwest::Response,
    log: Arc<LogWriter>,
    token_rate: Arc<TokenRateTracker>,
    start: Instant,
    proxy_base_url: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    upstream_no_data_timeout: Duration,
) -> Response {
    let status = upstream_res.status();
    let mut response_headers = http::filter_response_headers(upstream_res.headers());
    maybe_rewrite_gemini_upload_url(
        provider,
        &mut response_headers,
        proxy_base_url,
        client_gemini_api_key,
    );
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers, detail.request_body))
        .unwrap_or((None, None));
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        account_id,
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
        upstream_no_data_timeout,
    )
    .await
}

fn maybe_rewrite_gemini_upload_url(
    provider: &str,
    response_headers: &mut axum::http::HeaderMap,
    proxy_base_url: &str,
    client_gemini_api_key: Option<&str>,
) {
    if provider != PROVIDER_GEMINI {
        return;
    }
    let Some(upload_url) = response_headers
        .get(GEMINI_UPLOAD_URL_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return;
    };
    let Ok(proxy_url) = build_proxy_upload_url(proxy_base_url, upload_url, client_gemini_api_key)
    else {
        return;
    };
    let Ok(value) = axum::http::HeaderValue::from_str(proxy_url.as_str()) else {
        return;
    };
    response_headers.insert(GEMINI_UPLOAD_URL_HEADER, value);
}

fn build_proxy_upload_url(
    proxy_base_url: &str,
    upstream_upload_url: &str,
    client_gemini_api_key: Option<&str>,
) -> Result<Url, url::ParseError> {
    let mut sanitized_target = Url::parse(upstream_upload_url)?;
    let pairs = sanitized_target
        .query_pairs()
        .filter(|(name, _)| name != GEMINI_API_KEY_QUERY)
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    sanitized_target.set_query(None);
    if !pairs.is_empty() {
        let mut query = sanitized_target.query_pairs_mut();
        for (name, value) in pairs {
            query.append_pair(&name, &value);
        }
    }
    let mut proxy_url = Url::parse(proxy_base_url)?;
    proxy_url.set_path(GEMINI_UPLOAD_PROXY_PATH);
    proxy_url.set_query(None);
    {
        let mut pairs = proxy_url.query_pairs_mut();
        pairs.append_pair(GEMINI_PROXY_UPLOAD_TARGET_QUERY, sanitized_target.as_str());
        if let Some(api_key) = client_gemini_api_key {
            pairs.append_pair(GEMINI_API_KEY_QUERY, api_key);
        }
    }
    Ok(proxy_url)
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
    Bytes::from(format!(
        "event: {event_type}\ndata: {}\n\n",
        event.to_string()
    ))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

mod anthropic_to_responses;
mod chat_to_responses;
mod dispatch;
mod kiro_to_anthropic;
mod kiro_to_responses;
mod kiro_to_responses_helpers;
mod responses_to_anthropic;
mod responses_to_chat;
mod streaming;
mod token_count;
mod upstream_read;
mod upstream_stream;

pub(crate) use streaming::STREAM_DROPPED_ERROR;

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "response.test.rs"]
mod tests;
