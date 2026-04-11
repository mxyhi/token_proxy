use super::*;
use axum::body::Bytes;
use serde_json::Value;

fn test_upstream(
    filter_prompt_cache_retention: bool,
    filter_safety_identifier: bool,
    rewrite_developer_role_to_system: bool,
) -> UpstreamRuntime {
    UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention,
        filter_safety_identifier,
        rewrite_developer_role_to_system,
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

#[tokio::test]
async fn filters_prompt_cache_retention_for_openai_responses_upstream() {
    let upstream = test_upstream(true, false, false);
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
    let upstream = test_upstream(false, false, false);
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
    let upstream = test_upstream(false, true, false);
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
    let upstream = test_upstream(false, false, false);
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
    let upstream = test_upstream(false, false, true);
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
    let upstream = test_upstream(false, false, true);
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
    let upstream = test_upstream(false, false, false);
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
    let mut upstream = test_upstream(false, false, false);
    upstream.id = "bigmodel-chat".to_string();
    upstream.selector_key = "bigmodel-chat".to_string();
    upstream.base_url = "https://open.bigmodel.cn/api/paas/v4".to_string();
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
    let mut upstream = test_upstream(false, false, false);
    upstream.id = "bigmodel-responses".to_string();
    upstream.selector_key = "bigmodel-responses".to_string();
    upstream.base_url = "https://open.bigmodel.cn/api/paas/v4".to_string();
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
