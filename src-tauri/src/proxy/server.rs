use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use std::{
    sync::Arc,
    time::Instant,
};
use tokio::sync::RwLock;

use super::{
    config::ProxyConfig,
    gemini,
    http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai_compat::{
        inbound_format, ApiFormat, FormatTransform, CHAT_PATH, PROVIDER_CHAT, PROVIDER_RESPONSES,
        RESPONSES_PATH,
    },
    request_detail::{capture_request_detail, serialize_request_headers, RequestDetailSnapshot},
    request_body::ReplayableBody,
    server_helpers::{
        extract_request_path, is_anthropic_path, log_debug_request,
        maybe_force_openai_stream_options_include_usage, maybe_transform_request_body,
        parse_request_meta_best_effort,
    },
    upstream::forward_upstream_request,
    ProxyState, RequestMeta,
};
use crate::logging::LogLevel;

const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_PROXY: &str = "proxy";
const LOCAL_UPSTREAM_ID: &str = "local";

type ProxyStateHandle = Arc<RwLock<Arc<ProxyState>>>;

mod bootstrap;
pub(crate) use bootstrap::{build_router, build_upstream_cursors};

struct DispatchPlan {
    provider: &'static str,
    outbound_path: Option<&'static str>,
    request_transform: FormatTransform,
    response_transform: FormatTransform,
}

struct PreparedRequest {
    path: String,
    outbound_path_with_query: String,
    plan: DispatchPlan,
    meta: RequestMeta,
    request_detail: Option<RequestDetailSnapshot>,
    outbound_body: ReplayableBody,
    request_auth: http::RequestAuth,
}

struct InboundRequest {
    path: String,
    plan: DispatchPlan,
    body: ReplayableBody,
    meta: RequestMeta,
    request_detail: Option<RequestDetailSnapshot>,
}

const ERROR_NO_UPSTREAM: &str = "No available upstream configured.";
const ERROR_CHAT_CONVERSION_DISABLED: &str =
    "OpenAI format conversion is disabled (enable_api_format_conversion=false). Configure provider \"openai\" for /v1/chat/completions or enable conversion.";
const ERROR_RESPONSES_CONVERSION_DISABLED: &str =
    "OpenAI format conversion is disabled (enable_api_format_conversion=false). Configure provider \"openai-response\" for /v1/responses or enable conversion.";

fn base_plan(provider: &'static str) -> DispatchPlan {
    DispatchPlan {
        provider,
        outbound_path: None,
        request_transform: FormatTransform::None,
        response_transform: FormatTransform::None,
    }
}

fn resolve_gemini_plan(has_gemini: bool, path: &str) -> Option<Result<DispatchPlan, String>> {
    if !gemini::is_gemini_path(path) {
        return None;
    }
    if has_gemini {
        return Some(Ok(base_plan(PROVIDER_GEMINI)));
    }
    Some(Err(ERROR_NO_UPSTREAM.to_string()))
}

fn resolve_anthropic_plan(
    has_anthropic: bool,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    if !is_anthropic_path(path) {
        return None;
    }
    if has_anthropic {
        return Some(Ok(base_plan(PROVIDER_ANTHROPIC)));
    }
    Some(Err(ERROR_NO_UPSTREAM.to_string()))
}

fn resolve_formatless_plan(
    has_chat: bool,
    has_responses: bool,
    has_anthropic: bool,
) -> Result<DispatchPlan, String> {
    if has_chat {
        return Ok(base_plan(PROVIDER_CHAT));
    }
    if has_responses {
        return Ok(base_plan(PROVIDER_RESPONSES));
    }
    if has_anthropic {
        return Ok(base_plan(PROVIDER_ANTHROPIC));
    }
    Err(ERROR_NO_UPSTREAM.to_string())
}

fn resolve_chat_plan(
    has_chat: bool,
    has_responses: bool,
    allow_format_conversion: bool,
) -> Result<DispatchPlan, String> {
    if has_chat {
        return Ok(base_plan(PROVIDER_CHAT));
    }
    if has_responses {
        if !allow_format_conversion {
            return Err(ERROR_CHAT_CONVERSION_DISABLED.to_string());
        }
        return Ok(DispatchPlan {
            provider: PROVIDER_RESPONSES,
            outbound_path: Some(RESPONSES_PATH),
            request_transform: FormatTransform::ChatToResponses,
            response_transform: FormatTransform::ResponsesToChat,
        });
    }
    Err(ERROR_NO_UPSTREAM.to_string())
}

fn resolve_responses_plan(
    has_responses: bool,
    has_chat: bool,
    allow_format_conversion: bool,
) -> Result<DispatchPlan, String> {
    if has_responses {
        return Ok(base_plan(PROVIDER_RESPONSES));
    }
    if has_chat {
        if !allow_format_conversion {
            return Err(ERROR_RESPONSES_CONVERSION_DISABLED.to_string());
        }
        return Ok(DispatchPlan {
            provider: PROVIDER_CHAT,
            outbound_path: Some(CHAT_PATH),
            request_transform: FormatTransform::ResponsesToChat,
            response_transform: FormatTransform::ChatToResponses,
        });
    }
    Err(ERROR_NO_UPSTREAM.to_string())
}

fn resolve_dispatch_plan(config: &ProxyConfig, path: &str) -> Result<DispatchPlan, String> {
    let has_chat = config.provider_upstreams(PROVIDER_CHAT).is_some();
    let has_responses = config.provider_upstreams(PROVIDER_RESPONSES).is_some();
    let has_anthropic = config.provider_upstreams(PROVIDER_ANTHROPIC).is_some();
    let has_gemini = config.provider_upstreams(PROVIDER_GEMINI).is_some();
    let allow_format_conversion = config.enable_api_format_conversion;

    if let Some(plan) = resolve_gemini_plan(has_gemini, path) {
        return plan;
    }
    if let Some(plan) = resolve_anthropic_plan(has_anthropic, path) {
        return plan;
    }

    let Some(format) = inbound_format(path) else {
        return resolve_formatless_plan(has_chat, has_responses, has_anthropic);
    };

    match format {
        ApiFormat::ChatCompletions => {
            resolve_chat_plan(has_chat, has_responses, allow_format_conversion)
        }
        ApiFormat::Responses => {
            resolve_responses_plan(has_responses, has_chat, allow_format_conversion)
        }
    }
}

async fn capture_detail_from_body(
    headers: &HeaderMap,
    body: Body,
    max_body_bytes: usize,
) -> RequestDetailSnapshot {
    match ReplayableBody::from_body(body).await {
        Ok(replayable) => capture_request_detail(headers, &replayable, max_body_bytes).await,
        Err(err) => RequestDetailSnapshot {
            request_headers: serialize_request_headers(headers),
            request_body: Some(format!("Failed to read request body: {err}")),
        },
    }
}

fn log_request_error(
    log: &Arc<LogWriter>,
    detail: Option<RequestDetailSnapshot>,
    path: &str,
    provider: &str,
    upstream_id: &str,
    status: StatusCode,
    response_error: String,
    start: Instant,
) {
    let Some(detail) = detail else {
        return;
    };
    let context = LogContext {
        path: path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: None,
        mapped_model: None,
        stream: false,
        status: status.as_u16(),
        upstream_request_id: None,
        request_headers: detail.request_headers,
        request_body: detail.request_body,
        start,
    };
    let usage = UsageSnapshot {
        usage: None,
        cached_tokens: None,
        usage_json: None,
    };
    let entry = build_log_entry(&context, usage, Some(response_error));
    log.clone().write_detached(entry);
}

async fn ensure_local_auth_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_next: bool,
    path: &str,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<Body, Response> {
    if let Err(message) = http::ensure_local_auth(config, headers) {
        tracing::warn!("local auth failed");
        if capture_next {
            let detail = capture_detail_from_body(headers, body, max_body_bytes).await;
            log_request_error(
                log,
                Some(detail),
                path,
                PROVIDER_PROXY,
                LOCAL_UPSTREAM_ID,
                StatusCode::UNAUTHORIZED,
                message.clone(),
                request_start,
            );
        }
        return Err(http::error_response(StatusCode::UNAUTHORIZED, message));
    }
    Ok(body)
}

async fn resolve_plan_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_next: bool,
    path: &str,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<(DispatchPlan, Body), Response> {
    match resolve_dispatch_plan(config, path) {
        Ok(plan) => {
            tracing::debug!(provider = %plan.provider, "dispatch plan resolved");
            Ok((plan, body))
        }
        Err(message) => {
            tracing::warn!("no dispatch plan found");
            if capture_next {
                let detail = capture_detail_from_body(headers, body, max_body_bytes).await;
                log_request_error(
                    log,
                    Some(detail),
                    path,
                    PROVIDER_PROXY,
                    LOCAL_UPSTREAM_ID,
                    StatusCode::BAD_GATEWAY,
                    message.clone(),
                    request_start,
                );
            }
            Err(http::error_response(StatusCode::BAD_GATEWAY, message))
        }
    }
}

async fn read_body_or_respond(
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_next: bool,
    path: &str,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match ReplayableBody::from_body(body).await {
        Ok(body) => Ok(body),
        Err(err) => {
            let message = format!("Failed to read request body: {err}");
            if capture_next {
                let detail = RequestDetailSnapshot {
                    request_headers: serialize_request_headers(headers),
                    request_body: Some(message.clone()),
                };
                log_request_error(
                    log,
                    Some(detail),
                    path,
                    PROVIDER_PROXY,
                    LOCAL_UPSTREAM_ID,
                    StatusCode::BAD_REQUEST,
                    message.clone(),
                    request_start,
                );
            }
            Err(http::error_response(StatusCode::BAD_REQUEST, message))
        }
    }
}

async fn build_outbound_body_or_respond(
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    let body = match maybe_transform_request_body(plan.request_transform, body).await {
        Ok(body) => body,
        Err(err) => {
            log_request_error(
                log,
                request_detail.clone(),
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            return Err(http::error_response(err.status, err.message));
        }
    };

    match maybe_force_openai_stream_options_include_usage(
        plan.provider,
        plan.outbound_path.unwrap_or(path),
        meta,
        body,
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(err) => {
            log_request_error(
                log,
                request_detail,
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            Err(http::error_response(err.status, err.message))
        }
    }
}

fn resolve_request_auth_or_respond(
    config: &ProxyConfig,
    headers: &HeaderMap,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    provider: &str,
    request_start: Instant,
) -> Result<http::RequestAuth, Response> {
    match http::resolve_request_auth(config, headers) {
        Ok(auth) => Ok(auth),
        Err(message) => {
            log_request_error(
                log,
                request_detail,
                path,
                provider,
                LOCAL_UPSTREAM_ID,
                StatusCode::UNAUTHORIZED,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::UNAUTHORIZED, message))
        }
    }
}

fn build_outbound_path_with_query(outbound_path: &str, uri: &Uri) -> String {
    uri.query()
        .map(|query| format!("{outbound_path}?{query}"))
        .unwrap_or_else(|| outbound_path.to_string())
}

async fn prepare_inbound_request(
    state: &ProxyState,
    headers: &HeaderMap,
    path: String,
    body: Body,
    capture_next: bool,
    request_start: Instant,
    is_debug_log: bool,
) -> Result<InboundRequest, Response> {
    let body = ensure_local_auth_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_next,
        &path,
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let (plan, body) = resolve_plan_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_next,
        &path,
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let body = read_body_or_respond(&state.log, headers, body, capture_next, &path, request_start)
        .await?;
    if is_debug_log {
        log_debug_request(headers, &body).await;
    }
    let meta = parse_request_meta_best_effort(&path, &body).await;
    let request_detail = if capture_next {
        Some(
            capture_request_detail(headers, &body, state.config.max_request_body_bytes).await,
        )
    } else {
        None
    };
    Ok(InboundRequest {
        path,
        plan,
        meta,
        request_detail,
        body,
    })
}

async fn finalize_prepared_request(
    state: &ProxyState,
    headers: &HeaderMap,
    uri: &Uri,
    inbound: InboundRequest,
    request_start: Instant,
) -> Result<PreparedRequest, Response> {
    let outbound_path = inbound.plan.outbound_path.unwrap_or(inbound.path.as_str());
    let outbound_path_with_query = build_outbound_path_with_query(outbound_path, uri);
    let outbound_body = build_outbound_body_or_respond(
        &state.log,
        inbound.request_detail.clone(),
        &inbound.path,
        &inbound.plan,
        &inbound.meta,
        inbound.body,
        request_start,
    )
    .await?;
    let request_auth = resolve_request_auth_or_respond(
        &state.config,
        headers,
        &state.log,
        inbound.request_detail.clone(),
        &inbound.path,
        inbound.plan.provider,
        request_start,
    )?;
    Ok(PreparedRequest {
        path: inbound.path,
        outbound_path_with_query,
        plan: inbound.plan,
        meta: inbound.meta,
        request_detail: inbound.request_detail,
        outbound_body,
        request_auth,
    })
}

async fn proxy_request(
    State(state): State<ProxyStateHandle>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // 只在此处短暂持有读锁，避免影响并发请求性能。
    let state = { state.read().await.clone() };
    let request_start = Instant::now();
    let capture_next = state.request_detail.take();
    let is_debug_log = cfg!(debug_assertions)
        && matches!(state.config.log_level, LogLevel::Debug | LogLevel::Trace);
    let (path, _) = extract_request_path(&uri);
    tracing::info!(method = %method, path = %path, "incoming request");
    tracing::debug!(headers = ?headers.keys().collect::<Vec<_>>(), "request headers");

    let inbound = match prepare_inbound_request(
        &state,
        &headers,
        path,
        body,
        capture_next,
        request_start,
        is_debug_log,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(response) => return response,
    };
    let prepared = match finalize_prepared_request(&state, &headers, &uri, inbound, request_start)
        .await
    {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    forward_upstream_request(
        state,
        method,
        prepared.plan.provider,
        &prepared.path,
        &prepared.outbound_path_with_query,
        headers,
        prepared.outbound_body,
        prepared.meta,
        prepared.request_auth,
        prepared.plan.response_transform,
        prepared.request_detail,
    )
    .await
}

#[cfg(test)]
mod tests;
