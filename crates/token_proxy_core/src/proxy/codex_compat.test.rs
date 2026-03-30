use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::json;
use std::{sync::Arc, time::Instant};

use super::super::{
    log::{LogContext, LogWriter},
    token_rate::TokenRateTracker,
};
use super::tool_names::shorten_name_if_needed;
use super::{
    chat_request_to_codex, codex_response_to_chat, codex_response_to_responses,
    responses_request_to_codex, stream_codex_to_chat, stream_codex_to_responses,
};

#[test]
fn chat_request_to_codex_sets_model_and_stream() {
    let input = json!({
        "model": "gpt-5",
        "stream": true,
        "messages": [
            { "role": "user", "content": "hi" }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = chat_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(value["model"], "gpt-5-codex");
    assert_eq!(value["stream"], true);
    assert_eq!(value["input"][0]["type"], "message");
}

#[test]
fn codex_response_to_chat_restores_tool_name() {
    let original = "mcp__very_long_tool_name_for_codex_restoration_check_v1_tool_extra_long_suffix";
    let short = shorten_name_if_needed(original);
    assert!(short.len() <= 64);
    assert_ne!(short, original);

    let request_body = json!({
        "tools": [
            { "type": "function", "function": { "name": original } }
        ]
    })
    .to_string();

    let response = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_1",
            "created_at": 123,
            "model": "gpt-5",
            "status": "completed",
            "output": [
                { "type": "function_call", "call_id": "call_1", "name": short, "arguments": "{}" }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
        }
    });
    let bytes = Bytes::from(response.to_string());
    let output = codex_response_to_chat(&bytes, Some(&request_body)).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let name = value["choices"][0]["message"]["tool_calls"][0]["function"]["name"]
        .as_str()
        .expect("tool name");
    assert_eq!(name, original);
}

#[test]
fn codex_response_to_responses_rejects_json_error_payload() {
    let bytes = Bytes::from(
        json!({
            "error": {
                "message": "rate limit exceeded",
                "type": "rate_limit_error"
            }
        })
        .to_string(),
    );

    let error = codex_response_to_responses(&bytes, None).expect_err("should fail");
    assert!(
        error.contains("rate limit exceeded"),
        "unexpected error: {error}"
    );
    assert!(error.contains("error payload"), "unexpected error: {error}");
}

#[test]
fn codex_response_to_responses_rejects_non_json_success_payload_with_raw_text() {
    let bytes = Bytes::from("upstream gateway said nope");

    let error = codex_response_to_responses(&bytes, None).expect_err("should fail");
    assert!(
        error.contains("non-JSON success payload"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("upstream gateway said nope"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn stream_codex_to_responses_emits_error_event_for_invalid_json_event() {
    let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        "data: not-json\n\n",
    ))]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(text.contains("\"type\":\"error\""), "chunks: {text}");
    assert!(text.contains("invalid JSON stream event"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_chat_emits_error_event_for_invalid_json_event() {
    let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        "data: not-json\n\n",
    ))]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_chat(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(text.contains("\"error\":{"), "chunks: {text}");
    assert!(text.contains("invalid JSON stream event"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

fn test_log_context() -> LogContext {
    LogContext {
        path: "/v1/responses".to_string(),
        provider: "codex".to_string(),
        upstream_id: "test".to_string(),
        account_id: None,
        model: Some("gpt-5-codex".to_string()),
        mapped_model: None,
        stream: true,
        status: 200,
        upstream_request_id: None,
        request_headers: None,
        request_body: None,
        ttfb_ms: None,
        start: Instant::now(),
    }
}

fn join_stream_chunks(chunks: &[Result<Bytes, std::io::Error>]) -> String {
    chunks
        .iter()
        .map(|chunk| chunk.as_ref().expect("stream chunk"))
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn chat_request_to_codex_skips_missing_tool_names() {
    let input = json!({
        "model": "gpt-5",
        "messages": [
            { "role": "user", "content": "hi" }
        ],
        "tools": [
            { "type": "function", "function": { "description": "noop", "parameters": {} } }
        ],
        "tool_choice": { "type": "function", "function": {} }
    });
    let bytes = Bytes::from(input.to_string());
    let output = chat_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let tools = value["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);
    assert!(tools[0].get("name").is_none());
    let tool_choice = value["tool_choice"].as_object().expect("tool_choice");
    assert_eq!(
        tool_choice.get("type").and_then(serde_json::Value::as_str),
        Some("function")
    );
    assert!(tool_choice.get("name").is_none());
}

#[test]
fn responses_request_to_codex_uses_top_level_tool_name() {
    let input = json!({
        "model": "gpt-5",
        "input": "hi",
        "tools": [
            { "type": "function", "name": "demo_tool", "description": "noop", "parameters": {} }
        ],
        "tool_choice": { "type": "function", "name": "demo_tool" }
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let tools = value["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "demo_tool");
    assert_eq!(tools[0]["description"], "noop");
    assert!(tools[0]["parameters"].is_object());
    assert_eq!(
        value["tool_choice"]
            .get("name")
            .and_then(serde_json::Value::as_str),
        Some("demo_tool")
    );
}

#[test]
fn responses_request_to_codex_strips_prompt_cache_retention() {
    let input = json!({
        "model": "gpt-5",
        "input": "hi",
        "prompt_cache_retention": "24h",
        "previous_response_id": "resp_123",
        "safety_identifier": "sid_1"
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    assert!(value.get("prompt_cache_retention").is_none());
    assert!(value.get("previous_response_id").is_none());
    assert!(value.get("safety_identifier").is_none());
}

#[test]
fn responses_request_to_codex_preserves_parallel_tool_calls_false() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "hi" }]
            }
        ],
        "parallel_tool_calls": false
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(value["parallel_tool_calls"], json!(false));
}

#[test]
fn responses_request_to_codex_strips_output_parts_from_function_call_output() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "ok",
                "output_parts": [
                    { "type": "text", "text": "ok" }
                ]
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");
    assert_eq!(input_items.len(), 1);
    assert!(input_items[0].get("output_parts").is_none());
}
