use axum::{
    http::{HeaderMap, Method},
    response::Response,
};
use std::sync::Arc;

const GEMINI_API_KEY_QUERY: &str = "key";

mod attempt;
mod catalog;
mod dispatch;
mod kiro;
mod kiro_headers;
mod kiro_http;
mod prepare;
mod request;
mod result;
mod retry;
mod transport;
mod utils;

use dispatch::run_upstream_groups;
#[cfg(test)]
use prepare::normalize_mapped_model_reasoning_suffix;
use prepare::{
    build_mapped_meta, ordered_runtime_account_ids, prepare_upstream_request,
    requested_target_upstream_id,
};
pub(super) use result::ForwardUpstreamResult;
use result::{finalize_forward_result, resolve_provider_upstreams};

#[cfg(test)]
use crate::proxy::redact::redact_query_param_value;

use super::{
    config::InboundApiFormat, http, http::RequestAuth, inbound::detect_inbound_api_format,
    openai_compat::FormatTransform, request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot, ProxyState, RequestMeta,
};

const REQUEST_MODEL_MAPPING_LIMIT_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn aggregate_model_catalog_request(
    state: Arc<ProxyState>,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    request_auth: &RequestAuth,
) -> Response {
    catalog::aggregate_model_catalog_request(
        state,
        provider,
        inbound_path,
        upstream_path_with_query,
        headers,
        request_auth,
    )
    .await
}

pub(super) async fn forward_upstream_request(
    state: Arc<ProxyState>,
    method: Method,
    provider: &str,
    inbound_path: &str,
    dispatch_inbound_format: Option<InboundApiFormat>,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<String>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> ForwardUpstreamResult {
    let inbound_format =
        dispatch_inbound_format.or_else(|| detect_inbound_api_format(inbound_path));
    let upstreams = match resolve_provider_upstreams(
        &state,
        provider,
        inbound_path,
        meta,
        request_detail.as_ref(),
    ) {
        Ok(upstreams) => upstreams,
        Err(response) => {
            return ForwardUpstreamResult {
                response,
                // Treat missing upstream config as retryable for higher-level fallback (e.g. cross-provider).
                should_fallback: true,
            };
        }
    };
    let summary = run_upstream_groups(
        &state,
        method,
        provider,
        inbound_format,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        client_gemini_api_key.as_deref(),
        response_transform,
        request_detail.clone(),
        upstreams,
    )
    .await;
    finalize_forward_result(
        &state,
        provider,
        inbound_path,
        meta,
        request_detail.as_ref(),
        summary,
    )
}

enum AttemptOutcome {
    Success(Response),
    Retryable {
        message: String,
        response: Option<Response>,
        is_timeout: bool,
        should_cooldown: bool,
    },
    Fatal(Response),
    SkippedAuth,
}

pub(super) struct PreparedUpstreamRequest {
    upstream_path_with_query: String,
    upstream_url: String,
    request_headers: HeaderMap,
    proxy_url: Option<String>,
    selected_account_id: Option<String>,
    meta: RequestMeta,
}

struct ResolvedUpstreamAuth {
    upstream_url: String,
    auth: http::UpstreamAuthHeader,
    extra_headers: Option<HeaderMap>,
    proxy_url: Option<String>,
    selected_account_id: Option<String>,
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "upstream.test.rs"]
mod tests;
