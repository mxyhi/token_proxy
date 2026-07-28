use axum::{
    body::Body,
    extract::{connect_info::ConnectInfo, State},
    http::{HeaderMap, Method, Uri},
    response::Response,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Instant,
};
use tokio::sync::RwLock;

use super::{
    codex_models_manifest, http, server_helpers::extract_request_path,
    upstream::aggregate_all_providers_model_catalog, ProxyState,
};
// 测试模块 `use super::*` 需要可见；生产路径不直接使用。
#[cfg(test)]
use super::RequestMeta;

const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_KIRO: &str = "kiro";
const PROVIDER_CODEX: &str = "codex";
const PROVIDER_XAI: &str = "xai";
const PROVIDER_PROXY: &str = "proxy";
const LOCAL_UPSTREAM_ID: &str = "local";
const LOCALHOST_CLIENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const CODEX_RESPONSES_PATH: &str = "/responses";
const CODEX_RESPONSES_COMPACT_PATH: &str = "/responses/compact";

type ProxyStateHandle = Arc<RwLock<Arc<ProxyState>>>;

mod bootstrap;
mod dispatch;
mod execute;
mod fallback;
mod inbound;
mod prepared;
#[cfg(test)]
use super::openai_compat::{
    FormatTransform, CHAT_PATH, PROVIDER_CHAT, PROVIDER_RESPONSES, RESPONSES_PATH,
};
pub(crate) use bootstrap::{build_router, build_upstream_cursors};
use dispatch::{is_openai_compatible_models_index_path, is_openai_models_index_path};
#[cfg(test)]
use dispatch::{
    resolve_dispatch_plan, resolve_dispatch_plan_with_request, resolve_outbound_path,
    resolve_retry_fallback_plan, DispatchPlan,
};
use execute::is_debug_log_enabled;
use fallback::forward_with_provider_fallbacks;
use inbound::{ensure_local_auth_or_respond, prepare_inbound_request};
use prepared::{finalize_prepared_request, resolve_request_auth_or_respond};
#[cfg(test)]
use prepared::build_outbound_path_with_query;

const ERROR_NO_UPSTREAM: &str = "No available upstream configured.";

pub(super) async fn refresh_model_discovery(state: Arc<ProxyState>) {
    super::upstream::refresh_model_discovery(state).await;
}

#[cfg(test)]
// 测试直接调用 handler；真实 router 走 ConnectInfo 版本以记录客户端 IP。
async fn proxy_request(
    State(state): State<ProxyStateHandle>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    proxy_request_inner(state, method, uri, headers, body, None).await
}

async fn proxy_request_with_connect_info(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<ProxyStateHandle>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    proxy_request_inner(
        state,
        method,
        uri,
        headers,
        body,
        client_ip_for_request_log(addr),
    )
    .await
}

fn client_ip_for_request_log(addr: SocketAddr) -> Option<String> {
    let ip = addr.ip();
    if ip == LOCALHOST_CLIENT_IP {
        tracing::debug!(client_ip = %ip, "skip localhost client ip storage");
        return None;
    }
    Some(ip.to_string())
}

async fn proxy_request_inner(
    state: ProxyStateHandle,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
    client_ip: Option<String>,
) -> Response {
    // 只在此处短暂持有读锁，避免影响并发请求性能。
    let state = { state.read().await.clone() };
    let request_start = Instant::now();
    let capture_request_detail_enabled = state.request_detail.should_capture();
    let is_debug_log = is_debug_log_enabled(&state);
    let (path, _) = extract_request_path(&uri);
    let query = uri.query().map(|value| value.to_string());
    tracing::info!(
        method = %method,
        path = %path,
        client_ip = client_ip.as_deref().unwrap_or(""),
        "incoming request"
    );
    tracing::debug!(headers = ?headers.keys().collect::<Vec<_>>(), "request headers");

    if let Some(response) = http::cors_preflight_response(&state.config, &headers, &method) {
        return response;
    }

    let is_codex_models_manifest =
        codex_models_manifest::is_request(&method, &path, query.as_deref());
    // OpenAI 兼容模型索引：跨全部 enabled upstream 并集，不按 priority 单选 provider。
    // Codex 带 client_version 的 manifest 仍走后续转发路径。
    if method == Method::GET
        && !is_codex_models_manifest
        && (is_openai_models_index_path(&path) || is_openai_compatible_models_index_path(&path))
    {
        let _body = match ensure_local_auth_or_respond(
            &state.config,
            &state.log,
            &headers,
            &method,
            body,
            capture_request_detail_enabled,
            client_ip.clone(),
            &path,
            query.as_deref(),
            request_start,
            state.config.max_request_body_bytes,
        )
        .await
        {
            Ok(body) => body,
            Err(response) => return http::with_cors_headers(&state.config, &headers, response),
        };
        let request_auth = match resolve_request_auth_or_respond(
            &state.config,
            &headers,
            &state.log,
            None,
            client_ip.clone(),
            &path,
            "models",
            request_start,
        ) {
            Ok(request_auth) => request_auth,
            Err(response) => return http::with_cors_headers(&state.config, &headers, response),
        };
        tracing::debug!(path = %path, "serving aggregated multi-provider model catalog");
        let response =
            aggregate_all_providers_model_catalog(state.clone(), &headers, &request_auth).await;
        return http::with_cors_headers(&state.config, &headers, response);
    }

    let inbound = match prepare_inbound_request(
        &state,
        &headers,
        &method,
        path,
        query,
        body,
        capture_request_detail_enabled,
        client_ip,
        request_start,
        is_debug_log,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(response) => return http::with_cors_headers(&state.config, &headers, response),
    };
    let prepared =
        match finalize_prepared_request(&state, &headers, &uri, inbound, request_start).await {
            Ok(prepared) => prepared,
            Err(response) => return http::with_cors_headers(&state.config, &headers, response),
        };
    let response = forward_with_provider_fallbacks(
        state.clone(),
        method,
        &uri,
        &headers,
        &prepared,
        request_start,
    )
    .await;
    http::with_cors_headers(&state.config, &headers, response)
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
