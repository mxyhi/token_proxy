use axum::{body::Bytes, http::StatusCode};

use super::utils::{is_retryable_status, is_retryable_transport_error_message};
use super::*;

#[test]
fn retryable_status_matches_proxy_policy() {
    assert!(is_retryable_status(StatusCode::BAD_REQUEST));
    assert!(is_retryable_status(StatusCode::PAYMENT_REQUIRED));
    assert!(is_retryable_status(StatusCode::FORBIDDEN));
    assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
    assert!(is_retryable_status(StatusCode::TEMPORARY_REDIRECT));
    assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
    assert!(is_retryable_status(StatusCode::UNAUTHORIZED));
    assert!(is_retryable_status(StatusCode::NOT_FOUND));
    assert!(is_retryable_status(StatusCode::REQUEST_TIMEOUT));
    assert!(is_retryable_status(StatusCode::UNPROCESSABLE_ENTITY));
    assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));
    assert!(is_retryable_status(StatusCode::from_u16(524).expect("524")));
}

#[test]
fn cooldown_status_matches_proxy_policy() {
    assert!(result::should_cooldown_retryable_status(
        StatusCode::UNAUTHORIZED
    ));
    assert!(result::should_cooldown_retryable_status(
        StatusCode::FORBIDDEN
    ));
    assert!(result::should_cooldown_retryable_status(
        StatusCode::PAYMENT_REQUIRED
    ));
    // 413 全局 NextOnly。
    assert!(result::is_next_only_retryable_status(
        StatusCode::PAYLOAD_TOO_LARGE,
        "openai",
        None
    ));
    // 固定账户 codex/xai：鉴权/语义类 NextOnly，禁止 same-upstream 原位重放。
    assert!(result::is_next_only_retryable_status(
        StatusCode::UNAUTHORIZED,
        "codex",
        Some("acct-a")
    ));
    assert!(result::is_next_only_retryable_status(
        StatusCode::FORBIDDEN,
        "xai",
        Some("acct-b")
    ));
    assert!(result::is_next_only_retryable_status(
        StatusCode::PAYMENT_REQUIRED,
        "xai",
        Some("acct-b")
    ));
    assert!(result::is_next_only_retryable_status(
        StatusCode::BAD_REQUEST,
        "codex",
        Some("acct-a")
    ));
    // API-key / 无 account_id：保持 SameThenNext。
    assert!(!result::is_next_only_retryable_status(
        StatusCode::UNAUTHORIZED,
        "openai",
        None
    ));
    assert!(!result::is_next_only_retryable_status(
        StatusCode::BAD_REQUEST,
        "openai-response",
        None
    ));
    assert!(!result::is_next_only_retryable_status(
        StatusCode::FORBIDDEN,
        "codex",
        None
    ));
    // Kiro 不经本函数标 NextOnly（kiro_result 自管）。
    assert!(!result::is_next_only_retryable_status(
        StatusCode::UNAUTHORIZED,
        "kiro",
        Some("kiro-a")
    ));
    assert!(!result::is_next_only_retryable_status(
        StatusCode::TOO_MANY_REQUESTS,
        "codex",
        Some("acct-a")
    ));
    assert!(!result::is_next_only_retryable_status(
        StatusCode::INTERNAL_SERVER_ERROR,
        "codex",
        Some("acct-a")
    ));
    assert!(result::should_cooldown_retryable_status(
        StatusCode::REQUEST_TIMEOUT
    ));
    assert!(result::should_cooldown_retryable_status(
        StatusCode::TOO_MANY_REQUESTS
    ));
    assert!(result::should_cooldown_retryable_status(
        StatusCode::GATEWAY_TIMEOUT
    ));
    assert!(result::should_cooldown_retryable_status(
        StatusCode::from_u16(524).expect("524")
    ));

    assert!(!result::should_cooldown_retryable_status(
        StatusCode::BAD_REQUEST
    ));
    assert!(!result::should_cooldown_retryable_status(
        StatusCode::NOT_FOUND
    ));
    assert!(!result::should_cooldown_retryable_status(
        StatusCode::UNPROCESSABLE_ENTITY
    ));
    assert!(!result::should_cooldown_retryable_status(
        StatusCode::TEMPORARY_REDIRECT
    ));
}

#[test]
fn retryable_transport_error_message_matches_persistent_proxy_failures() {
    for message in [
        "error sending request: username/password authentication failed",
        "error sending request: proxy authentication required",
        "tcp connect error: connection refused",
        "tcp connect error: no route to host",
        "tcp connect error: network is unreachable",
        "dns error: no such host",
    ] {
        assert!(
            is_retryable_transport_error_message(message),
            "{message} should trigger upstream failover"
        );
    }

    assert!(!is_retryable_transport_error_message(
        "request body serialization failed"
    ));
}

#[test]
fn extract_query_param_reads_key_value() {
    let value =
        utils::extract_query_param("/v1beta/models/x:generateContent?key=abc&foo=bar", "key");
    assert_eq!(value.as_deref(), Some("abc"));
}

#[test]
fn ensure_query_param_overrides_existing_value() {
    let url = "https://example.com/v1beta/models/x:generateContent?foo=bar&key=old";
    let updated = utils::ensure_query_param(url, "key", "new").expect("updated url");
    assert!(updated.contains("foo=bar"));
    assert!(updated.contains("key=new"));
    assert!(!updated.contains("key=old"));
}

#[test]
fn redact_query_param_value_hides_secret() {
    let message = "error sending request for url (https://example.com/path?key=SECRET&foo=bar)";
    let redacted = redact_query_param_value(message, "key");
    assert!(redacted.contains("key=***"));
    assert!(!redacted.contains("SECRET"));
    assert!(redacted.contains("foo=bar"));
}

#[test]
fn apply_header_overrides_sets_and_removes() {
    use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, HOST};
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-remove"),
        HeaderValue::from_static("value"),
    );
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer original"));
    headers.insert(
        HeaderName::from_static("x-keep"),
        HeaderValue::from_static("old"),
    );

    let overrides = vec![
        token_proxy_config::HeaderOverride {
            name: HeaderName::from_static("x-custom"),
            value: Some(HeaderValue::from_static("new")),
        },
        token_proxy_config::HeaderOverride {
            name: AUTHORIZATION,
            value: Some(HeaderValue::from_static("Bearer override")),
        },
        token_proxy_config::HeaderOverride {
            name: HeaderName::from_static("x-remove"),
            value: None,
        },
        token_proxy_config::HeaderOverride {
            name: HOST,
            value: Some(HeaderValue::from_static("skip.example.com")),
        },
        token_proxy_config::HeaderOverride {
            name: CONTENT_LENGTH,
            value: Some(HeaderValue::from_static("123")),
        },
    ];

    request::apply_header_overrides(&mut headers, &overrides);

    assert_eq!(
        headers.get("x-custom").and_then(|v| v.to_str().ok()),
        Some("new")
    );
    assert_eq!(
        headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()),
        Some("Bearer override")
    );
    assert!(!headers.contains_key("x-remove"));
    // hop-by-hop/host/content-length must stay untouched/removed
    assert!(!headers.contains_key(HOST));
    assert!(!headers.contains_key(CONTENT_LENGTH));
}

#[test]
fn mapped_model_reasoning_suffix_is_stripped_and_becomes_effort() {
    let (model, effort) =
        normalize_mapped_model_reasoning_suffix(Some("gpt-4.1-reasoning-high".to_string()), None);
    assert_eq!(model.as_deref(), Some("gpt-4.1"));
    assert_eq!(effort.as_deref(), Some("high"));
}

#[test]
fn mapped_model_reasoning_suffix_does_not_override_existing_effort() {
    let (model, effort) = normalize_mapped_model_reasoning_suffix(
        Some("gpt-4.1-reasoning-high".to_string()),
        Some("low".to_string()),
    );
    assert_eq!(model.as_deref(), Some("gpt-4.1"));
    assert_eq!(effort.as_deref(), Some("low"));
}

#[test]
fn xai_routes_compact_and_media_to_official_api() {
    for path in [
        "/v1/responses/compact",
        "/v1/responses/compact?mode=test",
        "/v1/images/generations",
        "/v1/images/edits",
        "/v1/videos/video-123",
        "/v1/videos/video-123/content?download=1",
    ] {
        assert!(
            prepare::xai_request_url(path).starts_with("https://api.x.ai/v1/"),
            "path={path}"
        );
    }
    assert_eq!(
        prepare::xai_request_url("/v1/responses"),
        "https://cli-chat-proxy.grok.com/v1/responses"
    );
}

#[test]
fn xai_official_api_headers_keep_bearer_and_remove_cli_identity() {
    use axum::http::{header, HeaderMap, HeaderValue};

    let body = ReplayableBody::from_bytes(Default::default());
    for path in [
        "/v1/responses/compact",
        "/v1/images/generations",
        "/v1/videos/video-123",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer override"),
        );
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static(token_proxy_account_xai::CLI_USER_AGENT),
        );
        headers.insert(
            token_proxy_account_xai::CLI_TOKEN_AUTH_HEADER,
            HeaderValue::from_static(token_proxy_account_xai::CLI_TOKEN_AUTH_VALUE),
        );
        headers.insert("x-grok-conv-id", HeaderValue::from_static("conversation"));
        let mut protected = HeaderMap::new();
        protected.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer account-token"),
        );

        prepare::enforce_xai_request_headers(path, &body, true, Some(&protected), &mut headers);

        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer account-token")
        );
        assert!(!headers.contains_key(token_proxy_account_xai::CLI_TOKEN_AUTH_HEADER));
        assert!(!headers.contains_key(token_proxy_account_xai::CLI_CLIENT_VERSION_HEADER));
        assert!(!headers.contains_key("x-grok-conv-id"));
        assert!(!headers.contains_key(header::USER_AGENT));
        assert_eq!(
            headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
    }
}

#[test]
fn xai_image_edits_preserves_multipart_content_type() {
    use axum::http::{header, HeaderMap, HeaderValue};

    let body = ReplayableBody::from_bytes(Bytes::from_static(
        b"--xai-boundary\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit\r\n--xai-boundary--\r\n",
    ));
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("multipart/form-data; boundary=xai-boundary"),
    );

    prepare::enforce_xai_request_headers("/v1/images/edits", &body, false, None, &mut headers);

    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("multipart/form-data; boundary=xai-boundary")
    );
    assert_eq!(
        headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

#[test]
fn xai_video_content_uses_binary_accept_without_json_content_type() {
    use axum::http::{header, HeaderMap, HeaderValue};

    let body = ReplayableBody::from_bytes(Default::default());
    let mut headers = HeaderMap::new();
    prepare::enforce_xai_request_headers(
        "/v1/videos/video-123/content",
        &body,
        false,
        None,
        &mut headers,
    );

    assert_eq!(
        headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok()),
        Some("*/*")
    );
    assert!(!headers.contains_key(header::CONTENT_TYPE));

    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    prepare::enforce_xai_request_headers(
        "/v1/videos/video-123/content?download=1",
        &body,
        false,
        None,
        &mut headers,
    );
    assert_eq!(
        headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok()),
        Some("*/*")
    );
}

#[test]
fn xai_cli_responses_headers_include_identity_and_conversation() {
    use axum::http::{header, HeaderMap};

    let body =
        ReplayableBody::from_bytes(Bytes::from_static(br#"{"prompt_cache_key":"session-123"}"#));
    let mut headers = HeaderMap::new();
    prepare::enforce_xai_request_headers("/v1/responses", &body, true, None, &mut headers);

    assert_eq!(
        headers
            .get(token_proxy_account_xai::CLI_TOKEN_AUTH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(token_proxy_account_xai::CLI_TOKEN_AUTH_VALUE)
    );
    assert_eq!(
        headers
            .get("x-grok-conv-id")
            .and_then(|value| value.to_str().ok()),
        Some("session-123")
    );
    assert_eq!(
        headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some(token_proxy_account_xai::CLI_USER_AGENT)
    );
}
