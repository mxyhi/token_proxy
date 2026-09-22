use super::*;
use serde_json::json;

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
        collector.push_chunk(format!("data: {body}\n\n").as_bytes());
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
    );
    collector.push_chunk(
        b"data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":5}}\n\n",
    );
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
    collector.push_chunk(
        b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-6-astra\"}}\n\n",
    );
    collector.push_chunk(
        b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}}\n\n",
    );
    let snapshot = collector.finish();

    assert_eq!(snapshot.response_model.as_deref(), Some("gpt-5.6-luna"));
    assert_eq!(snapshot.billable_usage.output_tokens, 4);
    assert_eq!(
        snapshot.usage.as_ref().and_then(|usage| usage.input_tokens),
        Some(3)
    );
}
