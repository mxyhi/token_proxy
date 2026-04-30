use super::*;
use crate::logging::LogLevel;
use crate::proxy::config::UpstreamRuntime;
use std::collections::HashMap;

fn config_with_local(key: &str) -> ProxyConfig {
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: Some(key.to_string()),
        cors_enabled: true,
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 1024,
        retryable_failure_cooldown: std::time::Duration::from_secs(15),
        upstream_no_data_timeout: std::time::Duration::from_secs(120),
        upstream_strategy: crate::proxy::config::UpstreamStrategyRuntime::default(),
        hot_model_mappings: HashMap::new(),
        upstreams: HashMap::new(),
        kiro_preferred_endpoint: None,
    }
}

fn config_without_local() -> ProxyConfig {
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: None,
        cors_enabled: true,
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 1024,
        retryable_failure_cooldown: std::time::Duration::from_secs(15),
        upstream_no_data_timeout: std::time::Duration::from_secs(120),
        upstream_strategy: crate::proxy::config::UpstreamStrategyRuntime::default(),
        hot_model_mappings: HashMap::new(),
        upstreams: HashMap::new(),
        kiro_preferred_endpoint: None,
    }
}

fn upstream_without_key() -> UpstreamRuntime {
    UpstreamRuntime {
        id: "anthropic-test".to_string(),
        selector_key: "anthropic-test".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    }
}

#[test]
fn local_auth_accepts_anthropic_headers() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_static("local-key"));
    let result = ensure_local_auth(&config, &headers, &Method::POST, "/v1/messages", None);
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_anthropic_authorization_only() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer local-key"));
    let result = ensure_local_auth(&config, &headers, &Method::POST, "/v1/messages", None);
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_gemini_query_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::POST,
        "/v1beta/models/gemini-1.5-flash:generateContent",
        Some("key=local-key"),
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_gemini_model_catalog_query_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::GET,
        "/v1beta/models",
        Some("key=local-key"),
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_gemini_count_tokens_query_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::POST,
        "/v1beta/models/gemini-1.5-flash:countTokens",
        Some("key=local-key"),
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_gemini_upload_files_query_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::POST,
        "/upload/v1beta/files",
        Some("key=local-key"),
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_openai_authorization() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer local-key"));
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::POST,
        "/v1/chat/completions",
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_lowercase_bearer_authorization() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("bearer local-key"));
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::POST,
        "/v1/chat/completions",
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_allows_openai_models_index_without_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(&config, &headers, &Method::GET, "/v1/models", None);
    assert!(result.is_ok());
}

#[test]
fn local_auth_allows_openai_compatible_models_index_without_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::HEAD,
        "/v1beta/openai/models",
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn local_auth_rejects_post_models_index_without_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(&config, &headers, &Method::POST, "/v1/models", None);
    assert_eq!(result, Err("Missing local access key.".to_string()));
}

#[test]
fn local_auth_allows_cors_preflight_without_key_when_enabled() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("http://localhost:9021"));
    headers.insert(
        "access-control-request-method",
        HeaderValue::from_static("POST"),
    );
    headers.insert(
        "access-control-request-headers",
        HeaderValue::from_static("authorization,content-type"),
    );

    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::OPTIONS,
        "/v1/chat/completions",
        None,
    );

    assert!(result.is_ok());
}

#[test]
fn local_auth_rejects_cors_preflight_without_key_when_disabled() {
    let mut config = config_with_local("local-key");
    config.cors_enabled = false;
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("http://localhost:9021"));
    headers.insert(
        "access-control-request-method",
        HeaderValue::from_static("POST"),
    );

    let result = ensure_local_auth(
        &config,
        &headers,
        &Method::OPTIONS,
        "/v1/chat/completions",
        None,
    );

    assert_eq!(result, Err("Missing local access key.".to_string()));
}

#[test]
fn cors_preflight_response_echoes_loopback_origin_and_requested_headers() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("http://localhost:9021"));
    headers.insert(
        "access-control-request-method",
        HeaderValue::from_static("POST"),
    );
    headers.insert(
        "access-control-request-headers",
        HeaderValue::from_static("authorization,content-type"),
    );

    let response =
        cors_preflight_response(&config, &headers, &Method::OPTIONS).expect("preflight response");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:9021")
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-headers")
            .and_then(|value| value.to_str().ok()),
        Some("authorization,content-type")
    );
}

#[test]
fn cors_preflight_response_rejects_non_loopback_origin() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("https://example.com"));
    headers.insert(
        "access-control-request-method",
        HeaderValue::from_static("POST"),
    );

    let response = cors_preflight_response(&config, &headers, &Method::OPTIONS);

    assert!(response.is_none());
}

#[test]
fn with_cors_headers_adds_loopback_origin_to_actual_response() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:9021"));
    let response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("response");

    let response = with_cors_headers(&config, &headers, response);

    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://127.0.0.1:9021")
    );
}

#[test]
fn anthropic_upstream_auth_accepts_authorization_bearer_fallback() {
    let config = config_without_local();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer anthropic-request-key"),
    );

    let request_auth =
        resolve_request_auth(&config, &headers, "/v1/messages").expect("request auth");
    let auth = resolve_upstream_auth("anthropic", &upstream_without_key(), &request_auth)
        .expect("upstream auth")
        .expect("anthropic auth header");

    assert_eq!(auth.name, AUTHORIZATION);
    assert_eq!(
        auth.value.to_str().ok(),
        Some("Bearer anthropic-request-key")
    );
}

#[test]
fn anthropic_upstream_auth_defaults_to_x_api_key_for_non_native_inbound_requests() {
    let config = config_without_local();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer anthropic-request-key"),
    );

    let request_auth =
        resolve_request_auth(&config, &headers, "/v1/chat/completions").expect("request auth");
    let auth = resolve_upstream_auth("anthropic", &upstream_without_key(), &request_auth)
        .expect("upstream auth")
        .expect("anthropic auth header");

    assert_eq!(auth.name.as_str(), "x-api-key");
    assert_eq!(auth.value.to_str().ok(), Some("anthropic-request-key"));
}

#[test]
fn anthropic_upstream_auth_reuses_authorization_header_name_with_upstream_key() {
    let config = config_without_local();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer local-debug-key"),
    );

    let request_auth =
        resolve_request_auth(&config, &headers, "/v1/messages").expect("request auth");
    let mut upstream = upstream_without_key();
    upstream.api_key = Some("upstream-anthropic-key".to_string());
    let auth = resolve_upstream_auth("anthropic", &upstream, &request_auth)
        .expect("upstream auth")
        .expect("anthropic auth header");

    assert_eq!(auth.name, AUTHORIZATION);
    assert_eq!(
        auth.value.to_str().ok(),
        Some("Bearer upstream-anthropic-key")
    );
}

#[test]
fn anthropic_upstream_auth_reuses_x_api_key_header_name_with_upstream_key() {
    let config = config_without_local();
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_static("local-debug-key"));

    let request_auth =
        resolve_request_auth(&config, &headers, "/v1/messages").expect("request auth");
    let mut upstream = upstream_without_key();
    upstream.api_key = Some("upstream-anthropic-key".to_string());
    let auth = resolve_upstream_auth("anthropic", &upstream, &request_auth)
        .expect("upstream auth")
        .expect("anthropic auth header");

    assert_eq!(auth.name.as_str(), "x-api-key");
    assert_eq!(auth.value.to_str().ok(), Some("upstream-anthropic-key"));
}
