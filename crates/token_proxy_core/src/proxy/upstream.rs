use axum::{
    http::{
        header::{AUTHORIZATION, USER_AGENT},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    response::Response,
};
use crate::antigravity::endpoints as antigravity_endpoints;
use crate::antigravity::project as antigravity_project;
use std::{
    sync::{
        Arc,
    },
    time::Instant,
};

const GEMINI_API_KEY_QUERY: &str = "key";
const LOCAL_UPSTREAM_ID: &str = "local";
const ANTIGRAVITY_GENERATE_PATH: &str = "/v1internal:generateContent";
const ANTIGRAVITY_STREAM_PATH: &str = "/v1internal:streamGenerateContent";

mod request;
mod attempt;
mod result;
mod utils;
mod kiro;
mod kiro_headers;
mod kiro_http;

use utils::{
    build_group_order, resolve_group_start,
};

#[cfg(test)]
use crate::proxy::redact::redact_query_param_value;

use super::{
    config::{ProviderUpstreams, UpstreamRuntime},
    gemini,
    http,
    http::RequestAuth,
    openai_compat::FormatTransform,
    request_detail::RequestDetailSnapshot,
    request_body::ReplayableBody,
    ProxyState,
    RequestMeta,
};

const REQUEST_MODEL_MAPPING_LIMIT_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct ForwardUpstreamResult {
    pub(super) response: Response,
    pub(super) should_fallback: bool,
}

pub(super) async fn forward_upstream_request(
    state: Arc<ProxyState>,
    method: Method,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> ForwardUpstreamResult {
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
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        response_transform,
        request_detail.clone(),
        upstreams,
    )
    .await;
    if let Some(response) = summary.response {
        return ForwardUpstreamResult {
            response,
            should_fallback: false,
        };
    }
    let should_fallback = summary.last_retry_response.is_some()
        || summary.last_timeout_error.is_some()
        || summary.last_retry_error.is_some()
        || (summary.attempted == 0 && summary.missing_auth);
    let response = finalize_forward_response(
        &state,
        provider,
        inbound_path,
        meta,
        request_detail.as_ref(),
        summary,
    );
    ForwardUpstreamResult {
        response,
        should_fallback,
    }
}

struct GroupAttemptResult {
    response: Option<Response>,
    attempted: usize,
    missing_auth: bool,
    last_timeout_error: Option<String>,
    last_retry_error: Option<String>,
    last_retry_response: Option<Response>,
}

impl GroupAttemptResult {
    fn new() -> Self {
        Self {
            response: None,
            attempted: 0,
            missing_auth: false,
            last_timeout_error: None,
            last_retry_error: None,
            last_retry_response: None,
        }
    }
}

struct ForwardAttemptState {
    response: Option<Response>,
    attempted: usize,
    missing_auth: bool,
    last_timeout_error: Option<String>,
    last_retry_error: Option<String>,
    last_retry_response: Option<Response>,
}

impl ForwardAttemptState {
    fn new() -> Self {
        Self {
            response: None,
            attempted: 0,
            missing_auth: false,
            last_timeout_error: None,
            last_retry_error: None,
            last_retry_response: None,
        }
    }
}

enum AttemptOutcome {
    Success(Response),
    Retryable {
        message: String,
        response: Option<Response>,
        is_timeout: bool,
    },
    Fatal(Response),
    SkippedAuth,
}

fn apply_attempt_outcome(
    result: &mut GroupAttemptResult,
    outcome: AttemptOutcome,
) -> bool {
    match outcome {
        AttemptOutcome::Success(response) | AttemptOutcome::Fatal(response) => {
            result.response = Some(response);
            true
        }
        AttemptOutcome::Retryable {
            message,
            response,
            is_timeout,
        } => {
            if is_timeout {
                result.last_timeout_error = Some(message.clone());
            } else {
                result.last_retry_error = Some(message.clone());
            }
            if response.is_some() {
                result.last_retry_response = response;
            }
            false
        }
        AttemptOutcome::SkippedAuth => {
            result.missing_auth = true;
            false
        }
    }
}

fn merge_group_result(state: &mut ForwardAttemptState, result: GroupAttemptResult) -> bool {
    state.attempted += result.attempted;
    state.missing_auth |= result.missing_auth;
    if let Some(response) = result.response {
        state.response = Some(response);
        return true;
    }
    if result.last_timeout_error.is_some() {
        state.last_timeout_error = result.last_timeout_error;
    }
    if result.last_retry_error.is_some() {
        state.last_retry_error = result.last_retry_error;
    }
    if let Some(response) = result.last_retry_response {
        state.last_retry_response = Some(response);
    }
    false
}

pub(super) struct PreparedUpstreamRequest {
    upstream_path_with_query: String,
    upstream_url: String,
    request_headers: HeaderMap,
    meta: RequestMeta,
    antigravity: Option<AntigravityRequestInfo>,
}

struct ResolvedUpstreamAuth {
    upstream_url: String,
    auth: http::UpstreamAuthHeader,
    extra_headers: Option<HeaderMap>,
    antigravity: Option<AntigravityRequestInfo>,
}

#[derive(Clone)]
pub(super) struct AntigravityRequestInfo {
    project_id: Option<String>,
    user_agent: String,
}

fn resolve_provider_upstreams<'a>(
    state: &'a ProxyState,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
) -> Result<&'a ProviderUpstreams, Response> {
    match state.config.provider_upstreams(provider) {
        Some(upstreams) => Ok(upstreams),
        None => {
            result::log_upstream_error_if_needed(
                &state.log,
                request_detail,
                meta,
                provider,
                LOCAL_UPSTREAM_ID,
                inbound_path,
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.".to_string(),
                Instant::now(),
            );
            Err(http::error_response(
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.",
            ))
        }
    }
}

async fn run_upstream_groups(
    state: &ProxyState,
    method: Method,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    upstreams: &ProviderUpstreams,
) -> ForwardAttemptState {
    let mut summary = ForwardAttemptState::new();
    for (group_index, group) in upstreams.groups.iter().enumerate() {
        // Only rotate within the highest priority group; retry network failures before degrading.
        if group.items.is_empty() {
            continue;
        }
        let result = try_group_upstreams(
            state,
            method.clone(),
            provider,
            group_index,
            &group.items,
            inbound_path,
            upstream_path_with_query,
            headers,
            body,
            meta,
            request_auth,
            response_transform,
            request_detail.clone(),
        )
        .await;
        if merge_group_result(&mut summary, result) {
            break;
        }
    }
    summary
}

fn finalize_forward_response(
    state: &ProxyState,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
    summary: ForwardAttemptState,
) -> Response {
    if summary.attempted == 0 && summary.missing_auth {
        result::log_upstream_error_if_needed(
            &state.log,
            request_detail,
            meta,
            provider,
            LOCAL_UPSTREAM_ID,
            inbound_path,
            StatusCode::UNAUTHORIZED,
            "Missing upstream API key.".to_string(),
            Instant::now(),
        );
        return http::error_response(StatusCode::UNAUTHORIZED, "Missing upstream API key.");
    }
    if let Some(response) = summary.last_retry_response {
        return response;
    }
    if let Some(err) = summary.last_timeout_error {
        return http::error_response(StatusCode::GATEWAY_TIMEOUT, err);
    }
    if let Some(err) = summary.last_retry_error {
        return http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Upstream request failed: {err}"),
        );
    }
    http::error_response(
        StatusCode::BAD_GATEWAY,
        "No available upstream configured.",
    )
}

async fn try_group_upstreams(
    state: &ProxyState,
    method: Method,
    provider: &str,
    group_index: usize,
    items: &[UpstreamRuntime],
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> GroupAttemptResult {
    let mut result = GroupAttemptResult::new();
    let start = resolve_group_start(state, provider, group_index, items.len());
    for item_index in build_group_order(items.len(), start) {
        let upstream = &items[item_index];
        let outcome = attempt::attempt_upstream(
            state,
            method.clone(),
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            headers,
            body,
            meta,
            request_auth,
            response_transform,
            request_detail.clone(),
        )
        .await;
        if !matches!(outcome, AttemptOutcome::SkippedAuth) {
            result.attempted += 1;
        }
        if apply_attempt_outcome(&mut result, outcome) {
            return result;
        }
    }
    result
}

async fn prepare_upstream_request(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
) -> Result<PreparedUpstreamRequest, AttemptOutcome> {
    let mapped_meta = build_mapped_meta(meta, upstream);
    let upstream_path_with_query =
        resolve_upstream_path_with_query(provider, upstream_path_with_query, &mapped_meta);
    let upstream_url = upstream.upstream_url(&upstream_path_with_query);
    let resolved = resolve_upstream_auth(
        state,
        provider,
        upstream,
        request_auth,
        &upstream_path_with_query,
        &upstream_url,
    )
    .await?;
    let ResolvedUpstreamAuth {
        upstream_url,
        auth,
        extra_headers,
        antigravity,
    } = resolved;
    let request_headers = request::build_request_headers(
        provider,
        headers,
        auth,
        extra_headers.as_ref(),
        upstream.header_overrides.as_deref(),
    );
    Ok(PreparedUpstreamRequest {
        upstream_path_with_query,
        upstream_url,
        request_headers,
        meta: mapped_meta,
        antigravity,
    })
}

async fn resolve_upstream_auth(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    request_auth: &RequestAuth,
    upstream_path_with_query: &str,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    if provider == "gemini" {
        let (upstream_url, auth) = request::resolve_gemini_upstream(
            upstream,
            request_auth,
            upstream_path_with_query,
            upstream_url,
        )?;
        return Ok(ResolvedUpstreamAuth {
            upstream_url,
            auth,
            extra_headers: None,
            antigravity: None,
        });
    }
    if provider == "kiro" {
        return resolve_kiro_upstream(state, upstream, upstream_url).await;
    }
    if provider == "codex" {
        return resolve_codex_upstream(state, upstream, upstream_url).await;
    }
    if provider == "antigravity" {
        return resolve_antigravity_upstream(state, upstream, upstream_url).await;
    }
    let auth = match http::resolve_upstream_auth(provider, upstream, request_auth) {
        Ok(Some(auth)) => auth,
        Ok(None) => return Err(AttemptOutcome::SkippedAuth),
        Err(response) => return Err(AttemptOutcome::Fatal(response)),
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth,
        extra_headers: None,
        antigravity: None,
    })
}

async fn resolve_kiro_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let Some(account_id) = upstream.kiro_account_id.as_deref() else {
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Kiro account is not configured.",
        )));
    };
    let token = state
        .kiro_accounts
        .get_access_token(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))?;
    let value = http::bearer_header(&token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers: None,
        antigravity: None,
    })
}

async fn resolve_codex_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let Some(account_id) = upstream.codex_account_id.as_deref() else {
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Codex account is not configured.",
        )));
    };
    let record = state
        .codex_accounts
        .get_account_record(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))?;
    let value = http::bearer_header(&record.access_token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    let mut extra_headers = HeaderMap::new();
    if let Some(account_id) = record.account_id.as_deref() {
        if let Ok(value) = axum::http::HeaderValue::from_str(account_id) {
            extra_headers.insert(axum::http::HeaderName::from_static("chatgpt-account-id"), value);
        }
    }
    let extra_headers = if extra_headers.is_empty() {
        None
    } else {
        Some(extra_headers)
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers,
        antigravity: None,
    })
}

async fn resolve_antigravity_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let Some(account_id) = upstream.antigravity_account_id.as_deref() else {
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Antigravity account is not configured.",
        )));
    };
    let mut record = state
        .antigravity_accounts
        .get_account_record(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))?;
    if record
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        let proxy_url = state.antigravity_accounts.app_proxy_url().await;
        match antigravity_project::load_code_assist(&record.access_token, proxy_url.as_deref()).await
        {
            Ok(info) => {
                if let Some(value) = info.project_id.clone() {
                    let _ = state
                        .antigravity_accounts
                        .update_project_id(account_id, value.clone())
                        .await;
                    record.project_id = Some(value);
                } else if let Some(tier_id) = info.plan_type.as_deref() {
                    if let Ok(Some(value)) = antigravity_project::onboard_user(
                        &record.access_token,
                        proxy_url.as_deref(),
                        tier_id,
                    )
                    .await
                    {
                        let _ = state
                            .antigravity_accounts
                            .update_project_id(account_id, value.clone())
                            .await;
                        record.project_id = Some(value);
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "antigravity loadCodeAssist failed in proxy");
            }
        }
    }
    let value = http::bearer_header(&record.access_token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    let user_agent = state
        .config
        .antigravity_user_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(antigravity_endpoints::default_user_agent);
    let mut extra_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&user_agent) {
        extra_headers.insert(USER_AGENT, value);
    }
    let extra_headers = if extra_headers.is_empty() {
        None
    } else {
        Some(extra_headers)
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers,
        antigravity: Some(AntigravityRequestInfo {
            project_id: record.project_id.clone(),
            user_agent,
        }),
    })
}

fn build_mapped_meta(meta: &RequestMeta, upstream: &UpstreamRuntime) -> RequestMeta {
    let mapped_model = meta
        .original_model
        .as_deref()
        .map(|original| upstream.map_model(original).unwrap_or_else(|| original.to_string()));
    let (mapped_model, reasoning_effort) = normalize_mapped_model_reasoning_suffix(
        mapped_model,
        meta.reasoning_effort.clone(),
    );
    RequestMeta {
        stream: meta.stream,
        original_model: meta.original_model.clone(),
        mapped_model,
        reasoning_effort,
        estimated_input_tokens: meta.estimated_input_tokens,
    }
}

fn normalize_mapped_model_reasoning_suffix(
    mapped_model: Option<String>,
    reasoning_effort: Option<String>,
) -> (Option<String>, Option<String>) {
    let Some(mapped_model) = mapped_model else {
        return (None, reasoning_effort);
    };
    let Some((base_model, mapped_effort)) =
        super::server_helpers::parse_openai_reasoning_effort_from_model_suffix(&mapped_model)
    else {
        return (Some(mapped_model), reasoning_effort);
    };

    // If the user already specified an explicit effort in the incoming `model`, keep it.
    let reasoning_effort = reasoning_effort.or(Some(mapped_effort));
    (Some(base_model), reasoning_effort)
}

fn resolve_upstream_path_with_query(
    provider: &str,
    upstream_path_with_query: &str,
    meta: &RequestMeta,
) -> String {
    if provider == "antigravity" {
        return if meta.stream {
            // Align with CLIProxyAPIPlus: Antigravity streaming defaults to SSE via `alt=sse`.
            format!("{ANTIGRAVITY_STREAM_PATH}?alt=sse")
        } else {
            ANTIGRAVITY_GENERATE_PATH.to_string()
        };
    }
    if provider != "gemini" || meta.model_override().is_none() {
        return upstream_path_with_query.to_string();
    }
    let Some(mapped_model) = meta.mapped_model.as_deref() else {
        return upstream_path_with_query.to_string();
    };
    let (path, query) = request::split_path_query(upstream_path_with_query);
    let replaced = gemini::replace_gemini_model_in_path(path, mapped_model)
        .unwrap_or_else(|| path.to_string());
    match query {
        Some(query) => format!("{replaced}?{query}"),
        None => replaced,
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "upstream.test.rs"]
mod tests;
