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

#[tokio::test]
async fn filters_prompt_cache_retention_for_openai_responses_upstream() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: true,
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-4o","prompt_cache_retention":"24h","input":"hi"}"#,
    ));

    let rewritten = maybe_filter_openai_responses_request_fields(
        "openai-response",
        &upstream,
        "/v1/responses?foo=bar",
        &body,
    )
    .await;
    let rewritten = match rewritten {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };

    let rewritten = rewritten.expect("should rewrite");
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert!(value.get("prompt_cache_retention").is_none());
    assert_eq!(value.get("model").and_then(Value::as_str), Some("gpt-4o"));
}

#[tokio::test]
async fn filter_prompt_cache_retention_is_noop_when_disabled() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-4o","prompt_cache_retention":"24h","input":"hi"}"#,
    ));

    let rewritten = maybe_filter_openai_responses_request_fields(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
    )
    .await;
    let rewritten = match rewritten {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };

    assert!(rewritten.is_none());
}

#[tokio::test]
async fn filters_safety_identifier_for_openai_responses_upstream() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: true,
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-4o","safety_identifier":"sid_1","input":"hi"}"#,
    ));

    let rewritten = maybe_filter_openai_responses_request_fields(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
    )
    .await;
    let rewritten = match rewritten {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };

    let rewritten = rewritten.expect("should rewrite");
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert!(value.get("safety_identifier").is_none());
    assert_eq!(value.get("prompt_cache_retention"), None);
    assert_eq!(value.get("model").and_then(Value::as_str), Some("gpt-4o"));
}

#[tokio::test]
async fn filter_safety_identifier_is_noop_when_disabled() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-4o","safety_identifier":"sid_1","input":"hi"}"#,
    ));

    let rewritten = maybe_filter_openai_responses_request_fields(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
    )
    .await;
    let rewritten = match rewritten {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };

    assert!(rewritten.is_none());
}

#[tokio::test]
async fn rewrites_developer_role_to_system_for_chat_upstream() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: true,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"glm-5","messages":[{"role":"developer","content":"be precise"},{"role":"user","content":"hi"}]}"#,
    ));

    let rewritten = match maybe_rewrite_developer_role_to_system(
        &upstream,
        "/v1/chat/completions",
        &body,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };
    let rewritten = rewritten.expect("should rewrite");

    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");
    let messages = value["messages"].as_array().expect("messages");

    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
}

#[tokio::test]
async fn rewrites_developer_role_to_system_for_responses_upstream() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: true,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"glm-5","input":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"be precise"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}]}"#,
    ));

    let rewritten =
        match maybe_rewrite_developer_role_to_system(&upstream, "/v1/responses", &body).await {
            Ok(value) => value,
            Err(_) => panic!("rewrite result"),
        };
    let rewritten = rewritten.expect("should rewrite");

    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");
    let input = value["input"].as_array().expect("input");

    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[1]["role"], "user");
}

#[tokio::test]
async fn developer_role_rewrite_is_noop_when_disabled() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"glm-5","messages":[{"role":"developer","content":"be precise"}]}"#,
    ));

    let rewritten = match maybe_rewrite_developer_role_to_system(
        &upstream,
        "/v1/chat/completions",
        &body,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };

    assert!(rewritten.is_none());
}

#[tokio::test]
async fn developer_role_rewrite_is_noop_for_bigmodel_chat_when_disabled() {
    let upstream = UpstreamRuntime {
        id: "bigmodel-chat".to_string(),
        selector_key: "bigmodel-chat".to_string(),
        base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"glm-5","messages":[{"role":"developer","content":"be precise"},{"role":"user","content":"hi"}]}"#,
    ));

    let rewritten = match maybe_rewrite_developer_role_to_system(
        &upstream,
        "/v1/chat/completions",
        &body,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };
    assert!(rewritten.is_none());
}

#[tokio::test]
async fn developer_role_rewrite_is_noop_for_bigmodel_responses_when_disabled() {
    let upstream = UpstreamRuntime {
        id: "bigmodel-responses".to_string(),
        selector_key: "bigmodel-responses".to_string(),
        base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
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
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"glm-5","input":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"be precise"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}]}"#,
    ));

    let rewritten =
        match maybe_rewrite_developer_role_to_system(&upstream, "/v1/responses", &body).await {
            Ok(value) => value,
            Err(_) => panic!("rewrite result"),
        };
    assert!(rewritten.is_none());
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
