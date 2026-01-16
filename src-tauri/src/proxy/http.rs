use axum::{
    body::Body,
    http::{
        header::{
            HeaderName, HeaderValue, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HOST,
            PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
        },
        HeaderMap, StatusCode,
    },
    response::Response,
};
use reqwest::header::HeaderMap as ReqwestHeaderMap;
use serde_json::json;

use super::{
    config::{ProxyConfig, UpstreamRuntime},
    gemini,
    server_helpers::is_anthropic_path,
};
use url::form_urlencoded;

const KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
const X_OPENAI_API_KEY: &str = "x-openai-api-key";
const X_API_KEY: &str = "x-api-key";
const X_ANTHROPIC_API_KEY: &str = "x-anthropic-api-key";
const X_GOOG_API_KEY: &str = "x-goog-api-key";

pub(crate) fn ensure_local_auth(
    config: &ProxyConfig,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = config.local_api_key.as_ref() else {
        tracing::debug!("no local_api_key configured, skipping local auth");
        return Ok(());
    };
    tracing::debug!(path = %path, "local auth required, resolving local key");
    let Some(provided) = resolve_local_auth_token(headers, path, query)? else {
        tracing::warn!(path = %path, "missing local access key");
        return Err("Missing local access key.".to_string());
    };
    if provided != expected.as_str() {
        tracing::warn!(
            path = %path,
            got = %mask_key(&provided),
            expected = %mask_key(expected),
            "local auth mismatch"
        );
        return Err("Local access key is invalid.".to_string());
    }
    tracing::debug!(path = %path, "local auth passed");
    Ok(())
}

/// 遮蔽敏感 key，仅显示前 8 字符
fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        return key.to_string();
    }
    format!("{}...", &key[..8])
}

fn resolve_local_auth_token(
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
) -> Result<Option<String>, String> {
    // Local auth follows request format: Anthropic -> x-api-key (or Authorization), Gemini -> x-goog-api-key/?key, others -> Authorization.
    if is_anthropic_path(path) {
        if let Some(value) = parse_raw_header(headers, X_API_KEY)? {
            return Ok(Some(value));
        }
        if let Some(value) = parse_raw_header(headers, X_ANTHROPIC_API_KEY)? {
            return Ok(Some(value));
        }
        return parse_bearer_header(headers);
    }

    if gemini::is_gemini_path(path) {
        if let Some(value) = parse_raw_header(headers, X_GOOG_API_KEY)? {
            return Ok(Some(value));
        }
        return parse_query_key(query);
    }

    parse_bearer_header(headers)
}

fn parse_raw_header(headers: &HeaderMap, name: &str) -> Result<Option<String>, String> {
    let Some(header) = headers.get(name) else {
        return Ok(None);
    };
    let Ok(value) = header.to_str() else {
        return Err("Local access key is invalid.".to_string());
    };
    let value = value.trim();
    if value.is_empty() {
        return Err("Local access key is invalid.".to_string());
    }
    Ok(Some(value.to_string()))
}

fn parse_bearer_header(headers: &HeaderMap) -> Result<Option<String>, String> {
    let Some(header) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let Ok(value) = header.to_str() else {
        return Err("Local access key is invalid.".to_string());
    };
    let value = value.trim();
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err("Local access key is invalid.".to_string());
    };
    let token = token.trim();
    if token.is_empty() {
        return Err("Local access key is invalid.".to_string());
    }
    Ok(Some(token.to_string()))
}

fn parse_query_key(query: Option<&str>) -> Result<Option<String>, String> {
    let Some(query) = query else {
        return Ok(None);
    };
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if key != "key" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return Err("Local access key is invalid.".to_string());
        }
        return Ok(Some(value.to_string()));
    }
    Ok(None)
}

#[derive(Clone, Default)]
pub(crate) struct RequestAuth {
    pub(crate) openai_bearer: Option<HeaderValue>,
    pub(crate) anthropic_api_key: Option<HeaderValue>,
    pub(crate) gemini_api_key: Option<String>,
    pub(crate) authorization_fallback: Option<HeaderValue>,
}

pub(crate) struct UpstreamAuthHeader {
    pub(crate) name: HeaderName,
    pub(crate) value: HeaderValue,
}

pub(crate) fn resolve_request_auth(
    config: &ProxyConfig,
    headers: &HeaderMap,
) -> Result<RequestAuth, String> {
    let mut auth = RequestAuth::default();
    // When local auth is enabled, request auth headers are reserved for local access and not used upstream.
    if config.local_api_key.is_none() {
        if let Some(value) = headers.get(X_OPENAI_API_KEY) {
            let Ok(value) = value.to_str() else {
                return Err("Upstream API key is invalid.".to_string());
            };
            auth.openai_bearer = Some(bearer_header(value).ok_or_else(|| {
                "Upstream API key contains invalid characters.".to_string()
            })?);
        }

        // Anthropic uses `x-api-key`; allow explicit overrides as well.
        if let Some(value) = headers
            .get(X_API_KEY)
            .or_else(|| headers.get(X_ANTHROPIC_API_KEY))
        {
            let Ok(_) = value.to_str() else {
                return Err("Upstream API key is invalid.".to_string());
            };
            auth.anthropic_api_key = Some(value.clone());
        }

        if let Some(value) = headers.get(AUTHORIZATION) {
            auth.authorization_fallback = Some(value.clone());
        }

        if let Some(value) = headers.get(X_GOOG_API_KEY) {
            let Ok(value) = value.to_str() else {
                return Err("Upstream API key is invalid.".to_string());
            };
            let value = value.trim();
            if !value.is_empty() {
                auth.gemini_api_key = Some(value.to_string());
            }
        }
    }
    Ok(auth)
}

pub(crate) fn resolve_upstream_auth(
    provider: &str,
    upstream: &UpstreamRuntime,
    request_auth: &RequestAuth,
) -> Result<Option<UpstreamAuthHeader>, Response> {
    tracing::debug!(
        provider = %provider,
        upstream_id = %upstream.id,
        has_upstream_key = upstream.api_key.is_some(),
        has_openai_bearer = request_auth.openai_bearer.is_some(),
        has_anthropic_key = request_auth.anthropic_api_key.is_some(),
        has_auth_fallback = request_auth.authorization_fallback.is_some(),
        "resolving upstream auth"
    );

    match provider {
        "anthropic" => {
            let value = match upstream.api_key.as_ref() {
                Some(key) => {
                    tracing::debug!("using upstream.api_key for Anthropic");
                    HeaderValue::from_str(key).map_err(|_| {
                        error_response(
                            StatusCode::UNAUTHORIZED,
                            "Upstream API key contains invalid characters.",
                        )
                    })?
                }
                None => {
                    let Some(value) = request_auth.anthropic_api_key.clone() else {
                        tracing::warn!("no API key for Anthropic");
                        return Ok(None);
                    };
                    tracing::debug!("using request_auth.anthropic_api_key for Anthropic");
                    value
                }
            };

            Ok(Some(UpstreamAuthHeader {
                name: HeaderName::from_static(X_API_KEY),
                value,
            }))
        }
        _ => {
            if let Some(key) = upstream.api_key.as_ref() {
                tracing::debug!(provider = %provider, "using upstream.api_key");
                let value = bearer_header(key).ok_or_else(|| {
                    error_response(
                        StatusCode::UNAUTHORIZED,
                        "Upstream API key contains invalid characters.",
                    )
                })?;
                return Ok(Some(UpstreamAuthHeader {
                    name: AUTHORIZATION,
                    value,
                }));
            }

            if let Some(value) = request_auth.openai_bearer.clone() {
                tracing::debug!(provider = %provider, "using request_auth.openai_bearer");
                return Ok(Some(UpstreamAuthHeader {
                    name: AUTHORIZATION,
                    value,
                }));
            }

            if let Some(value) = request_auth.authorization_fallback.clone() {
                tracing::debug!(provider = %provider, "using request_auth.authorization_fallback");
                return Ok(Some(UpstreamAuthHeader {
                    name: AUTHORIZATION,
                    value,
                }));
            }

            tracing::warn!(provider = %provider, "no API key found");
            Ok(None)
        }
    }
}

pub(crate) fn bearer_header(value: &str) -> Option<HeaderValue> {
    let header = format!("Bearer {value}");
    HeaderValue::from_str(&header).ok()
}

pub(crate) fn build_upstream_headers(
    headers: &HeaderMap,
    auth: UpstreamAuthHeader,
) -> ReqwestHeaderMap {
    let mut output = ReqwestHeaderMap::new();
    for (name, value) in headers.iter() {
        if should_skip_request_header(name) {
            continue;
        }
        if name == AUTHORIZATION
            || name == &auth.name
            || name.as_str().eq_ignore_ascii_case(X_OPENAI_API_KEY)
            || name.as_str().eq_ignore_ascii_case(X_API_KEY)
            || name.as_str().eq_ignore_ascii_case(X_ANTHROPIC_API_KEY)
            || name.as_str().eq_ignore_ascii_case(X_GOOG_API_KEY)
        {
            continue;
        }
        output.append(name.clone(), value.clone());
    }
    output.insert(auth.name, auth.value);
    output
}

fn should_skip_request_header(name: &HeaderName) -> bool {
    is_hop_header(name) || name == HOST || name == CONTENT_LENGTH
}

pub(crate) fn is_hop_header(name: &HeaderName) -> bool {
    name == CONNECTION
        || name == KEEP_ALIVE
        || name == PROXY_AUTHENTICATE
        || name == PROXY_AUTHORIZATION
        || name == TE
        || name == TRAILER
        || name == TRANSFER_ENCODING
        || name == UPGRADE
}

pub(crate) fn filter_response_headers(headers: &ReqwestHeaderMap) -> HeaderMap {
    let mut output = HeaderMap::new();
    for (name, value) in headers.iter() {
        if is_hop_header(name) {
            continue;
        }
        output.append(name.clone(), value.clone());
    }
    output
}

pub(crate) fn build_response(status: StatusCode, headers: HeaderMap, body: Body) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

pub(crate) fn error_response(status: StatusCode, message: impl AsRef<str>) -> Response {
    let body = json!({
        "error": {
            "message": message.as_ref(),
            "type": "proxy_error"
        }
    });
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

pub(crate) fn extract_request_id(headers: &ReqwestHeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .or_else(|| headers.get("openai-request-id"))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::LogLevel;
    use std::collections::HashMap;

    fn config_with_local(key: &str) -> ProxyConfig {
        ProxyConfig {
            host: "127.0.0.1".to_string(),
            port: 9208,
            local_api_key: Some(key.to_string()),
            log_level: LogLevel::Silent,
            max_request_body_bytes: 1024,
            enable_api_format_conversion: false,
            upstream_strategy: crate::proxy::config::UpstreamStrategy::PriorityFillFirst,
            upstreams: HashMap::new(),
        }
    }

    #[test]
    fn local_auth_accepts_anthropic_headers() {
        let config = config_with_local("local-key");
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("local-key"));
        let result = ensure_local_auth(&config, &headers, "/v1/messages", None);
        assert!(result.is_ok());
    }

    #[test]
    fn local_auth_accepts_anthropic_authorization_only() {
        let config = config_with_local("local-key");
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer local-key"));
        let result = ensure_local_auth(&config, &headers, "/v1/messages", None);
        assert!(result.is_ok());
    }

    #[test]
    fn local_auth_accepts_gemini_query_key() {
        let config = config_with_local("local-key");
        let headers = HeaderMap::new();
        let result = ensure_local_auth(
            &config,
            &headers,
            "/v1beta/models/gemini-1.5-flash:generateContent",
            Some("key=local-key"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn local_auth_accepts_openai_authorization() {
        let config = config_with_local("local-key");
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer local-key"));
        let result = ensure_local_auth(&config, &headers, "/v1/chat/completions", None);
        assert!(result.is_ok());
    }
}
