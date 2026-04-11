use super::*;
use serde_json::json;

#[test]
fn extract_usage_from_gemini_usage_metadata() {
    let bytes = Bytes::from_static(
        br#"{"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"totalTokenCount":3}}"#,
    );
    let usage = extract_usage_from_response(&bytes).usage.expect("usage");
    assert_eq!(usage.input_tokens, Some(1));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(usage.total_tokens, Some(3));
}

#[test]
fn sse_usage_collector_extracts_gemini_usage_metadata() {
    let mut collector = SseUsageCollector::new();
    collector.push_chunk(
        b"data: {\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":2,\"totalTokenCount\":3}}\n\n",
    );
    let usage = collector.finish().usage.expect("usage");
    assert_eq!(usage.input_tokens, Some(1));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(usage.total_tokens, Some(3));
}

#[test]
fn extract_cached_tokens_from_openai_input_tokens_details() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3,"input_tokens_details":{"cached_tokens":4}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);
    assert_eq!(snapshot.cached_tokens, Some(4));
    assert_eq!(
        snapshot.usage_json.expect("usage_json")["input_tokens"],
        json!(1)
    );
}

#[test]
fn extract_cached_tokens_from_openai_prompt_tokens_details() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3,"prompt_tokens_details":{"cached_tokens":4}}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);
    assert_eq!(snapshot.cached_tokens, Some(4));
    assert_eq!(
        snapshot.usage_json.expect("usage_json")["prompt_tokens"],
        json!(1)
    );
}

#[test]
fn extract_cached_tokens_from_anthropic_cache_fields() {
    let bytes = Bytes::from_static(
        br#"{"usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":4,"cache_creation_input_tokens":5}}"#,
    );
    let snapshot = extract_usage_from_response(&bytes);
    assert_eq!(snapshot.cached_tokens, Some(9));
    assert_eq!(
        snapshot.usage_json.expect("usage_json")["cache_read_input_tokens"],
        json!(4)
    );
}

#[test]
fn sse_usage_collector_extracts_anthropic_message_usage_and_cache_tokens() {
    let mut collector = SseUsageCollector::new();
    collector.push_chunk(
        b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":5}}}\n\n",
    );
    let snapshot = collector.finish();
    assert_eq!(snapshot.cached_tokens, Some(9));
    let usage = snapshot.usage.expect("usage");
    assert_eq!(usage.input_tokens, Some(1));
    assert_eq!(usage.output_tokens, Some(2));
    assert_eq!(usage.total_tokens, None);
}
