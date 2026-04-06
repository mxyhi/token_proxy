use axum::{
    body::Bytes,
    http::{
        header::{HeaderName, HeaderValue, CONTENT_LENGTH, HOST},
        HeaderMap, StatusCode,
    },
};
use serde_json::Value;
use url::Url;

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
use crate::proxy::server_helpers::is_anthropic_path;

const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const GEMINI_API_KEY_QUERY: &str = "key";
const GEMINI_PROXY_UPLOAD_TARGET_QUERY: &str = "tp_upload_target";
const GEMINI_API_KEY_HEADER: HeaderName = HeaderName::from_static("x-goog-api-key");
const OPENAI_CHAT_PATH: &str = "/v1/chat/completions";
const OPENAI_RESPONSES_PATH: &str = "/v1/responses";
// Keep in sync with server_helpers request transform limit (20 MiB).
const REQUEST_FILTER_LIMIT_BYTES: usize = 20 * 1024 * 1024;
pub(super) fn split_path_query(path_with_query: &str) -> (&str, Option<&str>) {
    match path_with_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_with_query, None),
    }
}

pub(super) fn build_request_headers(
    provider: &str,
    inbound_path: &str,
    headers: &HeaderMap,
    auth: http::UpstreamAuthHeader,
    extra_headers: Option<&HeaderMap>,
    header_overrides: Option<&[HeaderOverride]>,
) -> HeaderMap {
    let mut request_headers = http::build_upstream_headers(headers, auth);
    sanitize_anthropic_fallback_headers(provider, inbound_path, &mut request_headers);
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

fn sanitize_anthropic_fallback_headers(
    provider: &str,
    inbound_path: &str,
    request_headers: &mut HeaderMap,
) {
    if !is_anthropic_path(inbound_path) || provider == "anthropic" {
        return;
    }
    // `anthropic-version` / `anthropic-beta` 只对 Anthropic 原生协议有意义。
    // 当 Claude/Anthropic 请求 fallback 到其他 provider 时，继续透传这些头
    // 只会把协议专属元信息泄漏到不相关上游。
    request_headers.remove(ANTHROPIC_VERSION_HEADER);
    request_headers.remove("anthropic-beta");
    let stainless_headers: Vec<HeaderName> = request_headers
        .keys()
        .filter(|name| name.as_str().starts_with("x-stainless-"))
        .cloned()
        .collect();
    for name in stainless_headers {
        request_headers.remove(name);
    }
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
) -> Result<reqwest::Body, AttemptOutcome> {
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
    let rewrite = maybe_rewrite_developer_role_to_system(
        upstream,
        upstream_path_with_query,
        filtered.as_ref().unwrap_or(source),
    )
    .await?;
    let final_source = rewrite.as_ref().or(filtered.as_ref()).unwrap_or(source);
    final_source.to_reqwest_body().await.map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to read cached request body: {err}"),
        ))
    })
}

async fn maybe_rewrite_developer_role_to_system(
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    if !should_rewrite_developer_role_to_system(upstream) {
        return Ok(None);
    }

    let upstream_path = split_path_query(upstream_path_with_query).0;
    if upstream_path != OPENAI_CHAT_PATH && upstream_path != OPENAI_RESPONSES_PATH {
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
        return Ok(None);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };

    let changed = if upstream_path == OPENAI_CHAT_PATH {
        rewrite_chat_developer_roles(object)
    } else {
        rewrite_responses_developer_roles(object)
    };
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

fn should_rewrite_developer_role_to_system(upstream: &UpstreamRuntime) -> bool {
    upstream.rewrite_developer_role_to_system
}

fn rewrite_chat_developer_roles(object: &mut serde_json::Map<String, Value>) -> bool {
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for message in messages {
        let Some(item) = message.as_object_mut() else {
            continue;
        };
        changed |= rewrite_role_field(item);
    }
    changed
}

fn rewrite_responses_developer_roles(object: &mut serde_json::Map<String, Value>) -> bool {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        changed |= rewrite_role_field(item);
    }
    changed
}

fn rewrite_role_field(object: &mut serde_json::Map<String, Value>) -> bool {
    let Some(role) = object.get_mut("role") else {
        return false;
    };
    if role.as_str() != Some("developer") {
        return false;
    }
    *role = Value::String("system".to_string());
    true
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

    let upstream_url = match resolve_gemini_target_url(upstream, upstream_path_with_query) {
        Ok(Some(url)) => url,
        Ok(None) => upstream_url.to_string(),
        Err(message) => {
            return Err(AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to resolve Gemini upload target: {message}"),
            )))
        }
    };

    let upstream_url = match remove_query_param(&upstream_url, GEMINI_API_KEY_QUERY)
        .and_then(|url| ensure_query_param(&url, GEMINI_API_KEY_QUERY, api_key))
    {
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

fn resolve_gemini_target_url(
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
) -> Result<Option<String>, String> {
    let Some(target) =
        extract_query_param(upstream_path_with_query, GEMINI_PROXY_UPLOAD_TARGET_QUERY)
    else {
        return Ok(None);
    };
    let target_url = Url::parse(&target).map_err(|err| err.to_string())?;
    validate_gemini_upload_target(upstream, &target_url)?;
    Ok(Some(target_url.to_string()))
}

fn validate_gemini_upload_target(
    upstream: &UpstreamRuntime,
    target_url: &Url,
) -> Result<(), String> {
    let upstream_base = Url::parse(&upstream.base_url).map_err(|err| err.to_string())?;
    let same_origin = upstream_base.scheme() == target_url.scheme()
        && upstream_base.host_str() == target_url.host_str()
        && upstream_base.port_or_known_default() == target_url.port_or_known_default();
    if !same_origin {
        return Err("upload target origin does not match configured Gemini upstream".to_string());
    }
    let base_path = upstream_base.path().trim_end_matches('/');
    let target_path = target_url.path();
    if !base_path.is_empty()
        && base_path != "/"
        && !target_path.starts_with(&format!("{base_path}/"))
        && target_path != base_path
    {
        return Err(
            "upload target path is outside configured Gemini upstream base path".to_string(),
        );
    }
    Ok(())
}

fn remove_query_param(url: &str, key: &str) -> Result<String, String> {
    let mut parsed = Url::parse(url).map_err(|err| err.to_string())?;
    let pairs = parsed
        .query_pairs()
        .filter(|(name, _)| name != key)
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    parsed.set_query(None);
    if !pairs.is_empty() {
        let mut query = parsed.query_pairs_mut();
        for (name, value) in pairs {
            query.append_pair(&name, &value);
        }
    }
    Ok(parsed.to_string())
}
