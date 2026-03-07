use axum::{
    body::Bytes,
    http::{
        header::{HeaderName, HeaderValue, CONTENT_LENGTH, HOST},
        HeaderMap, StatusCode,
    },
};
use serde_json::Value;

use super::super::http::RequestAuth;
use super::super::{
    codex_compat,
    config::{HeaderOverride, UpstreamRuntime},
    http, model,
    request_body::ReplayableBody,
    RequestMeta,
};
use super::{
    utils::{ensure_query_param, extract_query_param},
    AttemptOutcome,
};
use crate::proxy::server_helpers::{log_debug_headers_body, truncate_for_log};

const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const GEMINI_API_KEY_QUERY: &str = "key";
const GEMINI_API_KEY_HEADER: HeaderName = HeaderName::from_static("x-goog-api-key");
const OPENAI_RESPONSES_PATH: &str = "/v1/responses";
// Keep in sync with server_helpers request transform limit (20 MiB).
const REQUEST_FILTER_LIMIT_BYTES: usize = 20 * 1024 * 1024;
const DEBUG_UPSTREAM_LOG_LIMIT_BYTES: usize = usize::MAX;
const ANTIGRAVITY_WRAPPED_LOG_LIMIT_BYTES: usize = 8 * 1024;

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
    extra_headers: Option<&HeaderMap>,
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
    codex_compat::apply_codex_headers_if_needed(provider, &mut request_headers, headers);

    if let Some(extra_headers) = extra_headers {
        for (name, value) in extra_headers.iter() {
            request_headers.insert(name.clone(), value.clone());
        }
    }

    if let Some(overrides) = header_overrides {
        apply_header_overrides(&mut request_headers, overrides);
    }
    request_headers
}

pub(super) fn apply_header_overrides(
    request_headers: &mut HeaderMap,
    overrides: &[HeaderOverride],
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

pub(super) async fn build_upstream_body(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
    meta: &RequestMeta,
    antigravity: Option<&super::AntigravityRequestInfo>,
) -> Result<reqwest::Body, AttemptOutcome> {
    if provider == "antigravity" {
        return build_antigravity_body(body, meta, antigravity).await;
    }
    let mapped_body = maybe_rewrite_request_body_model(body, meta).await?;
    let mapped_source = mapped_body.as_ref().unwrap_or(body);
    let upstream_path = split_path_query(upstream_path_with_query).0;
    let reasoning_body =
        match super::super::server_helpers::maybe_rewrite_openai_reasoning_effort_from_model_suffix(
            provider,
            upstream_path,
            meta,
            mapped_source,
        )
        .await
        {
            Ok(body) => body,
            Err(err) => {
                return Err(AttemptOutcome::Fatal(http::error_response(
                    err.status,
                    err.message,
                )))
            }
        };
    let source = reasoning_body
        .as_ref()
        .or(mapped_body.as_ref())
        .unwrap_or(body);
    let filtered = maybe_filter_openai_responses_request_fields(
        provider,
        upstream,
        upstream_path_with_query,
        source,
    )
    .await?;
    let final_source = filtered.as_ref().unwrap_or(source);
    final_source.to_reqwest_body().await.map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to read cached request body: {err}"),
        ))
    })
}

async fn maybe_filter_openai_responses_request_fields(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    let should_filter_prompt_cache_retention = upstream.filter_prompt_cache_retention;
    let should_filter_safety_identifier = upstream.filter_safety_identifier;
    if provider != "openai-response"
        || (!should_filter_prompt_cache_retention && !should_filter_safety_identifier)
    {
        return Ok(None);
    }
    let upstream_path = split_path_query(upstream_path_with_query).0;
    if upstream_path != OPENAI_RESPONSES_PATH {
        return Ok(None);
    }

    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_FILTER_LIMIT_BYTES)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read cached request body: {err}"),
            ))
        })?
    else {
        // Best-effort: request body too large to rewrite.
        return Ok(None);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    let mut changed = false;
    if should_filter_prompt_cache_retention {
        changed = changed || object.remove("prompt_cache_retention").is_some();
    }
    if should_filter_safety_identifier {
        changed = changed || object.remove("safety_identifier").is_some();
    }
    if !changed {
        return Ok(None);
    }

    let outbound_bytes = serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to serialize request: {err}"),
        ))
    })?;
    Ok(Some(ReplayableBody::from_bytes(outbound_bytes)))
}

async fn build_antigravity_body(
    body: &ReplayableBody,
    meta: &RequestMeta,
    antigravity: Option<&super::AntigravityRequestInfo>,
) -> Result<reqwest::Body, AttemptOutcome> {
    let Some(info) = antigravity else {
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Antigravity account is not configured.",
        )));
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
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            "Antigravity request body is too large.",
        )));
    };
    let model = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref());
    let wrapped = super::super::antigravity_compat::wrap_gemini_request(
        &bytes,
        model,
        info.project_id.as_deref(),
        &info.user_agent,
    )
    .map_err(|message| {
        AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_GATEWAY, message))
    })?;
    let wrapped_body = ReplayableBody::from_bytes(wrapped.clone());
    log_debug_headers_body(
        "antigravity.wrapped",
        None,
        Some(&wrapped_body),
        DEBUG_UPSTREAM_LOG_LIMIT_BYTES,
    )
    .await;
    log_antigravity_wrapped_body(&wrapped);
    Ok(reqwest::Body::from(wrapped))
}

fn log_antigravity_wrapped_body(bytes: &[u8]) {
    if !tracing::enabled!(tracing::Level::WARN) {
        return;
    }
    let body_text = String::from_utf8_lossy(bytes);
    let truncated = truncate_for_log(&body_text, ANTIGRAVITY_WRAPPED_LOG_LIMIT_BYTES);
    // 仅在 antigravity 请求阶段记录，便于复现上游校验错误。
    tracing::warn!(body = %truncated, "antigravity wrapped request payload");
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

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "request.test.rs"]
mod tests;

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
