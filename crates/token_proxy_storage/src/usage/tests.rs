use super::*;
use serde_json::json;

#[test]
fn sparse_token_updates_do_not_keep_stale_reported_totals() {
    let mut collector = SseUsageCollector::new();
    for usage in [
        json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}),
        json!({"completion_tokens":3}),
    ] {
        collector
            .push_chunk(format!("data: {}\n\n", json!({"usage":usage})).as_bytes())
            .unwrap();
    }
    assert_eq!(collector.finish().usage.unwrap().total_tokens, Some(13));
}

#[test]
fn image_and_video_observation_retains_parameters_without_payloads_or_inferred_costs() {
    let image = json!({"data":[{"b64_json":"private image","size":"1024x1024","quality":"high"}],"usage":{"cost":{"usd_ticks":9},"input_tokens":1,"output_tokens":2}});
    let snapshot = extract_usage_from_response(&Bytes::from(image.to_string()));
    let usage = snapshot.usage_json.unwrap();
    assert_eq!(usage["cost"]["usd_ticks"], 9);
    assert_eq!(usage["_proxy_observed"]["tools"]["image_count"], 1);
    assert_eq!(usage["_proxy_observed"]["image_details"]["quality"], "high");
    assert!(!usage.to_string().contains("private"));
    let video = json!({"video":{"url":"private video","duration":3.5,"resolution":"720p"}});
    let snapshot = extract_usage_from_response(&Bytes::from(video.to_string()));
    assert_eq!(snapshot.billable_usage, BillableUsage::default());
    let usage = snapshot.usage_json.unwrap();
    assert_eq!(
        usage["_proxy_observed"]["response_metadata"]["video"]["duration"],
        3.5
    );
    assert!(!usage.to_string().contains("private"));
}

#[test]
fn multiline_usage_and_metadata_only_frames_preserve_token_snapshot() {
    let mut collector = SseUsageCollector::new();
    collector
        .push_chunk(b"data: {\"usage\":\ndata: {\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n")
        .unwrap();
    collector
        .push_chunk(b"data: {\"service_tier\":\"priority\",\"prompt_cache_retention\":\"24h\"}\n\n")
        .unwrap();
    let snapshot = collector.finish();
    assert_eq!(snapshot.service_tier.as_deref(), Some("priority"));
    assert_eq!(snapshot.usage.unwrap().total_tokens, Some(5));
    assert_eq!(
        snapshot.usage_json.unwrap()["_proxy_observed"]["response_metadata"]
            ["prompt_cache_retention"],
        "24h"
    );
}

#[test]
fn response_metadata_and_completed_tools_survive_empty_terminal_output() {
    let item = json!({"type":"web_search_call","id":"ws_1","status":"completed", "action":{"type":"search","queries":["private query"],"sources":[{"url":"private url"}]}});
    let terminal = json!({"type":"response.completed","response":{
        "usage":{"input_tokens":10,"output_tokens":2},"output":[],
        "prompt_cache_retention":"24h","prompt_cache_options":{"mode":"auto"},
        "tool_usage":{"web_search":{"num_requests":1}}
    }});
    let mut collector = SseUsageCollector::new();
    for event in [
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"id":"failed","type":"web_search_call","status":"failed"}}),
        terminal.clone(),
    ] {
        collector
            .push_chunk(format!("data: {event}\n\n").as_bytes())
            .unwrap();
    }
    let snapshot = collector.finish();
    let usage = snapshot.usage_json.unwrap();
    assert_eq!(usage["_proxy_observed"]["tools"]["web_search_calls"], 1);
    assert_eq!(usage["_proxy_observed"]["tools"]["web_search_queries"], 1);
    assert_eq!(
        usage["_proxy_observed"]["response_metadata"]["prompt_cache_retention"],
        "24h"
    );
    assert_eq!(
        usage["_proxy_observed"]["response_metadata"]["tool_usage"]["web_search"]["num_requests"],
        1
    );
    assert!(!usage.to_string().contains("private"));
    assert_eq!(snapshot.billable_usage.output_tokens, 2);
    let mut nonstream = terminal["response"].clone();
    nonstream["output"] = json!([item]);
    assert_eq!(
        extract_usage_from_response(&Bytes::from(nonstream.to_string()))
            .usage_json
            .unwrap(),
        usage
    );
}

#[test]
fn claude_tool_observation_keeps_reported_zero_separate_from_completed_searches() {
    let mut collector = SseUsageCollector::new();
    for event in [
        json!({"type":"message_start","message":{"usage":{"input_tokens":1,"server_tool_use":{"web_search_requests":0}}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srv_1","name":"web_search","input":{"query":"private"}}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srv_1","content":[{"type":"web_search_result","url":"private"}]}}),
        json!({"type":"message_delta","usage":{"output_tokens":2}}),
    ] {
        collector
            .push_chunk(format!("data: {event}\n\n").as_bytes())
            .unwrap();
    }
    let usage = collector.finish().usage_json.unwrap();
    assert_eq!(usage["server_tool_use"]["web_search_requests"], 0);
    assert_eq!(usage["_proxy_observed"]["tools"]["web_search_calls"], 1);
    assert_eq!(usage["_proxy_observed"]["tools"]["num_search_results"], 1);
    assert!(!usage.to_string().contains("private"));
}

#[test]
fn gemini_tool_observation_deduplicates_call_ids_and_partial_arguments() {
    let mut collector = SseUsageCollector::new();
    for call in [
        json!({"id":"c1","name":"lookup","partialArgs":[{"jsonPath":"$.q","stringValue":"private"}],"willContinue":true}),
        json!({"id":"c1","partialArgs":[{"jsonPath":"$.q","stringValue":" second"}],"willContinue":false}),
        json!({"id":"c1","name":"lookup","args":{"q":"private second"}}),
        json!({}),
        json!({"id":"c2","name":"lookup","args":{}}),
    ] {
        let event = json!({"candidates":[{"index":0,"content":{"parts":[{"functionCall":call}]}}]});
        collector
            .push_chunk(format!("data: {event}\n\n").as_bytes())
            .unwrap();
    }
    collector
        .push_chunk(
            b"data: {\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":1}}\n\n",
        )
        .unwrap();
    let usage = collector.finish().usage_json.unwrap();
    assert_eq!(usage["_proxy_observed"]["tools"]["tool_calls_requested"], 2);
    assert!(!usage.to_string().contains("private"));
}

#[test]
fn sparse_anthropic_usage_preserves_input_cache_and_structured_details() {
    let mut collector = SseUsageCollector::new();
    for event in [
        json!({"type":"message_start","message":{"usage":{
            "input_tokens":10,"output_tokens":0,"cache_read_input_tokens":4,
            "cache_creation_input_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":5},
            "server_tool_use":{"web_search_requests":0,"web_fetch_requests":1}
        }}}),
        json!({"type":"message_delta","usage":{
            "output_tokens":2,"output_tokens_details":{"thinking_tokens":0},
            "server_tool_use":{"web_search_requests":2}
        }}),
        json!({"type":"message_delta","usage":{"output_tokens":2}}),
    ] {
        collector
            .push_chunk(format!("data: {event}\n\n").as_bytes())
            .unwrap();
    }
    let snapshot = collector.finish();
    assert_eq!(snapshot.usage.as_ref().unwrap().total_tokens, Some(21));
    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 10);
    assert_eq!(snapshot.billable_usage.cache_read_tokens, 4);
    assert_eq!(snapshot.billable_usage.cache_write_5m_tokens, 5);
    assert_eq!(snapshot.billable_usage.output_tokens, 2);
    let raw = snapshot.usage_json.unwrap();
    assert_eq!(
        raw["server_tool_use"],
        json!({"web_search_requests":2,"web_fetch_requests":1})
    );
    assert_eq!(raw["output_tokens_details"]["thinking_tokens"], 0);
}

#[test]
fn sparse_openai_usage_keeps_tokens_after_null_and_metadata_only_events() {
    let mut collector = SseUsageCollector::new();
    for usage in [
        json!({"prompt_tokens":10,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":4}}),
        Value::Null,
        json!({"prompt_tokens_details":{"cached_tokens":0},"cost":{"usd_ticks":13}}),
    ] {
        collector
            .push_chunk(format!("data: {}\n\n", json!({"usage":usage})).as_bytes())
            .unwrap();
    }
    let snapshot = collector.finish();
    assert_eq!(snapshot.usage.as_ref().unwrap().total_tokens, Some(12));
    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 10);
    assert_eq!(snapshot.billable_usage.cache_read_tokens, 0);
    assert_eq!(snapshot.usage_json.unwrap()["cost"]["usd_ticks"], 13);
}

#[test]
fn claude_speed_is_effective_tier_and_survives_sparse_usage() {
    let mut collector = SseUsageCollector::new();
    for event in [
        json!({"type":"message_start","message":{"usage":{"input_tokens":10,"speed":"fast"}}}),
        json!({"type":"message_delta","usage":{"output_tokens":3}}),
    ] {
        collector
            .push_chunk(format!("data: {event}\n\n").as_bytes())
            .unwrap();
    }
    assert_eq!(collector.finish().service_tier.as_deref(), Some("fast"));
    collector.push_chunk(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":4,\"speed\":\"standard\"}}\n\n").unwrap();
    assert_eq!(collector.finish().service_tier.as_deref(), Some("standard"));
    for (speed, expected) in [
        ("fast", "fast"),
        ("standard", "standard"),
        ("unknown", "priority"),
    ] {
        let body = json!({"service_tier":"priority","usage":{"input_tokens":10,"output_tokens":3,"speed":speed}});
        assert_eq!(
            extract_usage_from_response(&Bytes::from(body.to_string()))
                .service_tier
                .as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn gemini_usage_merges_partial_snapshots_without_adding_duplicates() {
    for nested in [false, true] {
        let mut collector = SseUsageCollector::new();
        for metadata in [
            json!({"promptTokenCount":16,"cachedContentTokenCount":4}),
            json!({"candidatesTokenCount":5,"thoughtsTokenCount":42}),
            json!({"candidatesTokenCount":5,"thoughtsTokenCount":42}),
            json!({"candidatesTokenCount":0,"cachedContentTokenCount":0}),
        ] {
            let envelope = if nested {
                json!({"response":{"usageMetadata":metadata}})
            } else {
                json!({"usageMetadata":metadata})
            };
            collector
                .push_chunk(format!("data: {envelope}\n\n").as_bytes())
                .unwrap();
        }
        let snapshot = collector.finish();
        let usage = snapshot.usage.expect("usage");
        assert_eq!(usage.input_tokens, Some(16));
        assert_eq!(usage.output_tokens, Some(42));
        assert_eq!(usage.total_tokens, Some(58));
        assert_eq!(snapshot.billable_usage.uncached_input_tokens, 16);
        assert_eq!(snapshot.billable_usage.cache_read_tokens, 0);
        assert_eq!(snapshot.billable_usage.output_tokens, 42);
        assert_eq!(
            snapshot.usage_json.unwrap(),
            json!({
                "promptTokenCount":16,"cachedContentTokenCount":0,
                "candidatesTokenCount":0,"thoughtsTokenCount":42
            })
        );
    }
}

#[test]
fn gemini_thinking_is_billable_output_in_json_and_usage_only_sse() {
    for (candidates, expected) in [(Some(5), 47), (None, 42)] {
        let mut metadata =
            json!({"promptTokenCount":16,"thoughtsTokenCount":42,"cachedContentTokenCount":4});
        if let Some(tokens) = candidates {
            metadata["candidatesTokenCount"] = json!(tokens);
        }
        let body = json!({"usageMetadata":metadata});
        let snapshot = extract_usage_from_response(&Bytes::from(body.to_string()));
        assert_eq!(snapshot.billable_usage.output_tokens, expected);
        assert_eq!(snapshot.billable_usage.uncached_input_tokens, 12);
        assert_eq!(snapshot.usage.unwrap().total_tokens, Some(16 + expected));
        let mut collector = SseUsageCollector::new();
        collector
            .push_chunk(format!("data: {body}\n\n").as_bytes())
            .unwrap();
        assert_eq!(collector.finish().billable_usage.output_tokens, expected);
    }
}

#[test]
fn extracts_gemini_usage_and_cache_read_components() {
    let bytes = Bytes::from_static(
        br#"{"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12,"cachedContentTokenCount":4}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);
    let usage = snapshot.usage.expect("usage");

    assert_eq!(usage.input_tokens, Some(10));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(usage.total_tokens, Some(12));
    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 6);
    assert_eq!(snapshot.billable_usage.cache_read_tokens, 4);
}

#[test]
fn extracts_openai_cache_and_image_components() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"input_tokens":20,"output_tokens":7,"total_tokens":27,"input_tokens_details":{"cached_tokens":4,"image_tokens":3},"output_tokens_details":{"image_tokens":2}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);

    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 13);
    assert_eq!(snapshot.billable_usage.cache_read_tokens, 4);
    assert_eq!(snapshot.billable_usage.image_input_tokens, 3);
    assert_eq!(snapshot.billable_usage.output_tokens, 5);
    assert_eq!(snapshot.billable_usage.image_output_tokens, 2);
    assert_eq!(
        snapshot.usage_json.expect("usage json")["input_tokens"],
        json!(20)
    );
}

#[test]
fn extracts_responses_cache_write_details_without_double_counting() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"input_tokens":20,"output_tokens":7,"total_tokens":27,"input_tokens_details":{"cached_tokens":4,"cache_write_tokens":3}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);

    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 13);
    assert_eq!(snapshot.billable_usage.cache_read_tokens, 4);
    assert_eq!(snapshot.billable_usage.cache_write_tokens, 3);
    assert_eq!(snapshot.billable_usage.total_input_tokens(), 20);
}

#[test]
fn cache_write_tokens_take_priority_over_legacy_cached_creation_tokens() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"input_tokens":20,"output_tokens":7,"input_tokens_details":{"cache_write_tokens":3,"cached_creation_tokens":9}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);

    assert_eq!(snapshot.billable_usage.cache_write_tokens, 3);
    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 17);
}

#[test]
fn extracts_legacy_cached_creation_details_as_cache_write() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"input_tokens":8,"output_tokens":1,"input_tokens_details":{"cached_creation_tokens":2}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);

    assert_eq!(snapshot.billable_usage.cache_write_tokens, 2);
    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 6);
}

#[test]
fn anthropic_cache_breakdown_does_not_double_count_aggregate_write() {
    let bytes = Bytes::from_static(
        br#"{"service_tier":"priority","usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":4,"cache_creation_input_tokens":8,"cache_creation":{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":2}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);

    assert_eq!(snapshot.billable_usage.uncached_input_tokens, 10);
    assert_eq!(snapshot.billable_usage.cache_read_tokens, 4);
    assert_eq!(snapshot.billable_usage.cache_write_tokens, 3);
    assert_eq!(snapshot.billable_usage.cache_write_5m_tokens, 3);
    assert_eq!(snapshot.billable_usage.cache_write_1h_tokens, 2);
    assert_eq!(
        snapshot.usage.as_ref().and_then(|usage| usage.input_tokens),
        Some(22)
    );
    assert_eq!(snapshot.service_tier.as_deref(), Some("priority"));
}

#[test]
fn sse_collector_uses_latest_anthropic_usage_event() {
    let mut collector = SseUsageCollector::new();
    collector.push_chunk(
        b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":5}}}\n\n",
    ).unwrap();
    collector.push_chunk(
        b"data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":5}}\n\n",
    ).unwrap();
    let snapshot = collector.finish();

    assert_eq!(snapshot.billable_usage.cache_read_tokens, 4);
    assert_eq!(snapshot.billable_usage.cache_write_tokens, 5);
    assert_eq!(snapshot.billable_usage.output_tokens, 2);
    assert_eq!(snapshot.usage.expect("usage").total_tokens, Some(21));
}

#[test]
fn response_model_comes_from_chat_responses_and_gemini_envelopes() {
    let chat = extract_usage_from_response(&Bytes::from_static(
        br#"{"model":"gpt-5.6-luna","usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
    ));
    assert_eq!(chat.response_model.as_deref(), Some("gpt-5.6-luna"));

    let responses = extract_usage_from_response(&Bytes::from_static(
        br#"{"model":"gpt-6-astra","response":{"model":"gpt-5.6-luna","usage":{"input_tokens":1,"output_tokens":1}}}"#,
    ));
    // 嵌套 response.model 才是上游实际模型，外层可能是请求回显。
    assert_eq!(responses.response_model.as_deref(), Some("gpt-5.6-luna"));

    let gemini = extract_usage_from_response(&Bytes::from_static(
        br#"{"modelVersion":"gemini-2.5-pro","usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}}"#,
    ));
    assert_eq!(gemini.response_model.as_deref(), Some("gemini-2.5-pro"));
}

#[test]
fn sse_response_model_keeps_the_latest_event_and_does_not_drop_usage() {
    let mut collector = SseUsageCollector::new();
    collector
        .push_chunk(
            b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-6-astra\"}}\n\n",
        )
        .unwrap();
    collector.push_chunk(
        b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}}\n\n",
    ).unwrap();
    let snapshot = collector.finish();

    assert_eq!(snapshot.response_model.as_deref(), Some("gpt-5.6-luna"));
    assert_eq!(snapshot.billable_usage.output_tokens, 4);
    assert_eq!(
        snapshot.usage.as_ref().and_then(|usage| usage.input_tokens),
        Some(3)
    );
}

#[test]
fn observed_claude_speed_selects_existing_price_profile() {
    let settings = crate::pricing::default_model_pricing_settings();
    for speed in ["fast", "standard"] {
        let snapshot = extract_usage_from_response(&Bytes::from(
            json!({"usage":{"input_tokens":100,"output_tokens":10,"speed":speed}}).to_string(),
        ));
        let actual = crate::pricing::calculate_request_cost(
            &settings,
            Some("claude-opus-5-5"),
            None,
            snapshot.service_tier.as_deref(),
            &snapshot.billable_usage,
        )
        .unwrap();
        let expected = crate::pricing::calculate_request_cost(
            &settings,
            Some("claude-opus-5-5"),
            None,
            Some(speed),
            &snapshot.billable_usage,
        )
        .unwrap();
        assert_eq!(actual.service_tier, speed);
        assert_eq!(actual.cost_nano_usd, expected.cost_nano_usd);
    }
}
