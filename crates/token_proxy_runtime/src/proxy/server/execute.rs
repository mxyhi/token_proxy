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
use crate::proxy::{
    config::InboundApiFormat, cooldown_scope::CooldownScope, http, openai_compat::FormatTransform,
    request_body::ReplayableBody,
};

pub(super) struct OutboundRequest {
    pub(super) path: String,
    pub(super) body: ReplayableBody,
}

/// 从原始入站 body 构造单个 Provider 的出站合同；供实际执行候选时惰性缓存。
pub(super) async fn prepare_dispatch_request(
    state: &ProxyState,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
    plan: &DispatchPlan,
) -> Result<OutboundRequest, Response> {
    let outbound_path =
        resolve_outbound_path(&prepared.path, plan, &prepared.meta).map_err(|message| {
            tracing::warn!(provider = plan.provider, "rejected unsafe outbound path");
            http::error_response(axum::http::StatusCode::BAD_REQUEST, message)
        })?;
    let body = build_outbound_body_or_respond(
        &state.http_clients,
        &state.log,
        prepared.request_detail.clone(),
        prepared.client_ip.clone(),
        &prepared.path,
        plan,
        &prepared.meta,
        headers,
        prepared.source_body.clone(),
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    Ok(OutboundRequest {
        path: build_outbound_path_with_query(&outbound_path, uri),
        body,
    })
}

pub(super) async fn forward_retry_fallback_request(
    state: Arc<ProxyState>,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
    plan: &DispatchPlan,
    codex_cooldown_scope: &CooldownScope,
) -> Result<super::super::upstream::ForwardUpstreamResult, Response> {
    let outbound =
        prepare_dispatch_request(&state, uri, headers, prepared, request_start, plan).await?;
    let dispatch_inbound_format = bridge_inbound_format(plan.request_transform)
        .or_else(|| detect_inbound_api_format(&outbound.path));
    Ok(forward_upstream_request(
        state,
        method,
        plan.provider,
        &prepared.path,
        dispatch_inbound_format,
        &outbound.path,
        headers,
        &outbound.body,
        &prepared.meta,
        &prepared.request_auth,
        prepared.client_gemini_api_key.clone(),
        plan.response_transform,
        prepared.request_detail.clone(),
        codex_cooldown_scope,
    )
    .await)
}

fn bridge_inbound_format(transform: FormatTransform) -> Option<InboundApiFormat> {
    match transform {
        FormatTransform::ImagesGenerationsToCodex => Some(InboundApiFormat::OpenaiResponses),
        _ => None,
    }
}

pub(super) fn is_debug_log_enabled(state: &ProxyState) -> bool {
    cfg!(debug_assertions) && matches!(state.config.log_level, LogLevel::Debug | LogLevel::Trace)
}
