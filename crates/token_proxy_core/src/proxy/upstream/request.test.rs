use super::*;
use axum::http::header::AUTHORIZATION;
use url::form_urlencoded;

fn gemini_upstream() -> UpstreamRuntime {
    UpstreamRuntime {
        id: "gemini-test".to_string(),
        selector_key: "gemini-test".to_string(),
        base_url: "https://generativelanguage.googleapis.com".to_string(),
        api_key: Some("upstream-gemini-key".to_string()),
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
fn resolve_gemini_upstream_uses_proxy_upload_target_without_leaking_upstream_key() {
    let target =
        "https://generativelanguage.googleapis.com/upload/resumable/session-1?upload_id=session-1";
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair(GEMINI_PROXY_UPLOAD_TARGET_QUERY, target)
        .append_pair(GEMINI_API_KEY_QUERY, "local-debug-key")
        .finish();
    let path = format!("/upload/v1beta/files?{query}");
    let request_auth = RequestAuth::default();

    let resolved = resolve_gemini_upstream(
        &gemini_upstream(),
        &request_auth,
        &path,
        "https://generativelanguage.googleapis.com/upload/v1beta/files",
    );
    let (upstream_url, auth) = match resolved {
        Ok(value) => value,
        Err(_) => panic!("resolve gemini upload target"),
    };

    assert_eq!(
        upstream_url,
        "https://generativelanguage.googleapis.com/upload/resumable/session-1?upload_id=session-1&key=upstream-gemini-key"
    );
    assert_eq!(auth.name.as_str(), "x-goog-api-key");
    assert_eq!(auth.value.to_str().ok(), Some("upstream-gemini-key"));
    assert!(!upstream_url.contains("local-debug-key"));
}

#[test]
fn resolve_gemini_upstream_rejects_proxy_upload_target_from_other_origin() {
    let target = "https://evil.example/upload/resumable/session-1?upload_id=session-1";
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair(GEMINI_PROXY_UPLOAD_TARGET_QUERY, target)
        .finish();
    let path = format!("/upload/v1beta/files?{query}");
    let request_auth = RequestAuth::default();

    let result = resolve_gemini_upstream(
        &gemini_upstream(),
        &request_auth,
        &path,
        "https://generativelanguage.googleapis.com/upload/v1beta/files",
    );

    assert!(result.is_err());
}

#[test]
fn anthropic_specific_headers_are_removed_for_responses_fallback() {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("interleaved-thinking-2025-05-14"),
    );
    headers.insert("x-custom", HeaderValue::from_static("keep"));

    let built = build_request_headers(
        "openai-response",
        "/v1/messages",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    assert!(!built.contains_key("anthropic-version"));
    assert!(!built.contains_key("anthropic-beta"));
    assert_eq!(
        built.get("x-custom").and_then(|v| v.to_str().ok()),
        Some("keep")
    );
}

#[test]
fn anthropic_specific_headers_are_preserved_for_native_anthropic() {
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("interleaved-thinking-2025-05-14"),
    );

    let built = build_request_headers(
        "anthropic",
        "/v1/messages",
        &headers,
        http::UpstreamAuthHeader {
            name: HeaderName::from_static("x-api-key"),
            value: HeaderValue::from_static("anthropic-upstream"),
        },
        None,
        None,
    );

    assert_eq!(
        built.get("anthropic-version").and_then(|v| v.to_str().ok()),
        Some("2023-06-01")
    );
    assert_eq!(
        built.get("anthropic-beta").and_then(|v| v.to_str().ok()),
        Some("interleaved-thinking-2025-05-14")
    );
}

#[test]
fn anthropic_stainless_headers_are_removed_for_responses_fallback() {
    let mut headers = HeaderMap::new();
    headers.insert("x-stainless-lang", HeaderValue::from_static("js"));
    headers.insert(
        "x-stainless-package-version",
        HeaderValue::from_static("1.2.3"),
    );
    headers.insert("x-custom", HeaderValue::from_static("keep"));

    let built = build_request_headers(
        "openai-response",
        "/v1/messages",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    assert!(!built.contains_key("x-stainless-lang"));
    assert!(!built.contains_key("x-stainless-package-version"));
    assert_eq!(
        built.get("x-custom").and_then(|v| v.to_str().ok()),
        Some("keep")
    );
}

#[test]
fn anthropic_stainless_headers_are_preserved_for_native_anthropic() {
    let mut headers = HeaderMap::new();
    headers.insert("x-stainless-lang", HeaderValue::from_static("js"));
    headers.insert(
        "x-stainless-package-version",
        HeaderValue::from_static("1.2.3"),
    );

    let built = build_request_headers(
        "anthropic",
        "/v1/messages",
        &headers,
        http::UpstreamAuthHeader {
            name: HeaderName::from_static("x-api-key"),
            value: HeaderValue::from_static("anthropic-upstream"),
        },
        None,
        None,
    );

    assert_eq!(
        built.get("x-stainless-lang").and_then(|v| v.to_str().ok()),
        Some("js")
    );
    assert_eq!(
        built
            .get("x-stainless-package-version")
            .and_then(|v| v.to_str().ok()),
        Some("1.2.3")
    );
}
