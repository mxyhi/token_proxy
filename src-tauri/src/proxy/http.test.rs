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
        kiro_preferred_endpoint: None,
        antigravity_user_agent: None,
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
