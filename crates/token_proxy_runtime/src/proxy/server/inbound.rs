use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use std::{sync::Arc, time::Instant};

use super::super::{
    config::ProxyConfig,
    http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai, path_guard,
    request_body::ReplayableBody,
    request_detail::{capture_request_detail, serialize_request_headers, RequestDetailSnapshot},
    server_helpers::{log_debug_request, parse_request_meta_best_effort},
    RequestMeta,
};
use super::{
    dispatch::{resolve_dispatch_plan_with_request, DispatchPlan},
    ProxyState, LOCAL_UPSTREAM_ID, PROVIDER_PROXY,
};

pub(crate) struct InboundRequest {
    pub(crate) request_auth: http::RequestAuth,
    pub(crate) path: String,
    pub(crate) client_ip: Option<String>,
    pub(crate) plan: DispatchPlan,
    pub(crate) body: ReplayableBody,
    pub(crate) meta: RequestMeta,
    pub(crate) request_detail: Option<RequestDetailSnapshot>,
}

pub(super) async fn prepare_inbound_request(
    state: &ProxyState,
    headers: &HeaderMap,
    method: &Method,
    path: String,
    query: Option<String>,
    body: Body,
    capture_request_detail_enabled: bool,
    client_ip: Option<String>,
    request_start: Instant,
    is_debug_log: bool,
) -> Result<InboundRequest, Response> {
    let (body, local_access) = ensure_local_auth_or_respond(
        &state.config,
        &state.log,
        headers,
        method,
        body,
        capture_request_detail_enabled,
        client_ip.clone(),
        &path,
        query.as_deref(),
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    if let Err(message) = path_guard::validate_sensitive_path(&path) {
        tracing::warn!("rejected unsafe inbound upstream path");
        log_request_error(
            &state.log,
            None,
            client_ip,
            path_guard::REDACTED_INVALID_PATH,
            PROVIDER_PROXY,
            LOCAL_UPSTREAM_ID,
            StatusCode::BAD_REQUEST,
            message.to_string(),
            request_start,
        );
        return Err(http::error_response(StatusCode::BAD_REQUEST, message));
    }
    let body = read_body_or_respond(
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
        client_ip.clone(),
        &path,
        request_start,
    )
    .await?;
    if is_debug_log {
        log_debug_request(headers, &body).await;
    }
    let path_with_query = query
        .as_deref()
        .map(|query| format!("{path}?{query}"))
        .unwrap_or_else(|| path.clone());
    let mut meta = parse_request_meta_best_effort(
        &path_with_query,
        &body,
        state.config.max_request_body_bytes,
    )
    .await;
    meta.client_ip = client_ip.clone();
    meta.billing.local_api_key_id = local_access
        .as_ref()
        .map(|access| access.key_id.as_str().into());
    let request_detail = if capture_request_detail_enabled {
        Some(capture_request_detail(headers, &body, state.config.max_request_body_bytes).await)
    } else {
        None
    };
    let mut request_auth = super::prepared::resolve_request_auth_or_respond(
        &state.config,
        headers,
        &state.log,
        request_detail.clone(),
        client_ip.clone(),
        &path,
        PROVIDER_PROXY,
        request_start,
    )?;
    request_auth.local_access = local_access;
    request_auth.target_upstream_id = http::authorize_target_upstream(
        &state.config,
        &request_auth,
        meta.original_model.as_deref(),
    )
    .map_err(|message| {
        log_request_error(
            &state.log,
            request_detail.clone(),
            client_ip.clone(),
            &path,
            PROVIDER_PROXY,
            LOCAL_UPSTREAM_ID,
            StatusCode::FORBIDDEN,
            message.clone(),
            request_start,
        );
        http::error_response(StatusCode::FORBIDDEN, message)
    })?;
    // 仅路由规划使用授权后的配置视图；实际调度保留原始索引/轮询状态并再次检查范围。
    let routing_config = request_auth.routing_config(&state.config);
    let plan = resolve_dispatch_plan_with_request(
        &routing_config,
        method,
        &path,
        headers,
        query.as_deref(),
    )
    .map_err(|message| {
        let status = if openai::is_openai_responses_compact_path(&path) {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::BAD_GATEWAY
        };
        log_request_error(
            &state.log,
            request_detail.clone(),
            client_ip.clone(),
            &path,
            PROVIDER_PROXY,
            LOCAL_UPSTREAM_ID,
            status,
            message.clone(),
            request_start,
        );
        http::error_response(status, message)
    })?;
    Ok(InboundRequest {
        request_auth,
        path,
        client_ip,
        plan,
        meta,
        request_detail,
        body,
    })
}

pub(super) async fn ensure_local_auth_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    method: &Method,
    body: Body,
    capture_request_detail_enabled: bool,
    client_ip: Option<String>,
    path: &str,
    query: Option<&str>,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<(Body, Option<http::LocalAccess>), Response> {
    let access = match http::ensure_local_auth(config, headers, method, path, query) {
        Ok(access) => access,
        Err(message) => {
            tracing::warn!("local auth failed");
            let detail = if capture_request_detail_enabled {
                Some(capture_detail_from_body(headers, body, max_body_bytes).await)
            } else {
                None
            };
            log_request_error(
                log,
                detail,
                client_ip,
                path,
                PROVIDER_PROXY,
                LOCAL_UPSTREAM_ID,
                StatusCode::UNAUTHORIZED,
                message.clone(),
                request_start,
            );
            return Err(http::error_response(StatusCode::UNAUTHORIZED, message));
        }
    };
    Ok((body, access))
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

pub(super) fn log_request_error(
    log: &Arc<LogWriter>,
    detail: Option<RequestDetailSnapshot>,
    client_ip: Option<String>,
    path: &str,
    provider: &str,
    upstream_id: &str,
    status: StatusCode,
    response_error: String,
    start: Instant,
) {
    let (request_headers, request_body) = detail
        .map(|detail| (detail.request_headers, detail.request_body))
        .unwrap_or((None, None));
    let context = LogContext {
        client_ip,
        path: path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        account_id: None,
        model: None,
        mapped_model: None,
        stream: false,
        status: status.as_u16(),
        upstream_request_id: None,
        request_headers,
        request_body,
        client_request_body: None,
        ttfb_ms: None,
        timings: Default::default(),
        start,
    };
    let usage = UsageSnapshot::default();
    let entry = build_log_entry(&context, usage, Some(response_error));
    log.clone().write_detached(entry);
}

async fn read_body_or_respond(
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
    client_ip: Option<String>,
    path: &str,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match ReplayableBody::from_body(body).await {
        Ok(body) => Ok(body),
        Err(err) => {
            let message = format!("Failed to read request body: {err}");
            let detail = if capture_request_detail_enabled {
                Some(RequestDetailSnapshot {
                    request_headers: serialize_request_headers(headers),
                    request_body: Some(message.clone()),
                })
            } else {
                None
            };
            log_request_error(
                log,
                detail,
                client_ip,
                path,
                PROVIDER_PROXY,
                LOCAL_UPSTREAM_ID,
                StatusCode::BAD_REQUEST,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::BAD_REQUEST, message))
        }
    }
}
