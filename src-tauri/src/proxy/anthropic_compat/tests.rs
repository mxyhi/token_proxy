use super::*;

use axum::body::Bytes;
use serde_json::{json, Value};

use crate::proxy::http_client::ProxyHttpClients;

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(future)
}

fn bytes_from_json(value: Value) -> Bytes {
    Bytes::from(serde_json::to_vec(&value).expect("serialize JSON"))
}

fn json_from_bytes(bytes: Bytes) -> Value {
    serde_json::from_slice(&bytes).expect("parse JSON")
}

#[test]
fn anthropic_request_to_responses_maps_tools_and_tool_blocks() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "claude-3-5-sonnet",
        "max_tokens": 123,
        "stream": true,
        "system": "sys",
        "stop_sequences": ["a", "b"],
        "tools": [
            {
                "name": "search",
                "description": "Search something",
                "input_schema": {
                    "type": "object",
                    "properties": { "q": { "type": "string" } },
                    "required": ["q"]
                }
            }
        ],
        "tool_choice": {
            "type": "tool",
            "name": "search",
            "disable_parallel_tool_use": true
        },
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
            { "role": "assistant", "content": [{ "type": "tool_use", "id": "call_1", "name": "search", "input": { "q": "x" } }] },
            { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_1", "content": "ok" }] }
        ]
    }));

    let output = run_async(async {
        anthropic_request_to_responses(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("claude-3-5-sonnet"));
    assert_eq!(value["max_output_tokens"], json!(123));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["instructions"], json!("sys"));

    assert_eq!(value["tools"][0]["type"], json!("function"));
    assert_eq!(value["tools"][0]["name"], json!("search"));
    assert_eq!(value["tools"][0]["parameters"]["required"], json!(["q"]));

    assert_eq!(value["tool_choice"]["type"], json!("function"));
    assert_eq!(value["tool_choice"]["name"], json!("search"));
    assert_eq!(value["parallel_tool_calls"], json!(false));
    assert_eq!(value["stop"], json!(["a", "b"]));

    let input_items = value["input"].as_array().expect("input array");
    assert_eq!(input_items[0]["type"], json!("message"));
    assert_eq!(input_items[0]["role"], json!("user"));
    assert_eq!(input_items[0]["content"][0]["type"], json!("input_text"));
    assert_eq!(input_items[0]["content"][0]["text"], json!("hi"));

    assert_eq!(input_items[1]["type"], json!("function_call"));
    assert_eq!(input_items[1]["call_id"], json!("call_1"));
    assert_eq!(input_items[1]["name"], json!("search"));
    assert_eq!(input_items[1]["arguments"], json!("{\"q\":\"x\"}"));

    assert_eq!(input_items[2]["type"], json!("function_call_output"));
    assert_eq!(input_items[2]["call_id"], json!("call_1"));
    assert_eq!(input_items[2]["output"], json!("ok"));
}

#[test]
fn responses_request_to_anthropic_maps_tool_choice_and_tool_result() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "max_output_tokens": 50,
        "stream": true,
        "stop": ["a", "b"],
        "tools": [
            {
                "type": "function",
                "name": "search",
                "description": "Search something",
                "parameters": {
                    "type": "object",
                    "properties": { "q": { "type": "string" } },
                    "required": ["q"]
                }
            }
        ],
        "tool_choice": { "type": "function", "name": "search" },
        "parallel_tool_calls": false,
        "input": [
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
            { "type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{\"q\":\"x\"}" },
            { "type": "function_call_output", "call_id": "call_1", "output": "ok" }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["max_tokens"], json!(50));
    assert_eq!(value["stream"], json!(true));

    assert_eq!(value["tools"][0]["name"], json!("search"));
    assert_eq!(value["tool_choice"]["type"], json!("tool"));
    assert_eq!(value["tool_choice"]["name"], json!("search"));
    assert_eq!(value["tool_choice"]["disable_parallel_tool_use"], json!(true));
    assert_eq!(value["stop_sequences"], json!(["a", "b"]));

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(messages[0]["content"][0]["type"], json!("text"));
    assert_eq!(messages[0]["content"][0]["text"], json!("hi"));

    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(messages[1]["content"][0]["type"], json!("tool_use"));
    assert_eq!(messages[1]["content"][0]["id"], json!("call_1"));
    assert_eq!(messages[1]["content"][0]["name"], json!("search"));
    assert_eq!(messages[1]["content"][0]["input"]["q"], json!("x"));

    assert_eq!(messages[2]["role"], json!("user"));
    assert_eq!(messages[2]["content"][0]["type"], json!("tool_result"));
    assert_eq!(messages[2]["content"][0]["tool_use_id"], json!("call_1"));
    assert_eq!(messages[2]["content"][0]["content"], json!("ok"));
}

#[test]
fn responses_response_to_anthropic_includes_thinking_block() {
    let input = bytes_from_json(json!({
        "id": "resp_thinking",
        "model": "gpt-4.1",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "reasoning_text", "text": "think" },
                    { "type": "output_text", "text": "ok" }
                ]
            }
        ],
        "usage": { "input_tokens": 1, "output_tokens": 2 }
    }));

    let output = responses_response_to_anthropic(&input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["content"][0]["type"], json!("thinking"));
    assert_eq!(value["content"][0]["thinking"], json!("think"));
    assert!(value["content"][0]["signature"].as_str().is_some());
    assert_eq!(value["content"][1]["type"], json!("text"));
    assert_eq!(value["content"][1]["text"], json!("ok"));
}
