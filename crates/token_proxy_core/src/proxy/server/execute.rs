use axum::{
    http::{HeaderMap, Method, Uri},
    response::Response,
};
use std::{sync::Arc, time::Instant};

use super::super::{inbound::detect_inbound_api_format, upstream::forward_upstream_request};
use super::{
    prepared::{build_outbound_body_or_respond, build_outbound_path_with_query, PreparedRequest},
    resolve_outbound_path, DispatchPlan, ProxyState,
};
use crate::logging::LogLevel;

pub(super) async fn forward_retry_fallback_request(
    state: Arc<ProxyState>,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
    plan: &DispatchPlan,
) -> Result<super::super::upstream::ForwardUpstreamResult, Response> {
    let outbound_path = resolve_outbound_path(&prepared.path, plan, &prepared.meta);
    let dispatch_inbound_format = detect_inbound_api_format(&outbound_path);
    let outbound_path_with_query = build_outbound_path_with_query(&outbound_path, uri);
    let outbound_body = build_outbound_body_or_respond(
        &state.http_clients,
        &state.log,
        prepared.request_detail.clone(),
        &prepared.path,
        plan,
        &prepared.meta,
        prepared.source_body.clone(),
        request_start,
    )
    .await?;
    Ok(forward_upstream_request(
        state,
        method,
        plan.provider,
        &prepared.path,
        dispatch_inbound_format,
        &outbound_path_with_query,
        headers,
        &outbound_body,
        &prepared.meta,
        &prepared.request_auth,
        prepared.client_gemini_api_key.clone(),
        plan.response_transform,
        prepared.request_detail.clone(),
    )
    .await)
}

pub(super) fn is_debug_log_enabled(state: &ProxyState) -> bool {
    cfg!(debug_assertions) && matches!(state.config.log_level, LogLevel::Debug | LogLevel::Trace)
}
