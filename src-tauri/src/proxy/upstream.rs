use axum::{
    http::{
        header::{HeaderName, HeaderValue, CONTENT_LENGTH, HOST},
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

const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const GEMINI_API_KEY_QUERY: &str = "key";
const GEMINI_API_KEY_HEADER: HeaderName = HeaderName::from_static("x-goog-api-key");

mod utils;

use utils::{
    build_group_order, ensure_query_param, extract_query_param, is_retryable_error,
    is_retryable_status, resolve_group_start, sanitize_upstream_error,
};

#[cfg(test)]
use utils::redact_query_param_value;

use super::{
    config::UpstreamRuntime,
    gemini,
    http,
    http::RequestAuth,
    log::LogWriter,
    model,
    openai_compat::FormatTransform,
    request_body::ReplayableBody,
    response::{build_proxy_response, build_proxy_response_buffered},
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
) -> Response {
    let mut last_retry_error: Option<String> = None;
    let mut last_retry_response: Option<Response> = None;
    let mut attempted = 0;
    let mut missing_auth = false;

    let upstreams = match state.config.provider_upstreams(provider) {
        Some(upstreams) => upstreams,
        None => return http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured."),
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
        if result.last_retry_error.is_some() {
            last_retry_error = result.last_retry_error;
        }
    }

    if attempted == 0 && missing_auth {
        return http::error_response(StatusCode::UNAUTHORIZED, "Missing upstream API key.");
    }
    if let Some(response) = last_retry_response {
        return response;
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
    last_retry_error: Option<String>,
    last_retry_response: Option<Response>,
}

enum AttemptOutcome {
    Success(Response),
    Retryable {
        message: String,
        response: Option<Response>,
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
) -> GroupAttemptResult {
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
                    last_retry_error,
                    last_retry_response,
                };
            }
            AttemptOutcome::Retryable { message, response } => {
                last_retry_error = Some(message);
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
    let upstream_res = client
        .request(method, prepared.upstream_url)
        .headers(prepared.request_headers)
        .body(prepared.upstream_body)
        .send()
        .await;
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
    let request_headers = build_request_headers(
        provider,
        headers,
        auth,
        upstream.header_overrides.as_deref(),
    );
    let upstream_body = build_upstream_body(body, &mapped_meta).await?;
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
        return resolve_gemini_upstream(upstream, request_auth, upstream_path_with_query, upstream_url);
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
            )
            .await;
            AttemptOutcome::Retryable {
                message: format!("Upstream responded with {}", response.status()),
                response: Some(response),
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
            )
            .await;
            AttemptOutcome::Success(response)
        }
        Err(err) if is_retryable_error(&err) => AttemptOutcome::Retryable {
            message: sanitize_upstream_error(provider, &err),
            response: None,
        },
        Err(err) => AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Upstream request failed: {}", sanitize_upstream_error(provider, &err)),
        )),
    }
}

fn build_mapped_meta(meta: &RequestMeta, upstream: &UpstreamRuntime) -> RequestMeta {
    let mapped_model = meta
        .original_model
        .as_deref()
        .map(|original| upstream.map_model(original).unwrap_or_else(|| original.to_string()));
    RequestMeta {
        stream: meta.stream,
        original_model: meta.original_model.clone(),
        mapped_model,
        estimated_input_tokens: meta.estimated_input_tokens,
    }
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
    let (path, query) = split_path_query(upstream_path_with_query);
    let replaced = gemini::replace_gemini_model_in_path(path, mapped_model)
        .unwrap_or_else(|| path.to_string());
    match query {
        Some(query) => format!("{replaced}?{query}"),
        None => replaced,
    }
}

fn apply_header_overrides(
    request_headers: &mut HeaderMap,
    overrides: &[super::config::HeaderOverride],
) {
    for override_item in overrides {
        // 屏蔽 hop-by-hop / Host / Content-Length，无论配置为何。
        if crate::proxy::http::is_hop_header(&override_item.name)
            || override_item.name == HOST
            || override_item.name == CONTENT_LENGTH
        {
            continue;
        }

        match &override_item.value {
            Some(value) => {
                request_headers.insert(override_item.name.clone(), value.clone());
            }
            None => {
                request_headers.remove(&override_item.name);
            }
        }
    }
}

fn split_path_query(path_with_query: &str) -> (&str, Option<&str>) {
    match path_with_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_with_query, None),
    }
}

fn build_request_headers(
    provider: &str,
    headers: &HeaderMap,
    auth: http::UpstreamAuthHeader,
    header_overrides: Option<&[super::config::HeaderOverride]>,
) -> HeaderMap {
    let mut request_headers = http::build_upstream_headers(headers, auth);
    if provider == "anthropic" && !request_headers.contains_key(ANTHROPIC_VERSION_HEADER) {
        // Anthropic 官方 API 需要 `anthropic-version`；缺省时补一个稳定默认值，允许客户端覆盖。
        request_headers.insert(
            ANTHROPIC_VERSION_HEADER,
            HeaderValue::from_static(DEFAULT_ANTHROPIC_VERSION),
        );
    }

    if let Some(overrides) = header_overrides {
        apply_header_overrides(&mut request_headers, overrides);
    }
    request_headers
}

async fn build_upstream_body(
    body: &ReplayableBody,
    meta: &RequestMeta,
) -> Result<reqwest::Body, AttemptOutcome> {
    let mapped_body = maybe_rewrite_request_body_model(body, meta).await?;
    let source = mapped_body.as_ref().unwrap_or(body);
    source
        .to_reqwest_body()
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read cached request body: {err}"),
            ))
        })
}

async fn maybe_rewrite_request_body_model(
    body: &ReplayableBody,
    meta: &RequestMeta,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    if meta.model_override().is_none() {
        return Ok(None);
    }
    let Some(mapped_model) = meta.mapped_model.as_deref() else {
        return Ok(None);
    };
    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_MODEL_MAPPING_LIMIT_BYTES)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read request body: {err}"),
            ))
        })?
    else {
        return Ok(None);
    };
    let Some(rewritten) = model::rewrite_request_model(&bytes, mapped_model) else {
        return Ok(None);
    };
    Ok(Some(ReplayableBody::from_bytes(rewritten)))
}

fn resolve_gemini_upstream(
    upstream: &UpstreamRuntime,
    request_auth: &RequestAuth,
    upstream_path_with_query: &str,
    upstream_url: &str,
) -> Result<(String, http::UpstreamAuthHeader), AttemptOutcome> {
    let query_key = extract_query_param(upstream_path_with_query, GEMINI_API_KEY_QUERY);
    let selected = upstream
        .api_key
        .as_deref()
        .or_else(|| request_auth.gemini_api_key.as_deref())
        .or_else(|| query_key.as_deref());

    let Some(api_key) = selected else {
        return Err(AttemptOutcome::SkippedAuth);
    };

    let upstream_url = match ensure_query_param(upstream_url, GEMINI_API_KEY_QUERY, api_key) {
        Ok(url) => url,
        Err(message) => {
            return Err(AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to build upstream URL: {message}"),
            )))
        }
    };

    let value = HeaderValue::from_str(api_key).map_err(|_| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream API key contains invalid characters.",
        ))
    })?;

    Ok((
        upstream_url,
        http::UpstreamAuthHeader {
            name: GEMINI_API_KEY_HEADER.clone(),
            value,
        },
    ))
}

#[cfg(test)]
mod tests;
