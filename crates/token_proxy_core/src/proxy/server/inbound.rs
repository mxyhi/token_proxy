use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use std::{sync::Arc, time::Instant};

use super::super::{
    config::ProxyConfig,
    http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
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
    pub(crate) path: String,
    pub(crate) plan: DispatchPlan,
    pub(crate) body: ReplayableBody,
    pub(crate) meta: RequestMeta,
    pub(crate) request_detail: Option<RequestDetailSnapshot>,
}

pub(super) async fn prepare_inbound_request(
    state: &ProxyState,
    headers: &HeaderMap,
    path: String,
    query: Option<String>,
    body: Body,
    capture_request_detail_enabled: bool,
    request_start: Instant,
    is_debug_log: bool,
) -> Result<InboundRequest, Response> {
    let body = ensure_local_auth_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
        &path,
        query.as_deref(),
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let (plan, body) = resolve_plan_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
        &path,
        query.as_deref(),
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let body = read_body_or_respond(
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
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
    let meta = parse_request_meta_best_effort(&path_with_query, &body).await;
    let request_detail = if capture_request_detail_enabled {
        Some(capture_request_detail(headers, &body, state.config.max_request_body_bytes).await)
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

pub(super) async fn ensure_local_auth_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
    path: &str,
    query: Option<&str>,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<Body, Response> {
    if let Err(message) = http::ensure_local_auth(config, headers, path, query) {
        tracing::warn!("local auth failed");
        let detail = if capture_request_detail_enabled {
            Some(capture_detail_from_body(headers, body, max_body_bytes).await)
        } else {
            None
        };
        log_request_error(
            log,
            detail,
            path,
            PROVIDER_PROXY,
            LOCAL_UPSTREAM_ID,
            StatusCode::UNAUTHORIZED,
            message.clone(),
            request_start,
        );
        return Err(http::error_response(StatusCode::UNAUTHORIZED, message));
    }
    Ok(body)
}

pub(super) async fn resolve_plan_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
    path: &str,
    query: Option<&str>,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<(DispatchPlan, Body), Response> {
    match resolve_dispatch_plan_with_request(config, path, headers, query) {
        Ok(plan) => {
            tracing::debug!(provider = %plan.provider, "dispatch plan resolved");
            Ok((plan, body))
        }
        Err(message) => {
            tracing::warn!("no dispatch plan found");
            let detail = if capture_request_detail_enabled {
                Some(capture_detail_from_body(headers, body, max_body_bytes).await)
            } else {
                None
            };
            log_request_error(
                log,
                detail,
                path,
                PROVIDER_PROXY,
                LOCAL_UPSTREAM_ID,
                StatusCode::BAD_GATEWAY,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::BAD_GATEWAY, message))
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

pub(super) fn log_request_error(
    log: &Arc<LogWriter>,
    detail: Option<RequestDetailSnapshot>,
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
        ttfb_ms: None,
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

async fn read_body_or_respond(
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
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
