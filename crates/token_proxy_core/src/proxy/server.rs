use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Uri},
    response::Response,
};
use std::{sync::Arc, time::Instant};
use tokio::sync::RwLock;

use super::{
    server_helpers::extract_request_path, upstream::aggregate_model_catalog_request, ProxyState,
    RequestMeta,
};

const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_KIRO: &str = "kiro";
const PROVIDER_CODEX: &str = "codex";
const PROVIDER_PROXY: &str = "proxy";
const LOCAL_UPSTREAM_ID: &str = "local";
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
use dispatch::{
    is_openai_compatible_models_index_path, is_openai_models_index_path, resolve_outbound_path,
    DispatchPlan,
};
#[cfg(test)]
use dispatch::{
    resolve_dispatch_plan, resolve_dispatch_plan_with_request, resolve_retry_fallback_plan,
};
use execute::is_debug_log_enabled;
use fallback::forward_with_provider_fallbacks;
use inbound::{ensure_local_auth_or_respond, prepare_inbound_request, resolve_plan_or_respond};
use prepared::{
    build_outbound_path_with_query, finalize_prepared_request, resolve_request_auth_or_respond,
};

const ERROR_NO_UPSTREAM: &str = "No available upstream configured.";

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
    let capture_request_detail_enabled = state.request_detail.should_capture();
    let is_debug_log = is_debug_log_enabled(&state);
    let (path, _) = extract_request_path(&uri);
    let query = uri.query().map(|value| value.to_string());
    tracing::info!(method = %method, path = %path, "incoming request");
    tracing::debug!(headers = ?headers.keys().collect::<Vec<_>>(), "request headers");

    if method == Method::GET
        && (is_openai_models_index_path(&path) || is_openai_compatible_models_index_path(&path))
    {
        let body = match ensure_local_auth_or_respond(
            &state.config,
            &state.log,
            &headers,
            body,
            capture_request_detail_enabled,
            &path,
            query.as_deref(),
            request_start,
            state.config.max_request_body_bytes,
        )
        .await
        {
            Ok(body) => body,
            Err(response) => return response,
        };
        let (plan, _body) = match resolve_plan_or_respond(
            &state.config,
            &state.log,
            &headers,
            body,
            capture_request_detail_enabled,
            &path,
            query.as_deref(),
            request_start,
            state.config.max_request_body_bytes,
        )
        .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        let request_auth = match resolve_request_auth_or_respond(
            &state.config,
            &headers,
            &state.log,
            None,
            &path,
            plan.provider,
            request_start,
        ) {
            Ok(request_auth) => request_auth,
            Err(response) => return response,
        };
        let meta = RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        };
        let outbound_path = resolve_outbound_path(&path, &plan, &meta);
        let outbound_path_with_query = build_outbound_path_with_query(&outbound_path, &uri);
        return aggregate_model_catalog_request(
            state,
            plan.provider,
            &path,
            &outbound_path_with_query,
            &headers,
            &request_auth,
        )
        .await;
    }

    let inbound = match prepare_inbound_request(
        &state,
        &headers,
        path,
        query,
        body,
        capture_request_detail_enabled,
        request_start,
        is_debug_log,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(response) => return response,
    };
    let prepared =
        match finalize_prepared_request(&state, &headers, &uri, inbound, request_start).await {
            Ok(prepared) => prepared,
            Err(response) => return response,
        };
    forward_with_provider_fallbacks(state, method, &uri, &headers, &prepared, request_start).await
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "server.test.rs"]
mod tests;
