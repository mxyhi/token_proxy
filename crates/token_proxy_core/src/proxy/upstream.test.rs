use super::utils::is_retryable_status;
use super::*;

#[test]
fn retryable_status_matches_proxy_policy() {
    assert!(is_retryable_status(StatusCode::BAD_REQUEST));
    assert!(is_retryable_status(StatusCode::FORBIDDEN));
    assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
    assert!(is_retryable_status(StatusCode::TEMPORARY_REDIRECT));
    assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
    assert!(is_retryable_status(StatusCode::UNAUTHORIZED));
    assert!(is_retryable_status(StatusCode::NOT_FOUND));
    assert!(is_retryable_status(StatusCode::REQUEST_TIMEOUT));
    assert!(is_retryable_status(StatusCode::UNPROCESSABLE_ENTITY));
    assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));
    assert!(is_retryable_status(
        StatusCode::from_u16(524).expect("524")
    ));
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
        super::super::config::HeaderOverride {
            name: HeaderName::from_static("x-custom"),
            value: Some(HeaderValue::from_static("new")),
        },
        super::super::config::HeaderOverride {
            name: AUTHORIZATION,
            value: Some(HeaderValue::from_static("Bearer override")),
        },
        super::super::config::HeaderOverride {
            name: HeaderName::from_static("x-remove"),
            value: None,
        },
        super::super::config::HeaderOverride {
            name: HOST,
            value: Some(HeaderValue::from_static("skip.example.com")),
        },
        super::super::config::HeaderOverride {
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
fn antigravity_stream_path_defaults_to_alt_sse() {
    let meta = RequestMeta {
        stream: true,
        original_model: None,
        mapped_model: None,
        reasoning_effort: None,
        estimated_input_tokens: None,
    };
    let path = resolve_upstream_path_with_query("antigravity", "/v1/chat/completions", &meta);
    assert_eq!(path, format!("{ANTIGRAVITY_STREAM_PATH}?alt=sse"));
}
