use axum::http::{
    header::{HeaderName, HeaderValue, CONTENT_LENGTH, HOST},
    HeaderMap, StatusCode,
};

use super::{
    utils::{ensure_query_param, extract_query_param},
    AttemptOutcome,
};
use super::super::{
    config::{HeaderOverride, UpstreamRuntime},
    http,
    model,
    request_body::ReplayableBody,
    RequestMeta,
};
use super::super::http::RequestAuth;

const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const GEMINI_API_KEY_QUERY: &str = "key";
const GEMINI_API_KEY_HEADER: HeaderName = HeaderName::from_static("x-goog-api-key");

pub(super) fn split_path_query(path_with_query: &str) -> (&str, Option<&str>) {
    match path_with_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_with_query, None),
    }
}

pub(super) fn build_request_headers(
    provider: &str,
    headers: &HeaderMap,
    auth: http::UpstreamAuthHeader,
    header_overrides: Option<&[HeaderOverride]>,
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

pub(super) fn apply_header_overrides(request_headers: &mut HeaderMap, overrides: &[HeaderOverride]) {
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

pub(super) async fn build_upstream_body(
    provider: &str,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
    meta: &RequestMeta,
) -> Result<reqwest::Body, AttemptOutcome> {
    let mapped_body = maybe_rewrite_request_body_model(body, meta).await?;
    let mapped_source = mapped_body.as_ref().unwrap_or(body);
    let upstream_path = split_path_query(upstream_path_with_query).0;
    let reasoning_body = match super::super::server_helpers::maybe_rewrite_openai_reasoning_effort_from_model_suffix(
        provider,
        upstream_path,
        meta,
        mapped_source,
    )
    .await
    {
        Ok(body) => body,
        Err(err) => {
            return Err(AttemptOutcome::Fatal(http::error_response(err.status, err.message)))
        }
    };
    let source = reasoning_body
        .as_ref()
        .or(mapped_body.as_ref())
        .unwrap_or(body);
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
        .read_bytes_if_small(super::REQUEST_MODEL_MAPPING_LIMIT_BYTES)
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

pub(super) fn resolve_gemini_upstream(
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
            name: GEMINI_API_KEY_HEADER,
            value,
        },
    ))
}
