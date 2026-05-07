use axum::{
    body::Body,
    http::{
        header::{
            HeaderName, HeaderValue, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HOST,
            PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
        },
        HeaderMap, Method, StatusCode,
    },
    response::Response,
};
use reqwest::header::HeaderMap as ReqwestHeaderMap;
use serde_json::json;
use std::net::IpAddr;

use super::{
    config::{ProxyConfig, StaticApiKeyHeaders, UpstreamRuntime},
    gemini,
    server_helpers::is_anthropic_path,
};
use url::form_urlencoded;

const KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
const X_OPENAI_API_KEY: &str = "x-openai-api-key";
const X_API_KEY: &str = "x-api-key";
const X_ANTHROPIC_API_KEY: &str = "x-anthropic-api-key";
const X_GOOG_API_KEY: &str = "x-goog-api-key";
const OPENAI_MODELS_INDEX_PATH: &str = "/v1/models";
const OPENAI_COMPATIBLE_MODELS_INDEX_PATH: &str = "/v1beta/openai/models";
const ORIGIN: HeaderName = HeaderName::from_static("origin");
const VARY: HeaderName = HeaderName::from_static("vary");
const ACCESS_CONTROL_REQUEST_METHOD: HeaderName =
    HeaderName::from_static("access-control-request-method");
const ACCESS_CONTROL_REQUEST_HEADERS: HeaderName =
    HeaderName::from_static("access-control-request-headers");
const ACCESS_CONTROL_ALLOW_ORIGIN: HeaderName =
    HeaderName::from_static("access-control-allow-origin");
const ACCESS_CONTROL_ALLOW_METHODS: HeaderName =
    HeaderName::from_static("access-control-allow-methods");
const ACCESS_CONTROL_ALLOW_HEADERS: HeaderName =
    HeaderName::from_static("access-control-allow-headers");
const ACCESS_CONTROL_MAX_AGE: HeaderName = HeaderName::from_static("access-control-max-age");
const CORS_ALLOW_METHODS: HeaderValue =
    HeaderValue::from_static("GET,POST,PUT,PATCH,DELETE,OPTIONS,HEAD");
const CORS_DEFAULT_ALLOW_HEADERS: HeaderValue = HeaderValue::from_static(
    "authorization,content-type,x-api-key,x-goog-api-key,x-openai-api-key,x-anthropic-api-key",
);
const CORS_PREFLIGHT_VARY: HeaderValue = HeaderValue::from_static(
    "origin, access-control-request-method, access-control-request-headers",
);
const CORS_ACTUAL_VARY: HeaderValue = HeaderValue::from_static("origin");
const CORS_MAX_AGE: HeaderValue = HeaderValue::from_static("86400");

pub(crate) fn ensure_local_auth(
    config: &ProxyConfig,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
    query: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = config.local_api_key.as_ref() else {
        tracing::debug!("no local_api_key configured, skipping local auth");
        return Ok(());
    };
    if is_public_model_catalog_request(method, path) {
        tracing::debug!(method = %method, path = %path, "public model catalog request skips local auth");
        return Ok(());
    }
    if is_allowed_cors_preflight_request(config, headers, method) {
        tracing::debug!(method = %method, path = %path, "cors preflight skips local auth");
        return Ok(());
    }
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

fn is_public_model_catalog_request(method: &Method, path: &str) -> bool {
    matches!(method.as_str(), "GET" | "HEAD")
        && matches!(
            path,
            OPENAI_MODELS_INDEX_PATH | OPENAI_COMPATIBLE_MODELS_INDEX_PATH
        )
}

pub(crate) fn cors_preflight_response(
    config: &ProxyConfig,
    headers: &HeaderMap,
    method: &Method,
) -> Option<Response> {
    if !is_cors_preflight_request(headers, method) {
        return None;
    }
    let allow_origin = resolve_allowed_cors_origin(config, headers)?;
    let mut response = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()));
    insert_cors_preflight_headers(response.headers_mut(), allow_origin, headers);
    Some(response)
}

pub(crate) fn with_cors_headers(
    config: &ProxyConfig,
    request_headers: &HeaderMap,
    mut response: Response,
) -> Response {
    let Some(allow_origin) = resolve_allowed_cors_origin(config, request_headers) else {
        return response;
    };
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin);
    response.headers_mut().insert(VARY, CORS_ACTUAL_VARY);
    response
}

fn is_allowed_cors_preflight_request(
    config: &ProxyConfig,
    headers: &HeaderMap,
    method: &Method,
) -> bool {
    is_cors_preflight_request(headers, method)
        && resolve_allowed_cors_origin(config, headers).is_some()
}

fn is_cors_preflight_request(headers: &HeaderMap, method: &Method) -> bool {
    method == Method::OPTIONS
        && headers.contains_key(&ORIGIN)
        && headers.contains_key(&ACCESS_CONTROL_REQUEST_METHOD)
}

fn resolve_allowed_cors_origin(config: &ProxyConfig, headers: &HeaderMap) -> Option<HeaderValue> {
    if !config.cors_enabled {
        return None;
    }
    let origin = headers.get(&ORIGIN)?.to_str().ok()?.trim();
    if origin.is_empty() || !is_loopback_origin(origin) {
        return None;
    }
    HeaderValue::from_str(origin).ok()
}

fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // 本地代理 CORS 只给 loopback 浏览器源开放，避免任意公网 origin 调用本机代理。
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|addr| addr.is_loopback())
        .unwrap_or(false)
}

fn insert_cors_preflight_headers(
    response_headers: &mut HeaderMap,
    allow_origin: HeaderValue,
    request_headers: &HeaderMap,
) {
    response_headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin);
    response_headers.insert(ACCESS_CONTROL_ALLOW_METHODS, CORS_ALLOW_METHODS);
    let allow_headers = request_headers
        .get(&ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned()
        .unwrap_or(CORS_DEFAULT_ALLOW_HEADERS);
    response_headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, allow_headers);
    response_headers.insert(ACCESS_CONTROL_MAX_AGE, CORS_MAX_AGE);
    response_headers.insert(VARY, CORS_PREFLIGHT_VARY);
}

pub(crate) fn resolve_client_gemini_api_key(
    config: &ProxyConfig,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
) -> Result<Option<String>, String> {
    if !gemini::is_gemini_native_path(path) {
        return Ok(None);
    }
    if let Some(local_key) = config.local_api_key.as_ref() {
        return Ok(Some(local_key.clone()));
    }
    if let Some(value) = parse_raw_header(headers, X_GOOG_API_KEY)? {
        return Ok(Some(value));
    }
    parse_query_key(query)
}

pub(crate) fn local_proxy_base_url(config: &ProxyConfig) -> String {
    let host = config.host.trim();
    let host = if host.contains(':') && !host.starts_with('[') && !host.ends_with(']') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    format!("http://{host}:{}", config.port)
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

    if gemini::is_gemini_native_path(path) {
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
    let Some(token) = extract_bearer_token(value) else {
        return Err("Local access key is invalid.".to_string());
    };
    Ok(Some(token.to_string()))
}

fn extract_bearer_token(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token)
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
    pub(crate) anthropic_request_auth: Option<UpstreamAuthHeader>,
    pub(crate) gemini_api_key: Option<String>,
    pub(crate) authorization_fallback: Option<HeaderValue>,
}

#[derive(Clone)]
pub(crate) struct UpstreamAuthHeader {
    pub(crate) name: HeaderName,
    pub(crate) value: HeaderValue,
}

pub(crate) fn resolve_request_auth(
    config: &ProxyConfig,
    headers: &HeaderMap,
    path: &str,
) -> Result<RequestAuth, String> {
    let mut auth = RequestAuth::default();
    // When local auth is enabled, request auth headers are reserved for local access and not used upstream.
    if config.local_api_key.is_none() {
        if let Some(value) = headers.get(X_OPENAI_API_KEY) {
            let Ok(value) = value.to_str() else {
                return Err("Upstream API key is invalid.".to_string());
            };
            auth.openai_bearer = Some(
                bearer_header(value)
                    .ok_or_else(|| "Upstream API key contains invalid characters.".to_string())?,
            );
        }

        if is_anthropic_path(path) {
            auth.anthropic_request_auth = resolve_anthropic_request_auth(headers)?;
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

fn resolve_anthropic_request_auth(
    headers: &HeaderMap,
) -> Result<Option<UpstreamAuthHeader>, String> {
    if let Some(value) = headers.get(X_API_KEY) {
        let Ok(_) = value.to_str() else {
            return Err("Upstream API key is invalid.".to_string());
        };
        return Ok(Some(UpstreamAuthHeader {
            name: HeaderName::from_static(X_API_KEY),
            value: value.clone(),
        }));
    }

    if let Some(value) = headers.get(X_ANTHROPIC_API_KEY) {
        let Ok(_) = value.to_str() else {
            return Err("Upstream API key is invalid.".to_string());
        };
        return Ok(Some(UpstreamAuthHeader {
            name: HeaderName::from_static(X_ANTHROPIC_API_KEY),
            value: value.clone(),
        }));
    }

    let Some(value) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let Ok(value_str) = value.to_str() else {
        return Err("Upstream API key is invalid.".to_string());
    };
    if extract_bearer_token(value_str).is_none() {
        return Err("Upstream API key is invalid.".to_string());
    }
    Ok(Some(UpstreamAuthHeader {
        name: AUTHORIZATION,
        value: value.clone(),
    }))
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
        has_anthropic_key = request_auth.anthropic_request_auth.is_some(),
        has_auth_fallback = request_auth.authorization_fallback.is_some(),
        "resolving upstream auth"
    );

    match provider {
        "anthropic" => {
            if let Some(api_key_headers) = resolve_static_api_key_headers(upstream)? {
                tracing::debug!("using upstream.api_key for Anthropic");
                if let Some(request_header) = request_auth.anthropic_request_auth.as_ref() {
                    let value = if request_header.name == AUTHORIZATION {
                        api_key_headers.bearer()
                    } else {
                        api_key_headers.raw()
                    };
                    return Ok(Some(UpstreamAuthHeader {
                        name: request_header.name.clone(),
                        value,
                    }));
                }

                return Ok(Some(UpstreamAuthHeader {
                    name: HeaderName::from_static(X_API_KEY),
                    value: api_key_headers.raw(),
                }));
            }

            if let Some(header) = request_auth.anthropic_request_auth.clone() {
                tracing::debug!("using native anthropic request auth header");
                return Ok(Some(header));
            }

            let Some(value) = request_auth
                .authorization_fallback
                .as_ref()
                .and_then(|value| value.to_str().ok())
                .and_then(extract_bearer_token)
                .and_then(|value| HeaderValue::from_str(value).ok())
            else {
                tracing::warn!("no API key for Anthropic");
                return Ok(None);
            };

            tracing::debug!("using request auth fallback for Anthropic");
            Ok(Some(UpstreamAuthHeader {
                name: HeaderName::from_static(X_API_KEY),
                value,
            }))
        }
        _ => {
            if let Some(api_key_headers) = resolve_static_api_key_headers(upstream)? {
                tracing::debug!(provider = %provider, "using upstream.api_key");
                return Ok(Some(UpstreamAuthHeader {
                    name: AUTHORIZATION,
                    value: api_key_headers.bearer(),
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

fn resolve_static_api_key_headers(
    upstream: &UpstreamRuntime,
) -> Result<Option<StaticApiKeyHeaders>, Response> {
    if let Some(headers) = upstream.api_key_headers.as_ref() {
        return Ok(Some(headers.clone()));
    }
    let Some(key) = upstream.api_key.as_deref() else {
        return Ok(None);
    };
    StaticApiKeyHeaders::new(&upstream.id, key)
        .map(Some)
        .map_err(|_| {
            error_response(
                StatusCode::UNAUTHORIZED,
                "Upstream API key contains invalid characters.",
            )
        })
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

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
