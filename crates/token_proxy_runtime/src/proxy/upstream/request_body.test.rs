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
        api_key_headers: None,
        filter_prompt_cache_retention,
        filter_safety_identifier,
        rewrite_developer_role_to_system,
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

fn xai_meta(model: &str, stream: bool) -> RequestMeta {
    RequestMeta {
        client_ip: None,
        stream,
        original_model: Some(model.to_string()),
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    }
}

async fn transformed_xai_body(path: &str, body: &'static [u8], model: &str) -> Value {
    let upstream = test_upstream(false, false, false);
    let body = ReplayableBody::from_bytes(Bytes::from_static(body));
    let rewritten = match build_json_transformed_body(
        "xai",
        &upstream,
        path,
        &body,
        &xai_meta(model, true),
        None,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("xAI body should change"),
        Err(_) => panic!("xAI transform should succeed"),
    };
    let bytes = rewritten
        .read_bytes_if_small(4096)
        .await
        .expect("read transformed body")
        .expect("transformed bytes");
    serde_json::from_slice(&bytes).expect("transformed JSON")
}

async fn transformed_body_with_x_search(
    provider: &str,
    path: &str,
    body: &[u8],
    model: &str,
    enabled: bool,
) -> Value {
    let upstream = test_upstream(false, false, false);
    let original = ReplayableBody::from_bytes(Bytes::copy_from_slice(body));
    let rewritten = match build_json_transformed_body_with_headers(
        provider,
        &upstream,
        path,
        &original,
        &xai_meta(model, true),
        None,
        &axum::http::HeaderMap::new(),
        enabled,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("xAI X Search transform should succeed"),
    };
    let final_body = rewritten.as_ref().unwrap_or(&original);
    let bytes = final_body
        .read_bytes_if_small(4096)
        .await
        .expect("read transformed body")
        .expect("transformed bytes");
    serde_json::from_slice(&bytes).expect("transformed JSON")
}

#[tokio::test]
async fn xai_responses_does_not_inject_x_search_when_disabled() {
    let value = transformed_body_with_x_search(
        "xai",
        "/v1/responses",
        br#"{"model":"grok-4.5","input":"hi"}"#,
        "grok-4.5",
        false,
    )
    .await;

    assert!(value.get("tools").is_none());
}

#[tokio::test]
async fn xai_responses_injects_x_search_once_when_enabled() {
    let value = transformed_body_with_x_search(
        "xai",
        "/v1/responses",
        br#"{"model":"grok-4.5","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],"input":"hi"}"#,
        "grok-4.5",
        true,
    )
    .await;

    let tools = value["tools"].as_array().expect("tools");
    assert_eq!(
        tools
            .iter()
            .filter(|tool| tool["type"] == serde_json::json!("x_search"))
            .count(),
        1
    );
}

#[tokio::test]
async fn xai_responses_keeps_existing_x_search_without_duplication() {
    let value = transformed_body_with_x_search(
        "xai",
        "/v1/responses",
        br#"{"model":"grok-4.5","tools":[{"type":"x_search"}],"input":"hi"}"#,
        "grok-4.5",
        true,
    )
    .await;

    let tools = value["tools"].as_array().expect("tools");
    assert_eq!(
        tools
            .iter()
            .filter(|tool| tool["type"] == serde_json::json!("x_search"))
            .count(),
        1
    );
}

#[tokio::test]
async fn xai_responses_syncs_x_search_into_allowed_tools_without_duplication() {
    for allowed_tools in [
        r#"[{"type":"function","name":"lookup"}]"#,
        r#"[{"type":"function","name":"lookup"},{"type":"x_search"}]"#,
    ] {
        let payload = format!(
            r#"{{"model":"grok-4.5","tools":[{{"type":"function","name":"lookup","parameters":{{"type":"object"}}}}],"tool_choice":{{"type":"allowed_tools","tools":{allowed_tools}}},"input":"hi"}}"#
        );
        let value = transformed_body_with_x_search(
            "xai",
            "/v1/responses",
            payload.as_bytes(),
            "grok-4.5",
            true,
        )
        .await;

        let allowed = value["tool_choice"]["tools"]
            .as_array()
            .expect("allowed tools");
        assert_eq!(
            allowed
                .iter()
                .filter(|tool| tool["type"] == serde_json::json!("x_search"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn xai_x_search_injection_excludes_other_providers() {
    let openai = transformed_body_with_x_search(
        "openai-response",
        "/v1/responses",
        br#"{"model":"grok-4.5","input":"hi"}"#,
        "grok-4.5",
        true,
    )
    .await;

    assert!(openai.get("tools").is_none());
}

#[tokio::test]
async fn xai_x_search_injection_enables_internal_response_filter_without_client_tools() {
    let body =
        ReplayableBody::from_bytes(Bytes::from_static(br#"{"model":"grok-4.5","input":"hi"}"#));
    let mapping = match xai_client_tool_mapping("xai", "/v1/responses", &body, true).await {
        Ok(Some(mapping)) => mapping,
        Ok(None) => panic!("enabled X Search should create a response filter mapping"),
        Err(_) => panic!("response filter mapping should succeed"),
    };
    let payload = br#"{"output":[{"type":"custom_tool_call","name":"x_user_search","call_id":"xs_call_1"},{"type":"message","content":[]}]}"#;

    let (filtered, changed) =
        token_proxy_protocol::xai_client_tools::restore_json_payload(payload, &mapping)
            .expect("filter response");
    let value: Value = serde_json::from_slice(&filtered).expect("filtered JSON");

    assert!(changed);
    assert_eq!(value["output"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["output"][0]["type"], "message");
}

#[tokio::test]
async fn xai_responses_filters_unsupported_fields_and_orphaned_tool_controls() {
    let value = transformed_xai_body(
        "/v1/responses",
        br#"{"model":"grok-4.5","previous_response_id":"resp_1","prompt_cache_retention":"24h","safety_identifier":"sid","stream_options":{"include_usage":true},"prompt_cache_key":"session-1","tools":[],"tool_choice":"auto","parallel_tool_calls":true,"presence_penalty":1,"presencePenalty":1,"frequency_penalty":1,"frequencyPenalty":1,"stop":["done"],"input":"hi"}"#,
        "grok-4.5",
    )
    .await;

    for field in [
        "previous_response_id",
        "prompt_cache_retention",
        "safety_identifier",
        "stream_options",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "presence_penalty",
        "presencePenalty",
        "frequency_penalty",
        "frequencyPenalty",
        "stop",
    ] {
        assert!(value.get(field).is_none(), "field={field}");
    }
    assert_eq!(value["prompt_cache_key"], "session-1");
}

#[tokio::test]
async fn xai_responses_promotes_additional_tools_and_keeps_controls() {
    let value = transformed_xai_body(
        "/v1/responses",
        br#"{"model":"grok-4.3","previous_response_id":"resp_1","tools":[{"type":"function","name":"Read"}],"tool_choice":"auto","parallel_tool_calls":true,"input":[{"type":"message","role":"user","content":"hello"},{"type":"additional_tools","tools":[{"type":"function","name":"Bash"}]}]}"#,
        "grok-4.3",
    )
    .await;

    assert_eq!(value["tool_choice"], "auto");
    assert_eq!(value["parallel_tool_calls"], true);
    assert_eq!(value["tools"][0]["name"], "Read");
    assert_eq!(value["tools"][1]["name"], "Bash");
    assert_eq!(value["input"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["input"][0]["type"], "message");
    assert!(value.get("previous_response_id").is_none());
}

#[tokio::test]
async fn xai_responses_lowers_codex_client_tools_history_and_choice() {
    let value = transformed_xai_body(
        "/v1/responses",
        br#"{"model":"grok-4.5","tools":[{"type":"custom","name":"exec","format":{"type":"grammar"}},{"type":"tool_search"},{"type":"namespace","name":"team","tools":[{"type":"function","name":"send","parameters":{"type":"object"}}]}],"tool_choice":{"type":"custom","name":"exec"},"input":[{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"pwd"},{"type":"custom_tool_call_output","call_id":"c1","output":{"ok":true}},{"type":"tool_search_call","call_id":"s1","arguments":{"query":"git"}},{"type":"tool_search_output","call_id":"s1","output":{"groups":["git"]}},{"type":"function_call","call_id":"n1","namespace":"team","name":"send","arguments":"{}"}]}"#,
        "grok-4.5",
    )
    .await;

    assert_eq!(value["tools"][0]["type"], "function");
    assert!(value["tools"][0].get("format").is_none());
    assert_eq!(value["tools"][0]["parameters"]["required"][0], "input");
    assert_eq!(value["tools"][1]["type"], "function");
    assert_eq!(value["tools"][1]["name"], "tool_search");
    assert_eq!(value["tools"][2]["name"], "team__send");
    assert_eq!(value["tool_choice"]["type"], "function");
    assert_eq!(value["input"][0]["type"], "function_call");
    assert_eq!(value["input"][0]["arguments"], r#"{"input":"pwd"}"#);
    assert_eq!(value["input"][1]["type"], "function_call_output");
    assert_eq!(value["input"][1]["output"], r#"{"ok":true}"#);
    assert_eq!(value["input"][2]["type"], "function_call");
    assert_eq!(value["input"][2]["name"], "tool_search");
    assert_eq!(value["input"][4]["name"], "team__send");
    assert!(value["input"][4].get("namespace").is_none());
}

#[tokio::test]
async fn xai_responses_adds_object_type_to_root_tool_union_branches_only() {
    let value = transformed_xai_body(
        "/v1/responses",
        br#"{"model":"grok-4.5","tools":[{"type":"function","name":"crop","strict":true,"parameters":{"type":"object","oneOf":[{"required":["radius"]},{"required":["size"],"not":{"required":["radius"]}}],"properties":{"nested":{"oneOf":[{"required":["value"]},{}]}}}},{"type":"web_search","name":"search","parameters":{"type":"object","anyOf":[{"required":["query"]}]}}],"input":"crop"}"#,
        "grok-4.5",
    )
    .await;

    assert_eq!(
        value["tools"][0]["parameters"]["oneOf"][0]["type"],
        "object"
    );
    assert_eq!(
        value["tools"][0]["parameters"]["oneOf"][1]["type"],
        "object"
    );
    assert!(
        value["tools"][0]["parameters"]["properties"]["nested"]["oneOf"][0]
            .get("type")
            .is_none()
    );
    assert!(value["tools"][1]["parameters"]["anyOf"][0]
        .get("type")
        .is_none());
}

#[tokio::test]
async fn xai_normalizes_image_references_without_touching_chat_content_parts() {
    let value = transformed_xai_body(
        "/v1/images/edits",
        br#"{"model":"grok-imagine-image","image":{"type":"image_url","image_url":"https://example.com/a.png"},"images":[{"image_url":{"url":"https://example.com/b.png"}},{"url":"https://example.com/c.png","image_url":"https://example.com/ignored.png"}],"reference_images":[{"image_url":"https://example.com/d.png"}],"nested":{"image":{"image_url":"https://example.com/e.png"}},"content":[{"type":"image_url","image_url":{"url":"https://example.com/keep.png"}}]}"#,
        "grok-imagine-image",
    )
    .await;

    assert_eq!(value["image"]["url"], "https://example.com/a.png");
    assert!(value["image"].get("image_url").is_none());
    assert_eq!(value["images"][0]["url"], "https://example.com/b.png");
    assert_eq!(value["images"][1]["url"], "https://example.com/c.png");
    assert!(value["images"][1].get("image_url").is_none());
    assert_eq!(
        value["reference_images"][0]["url"],
        "https://example.com/d.png"
    );
    assert_eq!(value["nested"]["image"]["url"], "https://example.com/e.png");
    assert_eq!(
        value["content"][0]["image_url"]["url"],
        "https://example.com/keep.png"
    );
    assert!(value["content"][0].get("url").is_none());
}

#[tokio::test]
async fn xai_non_45_model_keeps_penalties() {
    let value = transformed_xai_body(
        "/v1/responses",
        br#"{"model":"grok-4.3","previous_response_id":"resp_1","presence_penalty":1,"frequency_penalty":1,"stop":["done"],"input":"hi"}"#,
        "grok-4.3",
    )
    .await;

    assert_eq!(value["presence_penalty"], 1);
    assert_eq!(value["frequency_penalty"], 1);
    assert_eq!(value["stop"][0], "done");
    assert!(value.get("previous_response_id").is_none());
}

#[tokio::test]
async fn xai_image_edits_multipart_body_is_not_json_transformed() {
    let upstream = test_upstream(false, false, false);
    let payload = Bytes::from_static(
        b"--xai-boundary\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-image\r\n--xai-boundary--\r\n",
    );
    let body = ReplayableBody::from_bytes(payload.clone());
    let mut meta = xai_meta("grok-imagine-image", false);
    meta.mapped_model = Some("grok-imagine-image-mapped".to_string());

    let transformed =
        match build_json_transformed_body("xai", &upstream, "/v1/images/edits", &body, &meta, None)
            .await
        {
            Ok(value) => value,
            Err(_) => panic!("multipart passthrough should succeed"),
        };

    assert!(transformed.is_none());
    assert_eq!(body.as_bytes(), &payload);
}

#[tokio::test]
async fn codex_responses_lite_header_and_metadata_force_parallel_tools_off() {
    let upstream = test_upstream(false, false, false);
    let mut header = axum::http::HeaderMap::new();
    header.insert(
        "x-openai-internal-codex-responses-lite",
        axum::http::HeaderValue::from_static("true"),
    );
    for (headers, payload) in [
        (
            header,
            br#"{"model":"gpt-5.6","input":"hi","parallel_tool_calls":true}"#.as_slice(),
        ),
        (
            axum::http::HeaderMap::new(),
            br#"{"model":"gpt-5.6","input":"hi","parallel_tool_calls":true,"client_metadata":{"ws_request_header_x_openai_internal_codex_responses_lite":"true"}}"#.as_slice(),
        ),
    ] {
        let body = ReplayableBody::from_bytes(Bytes::copy_from_slice(payload));
        let rewritten = match build_json_transformed_body_with_headers(
            "codex",
            &upstream,
            "/v1/responses",
            &body,
            &xai_meta("gpt-5.6", true),
            None,
            &headers,
            false,
        )
        .await
        {
            Ok(Some(rewritten)) => rewritten,
            Ok(None) => panic!("Responses Lite must change"),
            Err(_) => panic!("Responses Lite transform must succeed"),
        };
        let bytes = rewritten
            .read_bytes_if_small(4096)
            .await
            .expect("read")
            .expect("bytes");
        let value: Value = serde_json::from_slice(&bytes).expect("json");

        assert_eq!(value["parallel_tool_calls"], false);
    }
}

#[tokio::test]
async fn codex_non_lite_request_does_not_change_parallel_tool_calls() {
    let upstream = test_upstream(false, false, false);
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-5.6","input":"hi","parallel_tool_calls":true}"#,
    ));
    let rewritten = match build_json_transformed_body_with_headers(
        "codex",
        &upstream,
        "/v1/responses",
        &body,
        &xai_meta("gpt-5.6", true),
        None,
        &axum::http::HeaderMap::new(),
        false,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("non-lite transform inspection must succeed"),
    };

    assert!(rewritten.is_none());
}

#[tokio::test]
async fn normalizes_anthropic_one_meg_model_suffix_in_upstream_body() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: Some("claude-opus-4-6".to_string()),
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"claude-opus-4-6[1M][1m]","messages":[]}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "anthropic",
        &upstream,
        "/v1/messages",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    }
    .expect("should normalize model");
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read")
        .expect("bytes");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(value["model"], "claude-opus-4-6");
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
async fn strips_sampling_params_for_openai_responses_reasoning_model() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: Some("openai/gpt-5.5".to_string()),
        mapped_model: Some("gpt-5.5".to_string()),
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"openai/gpt-5.5","temperature":0.7,"top_p":0.9,"input":"hi"}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };
    let rewritten = rewritten.expect("should strip sampling params");
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(value.get("model").and_then(Value::as_str), Some("gpt-5.5"));
    assert!(value.get("temperature").is_none());
    assert!(value.get("top_p").is_none());
}

#[tokio::test]
async fn strips_sampling_params_for_openai_responses_reasoning_model_from_prefixed_original() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: Some("openai/gpt-5.5".to_string()),
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"openai/gpt-5.5","temperature":0.7,"top_p":0.9,"input":"hi"}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("rewrite result"),
    };
    let rewritten = rewritten.expect("should strip sampling params");
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(
        value.get("model").and_then(Value::as_str),
        Some("openai/gpt-5.5")
    );
    assert!(value.get("temperature").is_none());
    assert!(value.get("top_p").is_none());
}

#[tokio::test]
async fn rejects_large_openai_responses_reasoning_body_when_sampling_params_cannot_be_checked() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: Some("gpt-5.5".to_string()),
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let input = "x".repeat(REQUEST_FILTER_LIMIT_BYTES + 1);
    let body = ReplayableBody::from_bytes(Bytes::from(format!(
        r#"{{"model":"gpt-5.5","temperature":0.7,"input":"{input}"}}"#
    )));

    let result = build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
        &meta,
        None,
    )
    .await;

    match result {
        Err(AttemptOutcome::Fatal(response)) => {
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        }
        Ok(_) | Err(_) => panic!("large reasoning body should fail closed"),
    }
}

#[tokio::test]
async fn anthropic_count_tokens_removes_max_tokens_and_keeps_input_fields() {
    let upstream = test_upstream(false, false, false);
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"claude-sonnet-4-5","max_tokens":4096,"system":"guard","messages":[{"role":"user","content":"hi"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "anthropic",
        &upstream,
        "/v1/messages/count_tokens",
        &body,
        &xai_meta("claude-sonnet-4-5", false),
        None,
    )
    .await
    {
        Ok(Some(body)) => body,
        Ok(None) => panic!("body must change"),
        Err(_) => panic!("count_tokens filter must succeed"),
    };
    let bytes = rewritten
        .read_bytes_if_small(4096)
        .await
        .expect("read body")
        .expect("rewritten bytes");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert!(value.get("max_tokens").is_none());
    assert_eq!(value["model"], serde_json::json!("claude-sonnet-4-5"));
    assert_eq!(value["system"], serde_json::json!("guard"));
    assert_eq!(value["messages"][0]["content"], serde_json::json!("hi"));
    assert_eq!(value["tools"][0]["name"], serde_json::json!("lookup"));
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
async fn sanitizes_native_openai_responses_input_metadata() {
    let upstream = test_upstream(false, false, false);
    let meta = xai_meta("gpt-5.6-sol", false);
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{
            "model":"gpt-5.6-sol",
            "large":9007199254740993,
            "tools":[{"type":"namespace","name":"team","namespace":"keep-tool"}],
            "input":[
                {"type":"message","id":"item_message","namespace":"remove-message","role":"user","content":[{"type":"input_text","text":"hi","namespace":"keep-content"}]},
                {"type":"function_call","id":"item_call","namespace":"remove-call","call_id":"call_1","name":"exec","arguments":"{}"},
                {"type":"custom_tool_call","id":"fc_valid","namespace":"remove-custom","call_id":"call_2","name":"shell","input":"pwd"},
                {"type":"function_call_output","id":"item_output","namespace":"remove-output","call_id":"call_1","output":"ok"},
                {"type":"reasoning","id":"rs_valid","namespace":"remove-reasoning","summary":[]}
            ]
        }"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(Some(rewritten)) => rewritten,
        Ok(None) => panic!("request should change"),
        Err(_) => panic!("sanitize request should succeed"),
    };
    let bytes = rewritten
        .read_bytes_if_small(4096)
        .await
        .expect("read sanitized body")
        .expect("sanitized bytes");
    let value: Value = serde_json::from_slice(&bytes).expect("sanitized json");

    assert!(value["input"][0].get("id").is_none());
    assert!(value["input"][1].get("id").is_none());
    assert_eq!(value["input"][2]["id"], "fc_valid");
    assert_eq!(value["input"][3]["id"], "item_output");
    assert_eq!(value["input"][4]["id"], "rs_valid");
    for item in value["input"].as_array().expect("input array") {
        assert!(item.get("namespace").is_none());
    }
    assert_eq!(value["input"][0]["content"][0]["namespace"], "keep-content");
    assert_eq!(value["tools"][0]["namespace"], "keep-tool");
    assert_eq!(value["large"], 9_007_199_254_740_993_u64);
}

#[tokio::test]
async fn native_openai_responses_sanitizer_ignores_other_paths() {
    let upstream = test_upstream(false, false, false);
    let meta = xai_meta("gpt-5.6-sol", false);
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"message","id":"item_message","namespace":"keep"}]}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/chat/completions",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(rewritten) => rewritten,
        Err(_) => panic!("inspect request should succeed"),
    };

    assert!(rewritten.is_none());
}

#[tokio::test]
async fn sanitizes_openai_provider_native_responses_input_metadata() {
    let upstream = test_upstream(false, false, false);
    let meta = xai_meta("gpt-5.6-sol", false);
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"message","id":42,"namespace":"remove"}]}"#,
    ));

    let rewritten =
        match build_json_transformed_body("openai", &upstream, "/v1/responses", &body, &meta, None)
            .await
        {
            Ok(Some(rewritten)) => rewritten,
            Ok(None) => panic!("request should change"),
            Err(_) => panic!("sanitize request should succeed"),
        };
    let bytes = rewritten
        .read_bytes_if_small(4096)
        .await
        .expect("read sanitized body")
        .expect("sanitized bytes");
    let value: Value = serde_json::from_slice(&bytes).expect("sanitized json");

    assert!(value["input"][0].get("id").is_none());
    assert!(value["input"][0].get("namespace").is_none());
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

#[tokio::test]
async fn json_transform_pipeline_applies_reasoning_filters_and_role_rewrite_together() {
    let upstream = test_upstream(true, true, true);
    let meta = RequestMeta {
        client_ip: None,
        stream: true,
        original_model: Some("gpt-5".to_string()),
        mapped_model: None,
        reasoning_effort: Some("high".to_string()),
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-5-reasoning-high","prompt_cache_retention":"24h","safety_identifier":"sid_1","input":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"be precise"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}]}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("combined transform should rewrite"),
        Err(_) => panic!("transform result"),
    };
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");
    let input = value["input"].as_array().expect("input");

    assert_eq!(value.get("model").and_then(Value::as_str), Some("gpt-5"));
    assert_eq!(
        value
            .get("reasoning")
            .and_then(Value::as_object)
            .and_then(|reasoning| reasoning.get("effort"))
            .and_then(Value::as_str),
        Some("high")
    );
    assert!(value.get("prompt_cache_retention").is_none());
    assert!(value.get("safety_identifier").is_none());
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[1]["role"], "user");
}

#[tokio::test]
async fn openai_responses_grok_reasoning_effort_is_preserved() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: true,
        original_model: Some("grok-4.20".to_string()),
        mapped_model: Some("grok-4.20".to_string()),
        reasoning_effort: Some("high".to_string()),
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"grok-4.20","input":"hi","reasoning":{"effort":"high","summary":"auto"}}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai-response",
        &upstream,
        "/v1/responses",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("Grok effort should remain in outgoing body"),
        Err(_) => panic!("transform result"),
    };
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(value["model"], "grok-4.20");
    assert_eq!(value["reasoning"]["effort"], "high");
    assert_eq!(value["reasoning"]["summary"], "auto");
}

#[tokio::test]
async fn openai_chat_reasoning_effort_normalizes_glm_xhigh_to_max() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: Some("z-ai/glm-5.1-xhigh".to_string()),
        mapped_model: Some("glm-5.1".to_string()),
        reasoning_effort: Some("xhigh".to_string()),
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"z-ai/glm-5.1-xhigh","messages":[{"role":"user","content":"hi"}]}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai",
        &upstream,
        "/v1/chat/completions",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("GLM effort should rewrite body"),
        Err(_) => panic!("transform result"),
    };
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(value.get("model").and_then(Value::as_str), Some("glm-5.1"));
    assert_eq!(
        value.get("reasoning_effort").and_then(Value::as_str),
        Some("max")
    );
}

#[tokio::test]
async fn openai_chat_reasoning_effort_keeps_non_glm_xhigh() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: Some("openai/gpt-5.5-xhigh".to_string()),
        mapped_model: Some("gpt-5.5".to_string()),
        reasoning_effort: Some("xhigh".to_string()),
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"openai/gpt-5.5-xhigh","messages":[{"role":"user","content":"hi"}]}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "openai",
        &upstream,
        "/v1/chat/completions",
        &body,
        &meta,
        None,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("reasoning effort should rewrite body"),
        Err(_) => panic!("transform result"),
    };
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(
        value.get("reasoning_effort").and_then(Value::as_str),
        Some("xhigh")
    );
}

#[tokio::test]
async fn injects_codex_installation_id_into_client_metadata() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: true,
        original_model: Some("gpt-5-codex".to_string()),
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-5-codex","input":"hi","client_metadata":{"source":"token_proxy"}}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "codex",
        &upstream,
        "/backend-api/codex/responses",
        &body,
        &meta,
        Some("device-123"),
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("Codex installation id should rewrite body"),
        Err(_) => panic!("transform result"),
    };
    let bytes = rewritten
        .read_bytes_if_small(1024)
        .await
        .expect("read rewritten bytes")
        .expect("rewritten body exists");
    let value: Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(
        value["client_metadata"]["x-codex-installation-id"].as_str(),
        Some("device-123")
    );
    assert_eq!(
        value["client_metadata"]["source"].as_str(),
        Some("token_proxy")
    );
}

#[tokio::test]
async fn preserves_existing_codex_installation_id() {
    let upstream = test_upstream(false, false, false);
    let meta = RequestMeta {
        client_ip: None,
        stream: true,
        original_model: Some("gpt-5-codex".to_string()),
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        billing: Default::default(),
    };
    let body = ReplayableBody::from_bytes(Bytes::from_static(
        br#"{"model":"gpt-5-codex","input":"hi","client_metadata":{"x-codex-installation-id":"existing-device"}}"#,
    ));

    let rewritten = match build_json_transformed_body(
        "codex",
        &upstream,
        "/backend-api/codex/responses",
        &body,
        &meta,
        Some("device-123"),
    )
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("transform result"),
    };

    assert!(rewritten.is_none());
}
