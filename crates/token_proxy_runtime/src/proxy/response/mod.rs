use axum::{body::Bytes, http::StatusCode, response::Response};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

use super::{
    http,
    log::{LogContext, LogWriter, RequestTimings},
    openai,
    openai_compat::FormatTransform,
    request_detail::RequestDetailSnapshot,
    token_rate::RequestTokenTracker,
    RequestMeta,
};
use token_proxy_protocol::xai_client_tools::XaiClientToolMapping;

const PROVIDER_OPENAI: &str = "openai";
const PROVIDER_OPENAI_RESPONSES: &str = "openai-response";
const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_CODEX: &str = "codex";
const PROVIDER_XAI: &str = "xai";
const RESPONSE_ERROR_LIMIT_BYTES: usize = 256 * 1024;
const GEMINI_UPLOAD_URL_HEADER: &str = "x-goog-upload-url";
const GEMINI_PROXY_UPLOAD_TARGET_QUERY: &str = "tp_upload_target";
const GEMINI_UPLOAD_PROXY_PATH: &str = "/upload/v1beta/files";
const GEMINI_API_KEY_QUERY: &str = "key";
const IMAGE_GENERATION_MIN_NO_DATA_TIMEOUT: Duration = Duration::from_secs(300);

/// Track authoritative item IDs by Responses output index for terminal hydration.
pub(super) fn remember_responses_output_item_id(
    event: &Value,
    output_item_ids: &mut BTreeMap<i64, String>,
) {
    if !matches!(
        event.get("type").and_then(Value::as_str),
        Some("response.output_item.added") | Some("response.output_item.done")
    ) {
        return;
    }
    let Some(output_index) = event.get("output_index").and_then(Value::as_i64) else {
        return;
    };
    let Some(item_id) = event
        .get("item")
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
    else {
        return;
    };
    output_item_ids.insert(output_index, item_id.to_string());
}

/// Fill only missing/null/empty terminal item IDs from prior lifecycle events.
pub(super) fn hydrate_responses_output_item_ids(
    response: &mut Map<String, Value>,
    output_item_ids: &BTreeMap<i64, String>,
) -> usize {
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return 0;
    };
    let mut hydrated = 0;
    for (index, item) in output.iter_mut().enumerate() {
        let Some(item_object) = item.as_object_mut() else {
            continue;
        };
        let has_id = match item_object.get("id") {
            Some(Value::String(id)) => !id.trim().is_empty(),
            Some(Value::Null) | None => false,
            Some(_) => true,
        };
        if has_id {
            continue;
        }
        let Some(item_id) = output_item_ids.get(&(index as i64)) else {
            continue;
        };
        item_object.insert("id".to_string(), Value::String(item_id.clone()));
        hydrated += 1;
    }
    hydrated
}

#[derive(Clone)]
pub(super) struct RetryableStreamResponse {
    /// 上游流内错误的语义状态；外层 HTTP 可能仍是 200。
    pub(super) status: StatusCode,
    pub(super) message: String,
    pub(super) should_cooldown: bool,
}

#[derive(Clone)]
pub(crate) struct NonRetryableSemanticResponse;

#[derive(Clone, Copy)]
pub(super) struct AccountCooldownHint {
    pub(super) duration: Duration,
    pub(super) reason: &'static str,
}

pub(super) async fn build_proxy_response(
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    account_id: Option<String>,
    inbound_path: &str,
    upstream_res: reqwest::Response,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    start: Instant,
    timings: RequestTimings,
    proxy_base_url: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    xai_client_tools: Option<XaiClientToolMapping>,
    request_detail: Option<RequestDetailSnapshot>,
    stream_first_output_timeout: Duration,
    sync_response_timeout: Duration,
) -> Response {
    let sync_response_timeout = response_no_data_timeout(inbound_path, sync_response_timeout);
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
        client_ip: meta.client_ip.clone(),
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
        timings,
        start,
    };
    let model_override = meta.model_override();
    if response_transform != FormatTransform::None {
        // The body will change; let hyper recalculate the content length.
        response_headers.remove(axum::http::header::CONTENT_LENGTH);
    }
    // tracker 在上游发送前已 register（计 connections）；此处再写入 input 估计。
    apply_estimated_input_tokens(&request_tracker, meta.estimated_input_tokens).await;
    let should_stream = meta.stream
        && !status.is_client_error()
        && !status.is_server_error()
        && response_transform != FormatTransform::ResponsesInputTokensToAnthropicCountTokens;
    if should_stream {
        dispatch::build_stream_response(
            status,
            upstream_res,
            response_headers,
            context,
            log,
            request_tracker,
            response_transform,
            xai_client_tools,
            model_override,
            meta.estimated_input_tokens,
            meta.response_format.as_deref(),
            timeout_remaining(stream_first_output_timeout, start),
            stream_first_output_timeout,
            sync_response_timeout,
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
            xai_client_tools,
            model_override,
            meta.estimated_input_tokens,
            meta.response_format.as_deref(),
            sync_response_timeout,
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
    request_tracker: RequestTokenTracker,
    start: Instant,
    timings: RequestTimings,
    proxy_base_url: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    xai_client_tools: Option<XaiClientToolMapping>,
    request_detail: Option<RequestDetailSnapshot>,
    sync_response_timeout: Duration,
) -> Response {
    let sync_response_timeout = response_no_data_timeout(inbound_path, sync_response_timeout);
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
        client_ip: meta.client_ip.clone(),
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
        timings,
        start,
    };
    let model_override = meta.model_override();
    if response_transform != FormatTransform::None {
        response_headers.remove(axum::http::header::CONTENT_LENGTH);
    }
    apply_estimated_input_tokens(&request_tracker, meta.estimated_input_tokens).await;
    dispatch::build_buffered_response(
        status,
        upstream_res,
        response_headers,
        context,
        log,
        request_tracker,
        response_transform,
        xai_client_tools,
        model_override,
        meta.estimated_input_tokens,
        meta.response_format.as_deref(),
        sync_response_timeout,
    )
    .await
}

/// 响应阶段写入 prompt 估计；发送前 register 时不写，保证 TTFB 期间 ↑ 能显示 connections。
async fn apply_estimated_input_tokens(tracker: &RequestTokenTracker, estimated: Option<u64>) {
    let Some(tokens) = estimated.filter(|value| *value > 0) else {
        return;
    };
    tracing::debug!(tokens, "token_rate apply estimated input tokens");
    tracker.add_input_tokens(tokens).await;
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

fn response_no_data_timeout(inbound_path: &str, configured_timeout: Duration) -> Duration {
    if openai::is_openai_image_generations_path(inbound_path) {
        configured_timeout.max(IMAGE_GENERATION_MIN_NO_DATA_TIMEOUT)
    } else {
        configured_timeout
    }
}

fn timeout_remaining(timeout_duration: Duration, start_time: Instant) -> Duration {
    timeout_duration.saturating_sub(start_time.elapsed())
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
pub(crate) use token_proxy_protocol::responses_error;
pub(crate) use token_proxy_protocol::responses_failure;
pub(crate) use token_proxy_protocol::responses_sequence as sequence;

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
