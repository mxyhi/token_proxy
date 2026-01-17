use axum::{
    http::{
        HeaderMap, Method, StatusCode,
    },
    response::Response,
};
use std::{
    sync::{
        Arc,
    },
    time::Instant,
};
use tokio::time::timeout;

const GEMINI_API_KEY_QUERY: &str = "key";
const LOCAL_UPSTREAM_ID: &str = "local";

mod request;
mod utils;

use utils::{
    build_group_order, is_retryable_error, is_retryable_status, resolve_group_start,
    sanitize_upstream_error,
};

#[cfg(test)]
use crate::proxy::redact::redact_query_param_value;

use super::{
    config::UpstreamRuntime,
    gemini,
    http,
    http::RequestAuth,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai_compat::FormatTransform,
    request_detail::RequestDetailSnapshot,
    request_body::ReplayableBody,
    response::{build_proxy_response, build_proxy_response_buffered},
    UPSTREAM_NO_DATA_TIMEOUT,
    ProxyState,
    RequestMeta,
};

const REQUEST_MODEL_MAPPING_LIMIT_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn forward_upstream_request(
    state: Arc<ProxyState>,
    method: Method,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: HeaderMap,
    body: ReplayableBody,
    meta: RequestMeta,
    request_auth: RequestAuth,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> Response {
    let mut last_retry_error: Option<String> = None;
    let mut last_retry_response: Option<Response> = None;
    let mut last_timeout_error: Option<String> = None;
    let mut attempted = 0;
    let mut missing_auth = false;

    let upstreams = match state.config.provider_upstreams(provider) {
        Some(upstreams) => upstreams,
        None => {
            log_upstream_error_if_needed(
                &state.log,
                request_detail.as_ref(),
                &meta,
                provider,
                LOCAL_UPSTREAM_ID,
                inbound_path,
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.".to_string(),
                Instant::now(),
            );
            return http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured.");
        }
    };

    for (group_index, group) in upstreams.groups.iter().enumerate() {
        // Only rotate within the highest priority group; retry network failures before degrading.
        if group.items.is_empty() {
            continue;
        }
        let result = try_group_upstreams(
            &state,
            method.clone(),
            provider,
            group_index,
            &group.items,
            inbound_path,
            upstream_path_with_query,
            &headers,
            &body,
            &meta,
            &request_auth,
            response_transform,
            request_detail.clone(),
        )
        .await;
        attempted += result.attempted;
        missing_auth |= result.missing_auth;
        if let Some(response) = result.response {
            return response;
        }
        if let Some(response) = result.last_retry_response {
            last_retry_response = Some(response);
        }
        if result.last_timeout_error.is_some() {
            last_timeout_error = result.last_timeout_error;
        }
        if result.last_retry_error.is_some() {
            last_retry_error = result.last_retry_error;
        }
    }

    if attempted == 0 && missing_auth {
        log_upstream_error_if_needed(
            &state.log,
            request_detail.as_ref(),
            &meta,
            provider,
            LOCAL_UPSTREAM_ID,
            inbound_path,
            StatusCode::UNAUTHORIZED,
            "Missing upstream API key.".to_string(),
            Instant::now(),
        );
        return http::error_response(StatusCode::UNAUTHORIZED, "Missing upstream API key.");
    }
    if let Some(response) = last_retry_response {
        return response;
    }
    if let Some(err) = last_timeout_error {
        return http::error_response(StatusCode::GATEWAY_TIMEOUT, err);
    }
    if let Some(err) = last_retry_error {
        return http::error_response(StatusCode::BAD_GATEWAY, format!("Upstream request failed: {err}"));
    }
    http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured.")
}

struct GroupAttemptResult {
    response: Option<Response>,
    attempted: usize,
    missing_auth: bool,
    last_timeout_error: Option<String>,
    last_retry_error: Option<String>,
    last_retry_response: Option<Response>,
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

struct PreparedUpstreamRequest {
    upstream_url: String,
    request_headers: HeaderMap,
    upstream_body: reqwest::Body,
    meta: RequestMeta,
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
    let mut last_timeout_error = None;
    let mut last_retry_error = None;
    let mut last_retry_response = None;
    let mut attempted = 0;
    let mut missing_auth = false;
    let start = resolve_group_start(state, provider, group_index, items.len());
    for item_index in build_group_order(items.len(), start) {
        let upstream = &items[item_index];
        let outcome = attempt_upstream(
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
            attempted += 1;
        }
        match outcome {
            AttemptOutcome::Success(response) | AttemptOutcome::Fatal(response) => {
                return GroupAttemptResult {
                    response: Some(response),
                    attempted,
                    missing_auth,
                    last_timeout_error,
                    last_retry_error,
                    last_retry_response,
                };
            }
            AttemptOutcome::Retryable { message, response, is_timeout } => {
                if is_timeout {
                    last_timeout_error = Some(message.clone());
                } else {
                    last_retry_error = Some(message.clone());
                }
                if response.is_some() {
                    last_retry_response = response;
                }
            }
            AttemptOutcome::SkippedAuth => {
                missing_auth = true;
            }
        }
    }
    GroupAttemptResult {
        response: None,
        attempted,
        missing_auth,
        last_timeout_error,
        last_retry_error,
        last_retry_response,
    }
}

async fn attempt_upstream(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> AttemptOutcome {
    let prepared = match prepare_upstream_request(
        provider,
        upstream,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let start_time = Instant::now();
    let client = match state
        .http_clients
        .client_for_proxy_url(upstream.proxy_url.as_deref())
    {
        Ok(client) => client,
        Err(message) => {
            return AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_GATEWAY, message))
        }
    };
    let upstream_res = timeout(
        UPSTREAM_NO_DATA_TIMEOUT,
        client
            .request(method, prepared.upstream_url)
            .headers(prepared.request_headers)
            .body(prepared.upstream_body)
            .send(),
    )
    .await;
    let upstream_res = match upstream_res {
        Ok(result) => result,
        Err(_) => {
            let message = format!(
                "Upstream did not respond within {}s.",
                UPSTREAM_NO_DATA_TIMEOUT.as_secs()
            );
            log_upstream_error_if_needed(
                &state.log,
                request_detail.as_ref(),
                meta,
                provider,
                &upstream.id,
                inbound_path,
                StatusCode::GATEWAY_TIMEOUT,
                message.clone(),
                start_time,
            );
            return AttemptOutcome::Retryable {
                message,
                response: None,
                is_timeout: true,
            };
        }
    };
    handle_upstream_result(
        upstream_res,
        &prepared.meta,
        provider,
        &upstream.id,
        inbound_path,
        state.log.clone(),
        state.token_rate.clone(),
        start_time,
        response_transform,
        request_detail,
    )
    .await
}

async fn prepare_upstream_request(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
) -> Result<PreparedUpstreamRequest, AttemptOutcome> {
    let mapped_meta = build_mapped_meta(meta, upstream);
    let upstream_path_with_query =
        resolve_upstream_path_with_query(provider, upstream_path_with_query, &mapped_meta);
    let upstream_url = upstream.upstream_url(&upstream_path_with_query);
    let (upstream_url, auth) = resolve_upstream_auth(
        provider,
        upstream,
        request_auth,
        &upstream_path_with_query,
        &upstream_url,
    )?;
    let request_headers = request::build_request_headers(
        provider,
        headers,
        auth,
        upstream.header_overrides.as_deref(),
    );
    let upstream_body =
        request::build_upstream_body(provider, &upstream_path_with_query, body, &mapped_meta).await?;
    Ok(PreparedUpstreamRequest {
        upstream_url,
        request_headers,
        upstream_body,
        meta: mapped_meta,
    })
}

fn resolve_upstream_auth(
    provider: &str,
    upstream: &UpstreamRuntime,
    request_auth: &RequestAuth,
    upstream_path_with_query: &str,
    upstream_url: &str,
) -> Result<(String, http::UpstreamAuthHeader), AttemptOutcome> {
    if provider == "gemini" {
        return request::resolve_gemini_upstream(
            upstream,
            request_auth,
            upstream_path_with_query,
            upstream_url,
        );
    }
    let auth = match http::resolve_upstream_auth(provider, upstream, request_auth) {
        Ok(Some(auth)) => auth,
        Ok(None) => return Err(AttemptOutcome::SkippedAuth),
        Err(response) => return Err(AttemptOutcome::Fatal(response)),
    };
    Ok((upstream_url.to_string(), auth))
}

async fn handle_upstream_result(
    upstream_res: Result<reqwest::Response, reqwest::Error>,
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    inbound_path: &str,
    log: Arc<LogWriter>,
    token_rate: Arc<super::token_rate::TokenRateTracker>,
    start_time: Instant,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> AttemptOutcome {
    match upstream_res {
        Ok(res) if is_retryable_status(res.status()) => {
            let response = build_proxy_response_buffered(
                meta,
                provider,
                upstream_id,
                inbound_path,
                res,
                log,
                token_rate,
                start_time,
                response_transform,
                request_detail.clone(),
            )
            .await;
            AttemptOutcome::Retryable {
                message: format!("Upstream responded with {}", response.status()),
                response: Some(response),
                is_timeout: false,
            }
        }
        Ok(res) => {
            let response = build_proxy_response(
                meta,
                provider,
                upstream_id,
                inbound_path,
                res,
                log,
                token_rate,
                start_time,
                response_transform,
                request_detail.clone(),
            )
            .await;
            AttemptOutcome::Success(response)
        }
        Err(err) if is_retryable_error(&err) => {
            let message = sanitize_upstream_error(provider, &err);
            let status = if err.is_timeout() {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::BAD_GATEWAY
            };
            log_upstream_error_if_needed(
                &log,
                request_detail.as_ref(),
                meta,
                provider,
                upstream_id,
                inbound_path,
                status,
                message.clone(),
                start_time,
            );
            AttemptOutcome::Retryable {
                message,
                response: None,
                is_timeout: err.is_timeout(),
            }
        }
        Err(err) => {
            let message = sanitize_upstream_error(provider, &err);
            log_upstream_error_if_needed(
                &log,
                request_detail.as_ref(),
                meta,
                provider,
                upstream_id,
                inbound_path,
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {message}"),
                start_time,
            );
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {message}"),
            ))
        }
    }
}

fn log_upstream_error_if_needed(
    log: &Arc<LogWriter>,
    request_detail: Option<&RequestDetailSnapshot>,
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    inbound_path: &str,
    status: StatusCode,
    response_error: String,
    start_time: Instant,
) {
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers.clone(), detail.request_body.clone()))
        .unwrap_or((None, None));
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: None,
        request_headers,
        request_body,
        ttfb_ms: None,
        start: start_time,
    };
    let usage = UsageSnapshot {
        usage: None,
        cached_tokens: None,
        usage_json: None,
    };
    let entry = build_log_entry(&context, usage, Some(response_error));
    log.clone().write_detached(entry);
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

#[cfg(test)]
mod tests;
