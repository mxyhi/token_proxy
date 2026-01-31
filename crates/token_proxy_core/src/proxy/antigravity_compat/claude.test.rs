use axum::body::Bytes;
use serde_json::Value;

use super::claude_request_to_antigravity;

fn parse_output(bytes: Bytes) -> Value {
    serde_json::from_slice(&bytes).expect("json")
}

#[test]
fn converts_basic_structure_and_system() {
    let input = Bytes::from(
        r#"{
        "model": "claude-3-5-sonnet-20240620",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "Hello"}]}
        ],
        "system": [{"type": "text", "text": "You are helpful"}]
    }"#,
    );
    let output = parse_output(claude_request_to_antigravity(&input, None).expect("convert"));
    assert_eq!(output["contents"][0]["role"], "user");
    assert_eq!(
        output["systemInstruction"]["parts"][0]["text"],
        "You are helpful"
    );
}

#[test]
fn drops_non_string_text_blocks() {
    let input = Bytes::from(
        r#"{
        "model": "claude-3-5-sonnet-20240620",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": {"text": "hi"}}]}
        ]
    }"#,
    );
    let output = parse_output(claude_request_to_antigravity(&input, None).expect("convert"));
    let parts = output["contents"][0]["parts"].as_array().unwrap();
    assert!(parts.is_empty());
}

#[test]
fn tool_use_adds_skip_signature() {
    let input = Bytes::from(
        r#"{
        "model": "claude-3-5-sonnet-20240620",
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_123", "name": "get_weather", "input": "{\"location\":\"Paris\"}"}
                ]
            }
        ]
    }"#,
    );
    let output = parse_output(claude_request_to_antigravity(&input, None).expect("convert"));
    let part = &output["contents"][0]["parts"][0];
    assert_eq!(part["functionCall"]["name"], "get_weather");
    assert_eq!(part["thoughtSignature"], "skip_thought_signature_validator");
}

#[test]
fn unsigned_thinking_is_removed() {
    let input = Bytes::from(
        r#"{
        "model": "claude-sonnet-4-5-thinking",
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "Let me think"},
                    {"type": "text", "text": "Answer"}
                ]
            }
        ]
    }"#,
    );
    let output = parse_output(claude_request_to_antigravity(&input, None).expect("convert"));
    let parts = output["contents"][0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "Answer");
}

#[test]
fn model_hint_overrides_request_model() {
    let input = Bytes::from(
        r#"{
        "model": "claude-haiku-4-5-20251001",
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "Hello"}]}
        ]
    }"#,
    );
    let output = parse_output(
        claude_request_to_antigravity(&input, Some("gemini-2.5-flash")).expect("convert"),
    );
    assert_eq!(output["model"], "gemini-2.5-flash");
}
