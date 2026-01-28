use super::*;

#[tokio::test]
async fn filters_prompt_cache_retention_for_openai_responses_upstream() {
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: true,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
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
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
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
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: true,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
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
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
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
