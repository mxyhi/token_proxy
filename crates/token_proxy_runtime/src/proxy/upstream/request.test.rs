use super::*;
use axum::http::header::{ACCEPT_ENCODING, AUTHORIZATION};
use url::form_urlencoded;

fn gemini_upstream() -> UpstreamRuntime {
    UpstreamRuntime {
        id: "gemini-test".to_string(),
        selector_key: "gemini-test".to_string(),
        base_url: "https://generativelanguage.googleapis.com".to_string(),
        api_key: Some("upstream-gemini-key".to_string()),
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        xai_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        available_models: Vec::new(),
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
fn upstream_headers_force_identity_accept_encoding() {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
    headers.insert("x-custom", HeaderValue::from_static("keep"));

    let built = build_request_headers(
        "openai-response",
        "/v1/responses",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    assert_eq!(
        built
            .get(ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("identity")
    );
    assert_eq!(
        built.get("x-custom").and_then(|value| value.to_str().ok()),
        Some("keep")
    );
}

#[test]
fn header_overrides_cannot_reenable_compressed_upstream_bodies() {
    let headers = HeaderMap::new();
    let overrides = [HeaderOverride {
        name: ACCEPT_ENCODING,
        value: Some(HeaderValue::from_static("gzip")),
    }];

    let built = build_request_headers(
        "openai-response",
        "/v1/responses",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        Some(&overrides),
    );

    assert_eq!(
        built
            .get(ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("identity")
    );
}

#[test]
fn codex_headers_do_not_send_version_header() {
    let headers = HeaderMap::new();

    let built = build_request_headers(
        "codex",
        "/v1/responses",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    assert!(!built.contains_key("version"));
    assert!(!built.contains_key("openai-beta"));
    assert!(!built.contains_key("session_id"));
    assert_eq!(
        built
            .get("x-codex-beta-features")
            .and_then(|value| value.to_str().ok()),
        Some("remote_compaction_v2")
    );
    let user_agent = built
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .expect("codex user-agent");
    assert_eq!(user_agent, "codex_cli_rs/0.144.1 (token_proxy)");
    assert_eq!(
        built
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs")
    );
    let session_id = built
        .get("session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session-id");
    let thread_id = built
        .get("thread-id")
        .and_then(|value| value.to_str().ok())
        .expect("thread-id");
    assert!(!session_id.is_empty());
    assert!(!thread_id.is_empty());
    assert_eq!(
        built
            .get("x-client-request-id")
            .and_then(|value| value.to_str().ok()),
        Some(thread_id)
    );
}

#[test]
fn codex_headers_override_non_codex_client_identity() {
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_static("curl/8.7.1"));
    headers.insert("originator", HeaderValue::from_static("unknown-client"));
    headers.insert("openai-beta", HeaderValue::from_static("assistants=v2"));
    headers.insert("session-id", HeaderValue::from_static("session-inbound"));
    headers.insert("thread-id", HeaderValue::from_static("thread-inbound"));
    headers.insert(
        "x-client-request-id",
        HeaderValue::from_static("request-inbound"),
    );
    headers.insert("accept", HeaderValue::from_static("application/json"));
    headers.insert(
        "x-codex-beta-features",
        HeaderValue::from_static("client_feature"),
    );

    let built = build_request_headers(
        "codex",
        "/v1/responses",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    let user_agent = built
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .expect("codex user-agent");
    assert!(user_agent.starts_with("codex_cli_rs/"));
    assert_eq!(
        built
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs")
    );
    assert!(!built.contains_key("openai-beta"));
    assert_eq!(
        built
            .get("session-id")
            .and_then(|value| value.to_str().ok()),
        Some("session-inbound")
    );
    assert_eq!(
        built.get("thread-id").and_then(|value| value.to_str().ok()),
        Some("thread-inbound")
    );
    assert_eq!(
        built
            .get("x-client-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("request-inbound")
    );
    assert_eq!(
        built.get("accept").and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(!built.contains_key("connection"));
    assert!(!built.contains_key("version"));
    assert_eq!(
        built
            .get("x-codex-beta-features")
            .and_then(|value| value.to_str().ok()),
        Some("client_feature")
    );
}

#[test]
fn codex_headers_replace_native_identity_below_minimum_version() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "user-agent",
        HeaderValue::from_static("codex_cli_rs/0.135.0 (Mac OS 15.5.0; arm64) codex-cli"),
    );
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert("session-id", HeaderValue::from_static("native-session"));
    headers.insert("thread-id", HeaderValue::from_static("native-thread"));
    headers.insert(
        "openai-beta",
        HeaderValue::from_static("responses=experimental"),
    );

    let built = build_request_headers(
        "codex",
        "/v1/responses",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    assert_eq!(
        built
            .get("user-agent")
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs/0.144.1 (token_proxy)")
    );
    assert_eq!(
        built
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs")
    );
    assert_eq!(
        built
            .get("session-id")
            .and_then(|value| value.to_str().ok()),
        Some("native-session")
    );
    assert_eq!(
        built.get("thread-id").and_then(|value| value.to_str().ok()),
        Some("native-thread")
    );
    assert_eq!(
        built
            .get("x-client-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("native-thread")
    );
    assert!(!built.contains_key("openai-beta"));
    assert!(!built.contains_key("connection"));
}

#[test]
fn codex_headers_pair_originator_with_final_official_user_agent() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "user-agent",
        HeaderValue::from_static(
            "codex-tui/0.145.0 (Mac OS X 15.5.0; arm64) iTerm (codex-tui; 0.145.0)",
        ),
    );
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));

    let built = build_request_headers(
        "codex",
        "/v1/responses",
        &headers,
        http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value: HeaderValue::from_static("Bearer upstream"),
        },
        None,
        None,
    );

    assert_eq!(
        built
            .get("user-agent")
            .and_then(|value| value.to_str().ok()),
        Some("codex-tui/0.145.0 (Mac OS X 15.5.0; arm64) iTerm (codex-tui; 0.145.0)")
    );
    assert_eq!(
        built
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex-tui")
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
