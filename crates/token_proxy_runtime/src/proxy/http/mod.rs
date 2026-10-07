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
/// Claude Code 启动探活：`HEAD/GET {ANTHROPIC_BASE_URL}/api/hello`，官方 Anthropic 上无需 key。
const CLAUDE_CONNECTIVITY_HELLO_PATH: &str = "/api/hello";
/// Grok CLI 空闲恢复时 `GET {models_base_url}/models-v2` 刷新模型元数据，且只携带 xAI 登录 token。
const GROK_MODELS_V2_PATH: &str = "/v1/models-v2";
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalAccess {
    pub(crate) key_id: String,
    pub(crate) scope: token_proxy_config::LocalApiKeyScope,
}

pub(crate) fn ensure_local_auth(
    config: &ProxyConfig,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
    query: Option<&str>,
) -> Result<Option<LocalAccess>, String> {
    if config.local_api_keys.is_empty()
        || is_public_connectivity_hello_request(method, path)
        || is_allowed_cors_preflight_request(config, headers, method)
    {
        return Ok(None);
    }
    let provided = resolve_local_auth_token(headers, path, query)?
        .ok_or_else(|| "Missing local access key.".to_string())?;
    let key = config
        .local_api_keys
        .iter()
        .find(|key| key.enabled && key.key == provided)
        .ok_or_else(|| "Local access key is invalid.".to_string())?;
    tracing::debug!(key_id = %key.id, scope = ?key.scope, decision = "allowed", "local access authenticated");
    Ok(Some(LocalAccess {
        key_id: key.id.clone(),
        scope: key.scope.clone(),
    }))
}

/// Claude Code 探活只接受 GET/HEAD；POST 仍走本地 key。
pub(crate) fn is_public_connectivity_hello_request(method: &Method, path: &str) -> bool {
    matches!(method.as_str(), "GET" | "HEAD") && path == CLAUDE_CONNECTIVITY_HELLO_PATH
}

/// Grok 元数据探测只接受 GET/HEAD；其余方法仍走普通路由与本地 key。
pub(crate) fn is_grok_models_v2_probe_request(method: &Method, path: &str) -> bool {
    matches!(method.as_str(), "GET" | "HEAD") && path == GROK_MODELS_V2_PATH
}

/// 本代理不提供 Grok 私有元数据格式，本地直接 404。
/// 不能回 401：Grok 会误判登录 token 失效并强制刷新重试，且恢复后的对话轮次要等这一过程结束。
/// 也不能回聚合目录：跨上游串行拉取要数十秒，同样会阻塞该轮次。
pub(crate) fn grok_models_v2_probe_response() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "Grok models-v2 metadata is not served by this proxy.",
    )
}

/// 探活成功体：Claude Code 只校验 HTTP 200；HEAD 无 body。
pub(crate) fn connectivity_hello_response(method: &Method) -> Response {
    let mut response = if *method == Method::HEAD {
        Response::new(Body::empty())
    } else {
        let mut response = Response::new(Body::from(json!({ "ok": true }).to_string()));
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    };
    *response.status_mut() = StatusCode::OK;
    response
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
    if !config.local_api_keys.is_empty() {
        let provided = resolve_local_auth_token(headers, path, query)?;
        return Ok(provided.filter(|provided| {
            config
                .local_api_keys
                .iter()
                .any(|key| key.enabled && key.key == *provided)
        }));
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
    pub(crate) local_access: Option<LocalAccess>,
    pub(crate) local_auth_enabled: bool,
    pub(crate) target_upstream_id: Option<String>,
    pub(crate) openai_bearer: Option<HeaderValue>,
    pub(crate) anthropic_request_auth: Option<UpstreamAuthHeader>,
    pub(crate) gemini_api_key: Option<String>,
    pub(crate) authorization_fallback: Option<HeaderValue>,
}

impl RequestAuth {
    pub(crate) fn routing_config<'a>(
        &self,
        config: &'a ProxyConfig,
    ) -> std::borrow::Cow<'a, ProxyConfig> {
        if self.target_upstream_id.is_none()
            && self.local_access.as_ref().is_none_or(|access| {
                matches!(access.scope, token_proxy_config::LocalApiKeyScope::Auto)
            })
        {
            return std::borrow::Cow::Borrowed(config);
        }
        let mut scoped = config.clone();
        scoped.upstreams.retain(|_, upstreams| {
            for group in &mut upstreams.groups {
                group
                    .items
                    .retain(|upstream| self.allows_upstream(&upstream.id));
            }
            upstreams.groups.retain(|group| !group.items.is_empty());
            !upstreams.groups.is_empty()
        });
        std::borrow::Cow::Owned(scoped)
    }

    pub(crate) fn allows_upstream(&self, id: &str) -> bool {
        self.local_access
            .as_ref()
            .is_none_or(|access| access.scope.allows(id))
            && self
                .target_upstream_id
                .as_deref()
                .is_none_or(|target| target == id)
    }
}

pub(crate) fn authorize_target_upstream(
    config: &ProxyConfig,
    auth: &RequestAuth,
    model: Option<&str>,
) -> Result<Option<String>, String> {
    let Some((prefix, rest)) = model.and_then(|model| model.trim().split_once('/')) else {
        return Ok(None);
    };
    if rest.trim().is_empty() {
        return Ok(None);
    }
    let bound = auth
        .local_access
        .as_ref()
        .is_some_and(|access| match &access.scope {
            token_proxy_config::LocalApiKeyScope::Selected { upstream_ids } => {
                upstream_ids.iter().any(|id| id == prefix)
            }
            _ => false,
        });
    let known = config.upstream_ids.contains(prefix)
        || config
            .upstreams
            .values()
            .flat_map(|provider| &provider.groups)
            .flat_map(|group| &group.items)
            .any(|upstream| upstream.id == prefix);
    if !known && !bound {
        return Ok(None);
    }
    if !auth.allows_upstream(prefix) {
        tracing::warn!(
            key_id = auth
                .local_access
                .as_ref()
                .map(|access| access.key_id.as_str())
                .unwrap_or(""),
            upstream_id = prefix,
            decision = "denied",
            "explicit upstream outside local key scope"
        );
        return Err("Local access key is not authorized for the requested upstream.".into());
    }
    Ok(Some(prefix.to_string()))
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
    let mut auth = RequestAuth {
        local_auth_enabled: !config.local_api_keys.is_empty(),
        ..RequestAuth::default()
    };
    // When local auth is enabled, request auth headers are reserved for local access and not used upstream.
    if config.local_api_keys.is_empty() {
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
    let connection_headers = connection_nominated_headers(headers);
    for (name, value) in headers.iter() {
        if should_skip_request_header(name) || connection_headers.contains(name) {
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
    let connection_headers = connection_nominated_headers(headers);
    for (name, value) in headers.iter() {
        if is_hop_header(name) || connection_headers.contains(name) {
            continue;
        }
        output.append(name.clone(), value.clone());
    }
    output
}

fn connection_nominated_headers(headers: &HeaderMap) -> Vec<HeaderName> {
    // Connection 可以有多行、多字段；这些名字声明的字段只对当前一跳有效。
    headers
        .get_all(CONNECTION)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .filter_map(|name| HeaderName::from_bytes(name.trim_ascii()).ok())
        .collect()
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
            "type": "proxy_error",
            "param": null,
            "code": "proxy_error",
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
