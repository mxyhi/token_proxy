use super::*;
use crate::logging::LogLevel;
use crate::proxy::config::UpstreamRuntime;
use std::collections::HashMap;

fn config_with_local(key: &str) -> ProxyConfig {
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: Some(key.to_string()),
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 1024,
        retryable_failure_cooldown: std::time::Duration::from_secs(15),
        upstream_no_data_timeout: std::time::Duration::from_secs(120),
        upstream_strategy: crate::proxy::config::UpstreamStrategyRuntime::default(),
        upstreams: HashMap::new(),
        kiro_preferred_endpoint: None,
    }
}

fn config_without_local() -> ProxyConfig {
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: None,
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 1024,
        retryable_failure_cooldown: std::time::Duration::from_secs(15),
        upstream_no_data_timeout: std::time::Duration::from_secs(120),
        upstream_strategy: crate::proxy::config::UpstreamStrategyRuntime::default(),
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
fn local_auth_accepts_gemini_model_catalog_query_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(&config, &headers, "/v1beta/models", Some("key=local-key"));
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_gemini_count_tokens_query_key() {
    let config = config_with_local("local-key");
    let headers = HeaderMap::new();
    let result = ensure_local_auth(
        &config,
        &headers,
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
    let result = ensure_local_auth(&config, &headers, "/v1/chat/completions", None);
    assert!(result.is_ok());
}

#[test]
fn local_auth_accepts_lowercase_bearer_authorization() {
    let config = config_with_local("local-key");
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("bearer local-key"));
    let result = ensure_local_auth(&config, &headers, "/v1/chat/completions", None);
    assert!(result.is_ok());
}

#[test]
fn anthropic_upstream_auth_accepts_authorization_bearer_fallback() {
    let config = config_without_local();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer anthropic-request-key"),
    );

    let request_auth = resolve_request_auth(&config, &headers).expect("request auth");
    let auth = resolve_upstream_auth("anthropic", &upstream_without_key(), &request_auth)
        .expect("upstream auth")
        .expect("anthropic auth header");

    assert_eq!(auth.name.as_str(), "x-api-key");
    assert_eq!(auth.value.to_str().ok(), Some("anthropic-request-key"));
}
