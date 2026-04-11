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
fn anthropic_request_to_responses_maps_reasoning_context_and_structured_output() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "claude-3-7-sonnet",
        "max_tokens": 256,
        "system": [{ "type": "text", "text": "sys" }],
        "thinking": { "type": "enabled", "budget_tokens": 6000 },
        "output_format": {
            "type": "json_schema",
            "schema": {
                "type": "object",
                "properties": { "answer": { "type": "string" } },
                "required": ["answer"]
            }
        },
        "context_management": {
            "edits": [
                {
                    "type": "compact_20260112",
                    "trigger": { "type": "input_tokens", "value": 150000 }
                }
            ]
        },
        "metadata": { "user_id": "user-123" },
        "tools": [
            { "type": "web_search_20250305", "name": "web_search" }
        ],
        "messages": [
            {
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "chain-of-thought summary" },
                    { "type": "text", "text": "draft answer" }
                ]
            }
        ]
    }));

    let output = run_async(async {
        anthropic_request_to_responses(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["reasoning"]["effort"], json!("medium"));
    assert_eq!(value["reasoning"]["summary"], json!("detailed"));
    assert_eq!(value["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(
        value["text"]["format"]["schema"]["required"],
        json!(["answer"])
    );
    assert_eq!(value["context_management"][0]["type"], json!("compaction"));
    assert_eq!(
        value["context_management"][0]["compact_threshold"],
        json!(150000)
    );
    assert_eq!(value["user"], json!("user-123"));
    assert_eq!(value["tools"][0]["type"], json!("web_search_preview"));

    let input_items = value["input"].as_array().expect("input array");
    assert_eq!(input_items.len(), 1);
    assert_eq!(input_items[0]["type"], json!("message"));
    assert_eq!(input_items[0]["role"], json!("assistant"));
    assert_eq!(input_items[0]["content"][0]["type"], json!("output_text"));
    assert_eq!(
        input_items[0]["content"][0]["text"],
        json!("chain-of-thought summary")
    );
    assert_eq!(input_items[0]["content"][1]["type"], json!("output_text"));
    assert_eq!(input_items[0]["content"][1]["text"], json!("draft answer"));
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
    assert_eq!(
        value["tool_choice"]["disable_parallel_tool_use"],
        json!(true)
    );
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
fn responses_request_to_anthropic_preserves_structured_tool_result_parts_and_error() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "{\"status\":\"failed\"}",
                "is_error": true,
                "output_parts": [
                    { "type": "text", "text": "tool failed" },
                    { "type": "refusal", "refusal": "permission denied" },
                    { "type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo=" },
                    { "type": "input_file", "file_url": "data:application/pdf;base64,JVBERi0xLjQK" }
                ]
            }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(messages[0]["content"][0]["type"], json!("tool_result"));
    assert_eq!(messages[0]["content"][0]["tool_use_id"], json!("call_1"));
    assert_eq!(messages[0]["content"][0]["is_error"], json!(true));
    assert_eq!(
        messages[0]["content"][0]["content"][0],
        json!({ "type": "text", "text": "tool failed" })
    );
    assert_eq!(
        messages[0]["content"][0]["content"][1],
        json!({ "type": "text", "text": "permission denied" })
    );
    assert_eq!(
        messages[0]["content"][0]["content"][2]["type"],
        json!("image")
    );
    assert_eq!(
        messages[0]["content"][0]["content"][2]["source"]["media_type"],
        json!("image/png")
    );
    assert_eq!(
        messages[0]["content"][0]["content"][2]["source"]["data"],
        json!("iVBORw0KGgo=")
    );
    assert_eq!(
        messages[0]["content"][0]["content"][3]["type"],
        json!("document")
    );
    assert_eq!(
        messages[0]["content"][0]["content"][3]["source"]["media_type"],
        json!("application/pdf")
    );
    assert_eq!(
        messages[0]["content"][0]["content"][3]["source"]["data"],
        json!("JVBERi0xLjQK")
    );
}

#[test]
fn responses_request_to_anthropic_sanitizes_tool_use_ids_and_adds_missing_results() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            { "type": "message", "role": "user", "content": "ask tool" },
            {
                "type": "function_call",
                "call_id": "call/1?bad",
                "name": "search",
                "arguments": "{\"q\":\"x\"}"
            }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["content"][0]["type"], json!("tool_use"));
    assert_eq!(messages[1]["content"][0]["id"], json!("call_1_bad"));
    assert_eq!(messages[2]["role"], json!("user"));
    assert_eq!(messages[2]["content"][0]["type"], json!("tool_result"));
    assert_eq!(
        messages[2]["content"][0]["tool_use_id"],
        json!("call_1_bad")
    );
    assert_eq!(
        messages[2]["content"][0]["content"],
        json!("[System: Tool execution skipped/interrupted by user. No result provided for tool 'search'.]")
    );
}

#[test]
fn responses_request_to_anthropic_drops_orphaned_and_duplicate_tool_results() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "search",
                "arguments": "{\"q\":\"x\"}"
            },
            { "type": "function_call_output", "call_id": "orphan", "output": "ignore me" },
            { "type": "function_call_output", "call_id": "call_1", "output": "old value" },
            { "type": "function_call_output", "call_id": "call_1", "output": "new value" }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(messages[2]["role"], json!("user"));
    let content = messages[2]["content"].as_array().expect("tool results");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["tool_use_id"], json!("call_1"));
    assert_eq!(content[0]["content"], json!("new value"));
}

#[test]
fn responses_request_to_anthropic_sanitizes_empty_text_messages() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            { "type": "message", "role": "user", "content": "   " },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "" }]
            }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(
        messages[0]["content"][0]["text"],
        json!("[System: Empty message content sanitised to satisfy protocol]")
    );
    assert_eq!(
        messages[1]["content"][0]["text"],
        json!("[System: Empty message content sanitised to satisfy protocol]")
    );
}

#[test]
fn responses_response_to_anthropic_maps_reasoning_items_to_thinking_blocks() {
    let input = bytes_from_json(json!({
        "id": "resp_reasoning_item",
        "model": "gpt-5",
        "output": [
            {
                "id": "rs_1",
                "type": "reasoning",
                "summary": [
                    { "type": "summary_text", "text": "first analyze then answer" }
                ]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "final answer" }
                ]
            }
        ],
        "usage": { "input_tokens": 3, "output_tokens": 5 }
    }));

    let output = responses_response_to_anthropic(&input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["content"][0]["type"], json!("thinking"));
    assert_eq!(
        value["content"][0]["thinking"],
        json!("first analyze then answer")
    );
    assert_eq!(value["content"][1]["type"], json!("text"));
    assert_eq!(value["content"][1]["text"], json!("final answer"));
}

#[test]
fn anthropic_response_to_responses_maps_thinking_blocks_to_reasoning_items() {
    let input = bytes_from_json(json!({
        "id": "msg_thinking",
        "model": "claude-3-7-sonnet",
        "content": [
            { "type": "thinking", "thinking": "analyze first" },
            { "type": "text", "text": "final answer" },
            { "type": "tool_use", "id": "call_1", "name": "search", "input": { "q": "x" } }
        ],
        "usage": { "input_tokens": 2, "output_tokens": 4 }
    }));

    let output = anthropic_response_to_responses(&input).expect("transform");
    let value = json_from_bytes(output);

    let output_items = value["output"].as_array().expect("output array");
    assert_eq!(output_items[0]["type"], json!("reasoning"));
    assert_eq!(output_items[0]["status"], json!("completed"));
    assert_eq!(
        output_items[0]["summary"][0],
        json!({ "type": "summary_text", "text": "analyze first" })
    );
    assert_eq!(output_items[1]["type"], json!("message"));
    assert_eq!(output_items[1]["content"][0]["type"], json!("output_text"));
    assert_eq!(output_items[1]["content"][0]["text"], json!("final answer"));
    assert_eq!(output_items[2]["type"], json!("function_call"));
    assert_eq!(output_items[2]["call_id"], json!("call_1"));
    assert_eq!(output_items[2]["name"], json!("search"));
    assert_eq!(output_items[2]["arguments"], json!("{\"q\":\"x\"}"));
}

#[test]
fn anthropic_response_to_responses_maps_redacted_thinking_to_encrypted_reasoning() {
    let input = bytes_from_json(json!({
        "id": "msg_redacted",
        "model": "claude-3-7-sonnet",
        "content": [
            { "type": "thinking", "thinking": "analyze first" },
            { "type": "redacted_thinking", "data": "ENC123" },
            { "type": "text", "text": "final answer" }
        ],
        "usage": { "input_tokens": 2, "output_tokens": 4 }
    }));

    let output = anthropic_response_to_responses(&input).expect("transform");
    let value = json_from_bytes(output);

    let output_items = value["output"].as_array().expect("output array");
    assert_eq!(output_items[0]["type"], json!("reasoning"));
    assert_eq!(
        output_items[0]["summary"][0],
        json!({ "type": "summary_text", "text": "analyze first" })
    );
    assert_eq!(output_items[0]["encrypted_content"], json!("ENC123"));
    assert_eq!(output_items[1]["type"], json!("message"));
    assert_eq!(output_items[1]["content"][0]["text"], json!("final answer"));
}

#[test]
fn anthropic_response_to_responses_maps_max_tokens_to_incomplete_status() {
    let input = bytes_from_json(json!({
        "id": "msg_incomplete",
        "model": "claude-3-7-sonnet",
        "stop_reason": "max_tokens",
        "content": [
            { "type": "text", "text": "partial answer" }
        ],
        "usage": { "input_tokens": 2, "output_tokens": 4 }
    }));

    let output = anthropic_response_to_responses(&input).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["status"], json!("incomplete"));
    assert_eq!(value["incomplete_details"]["reason"], json!("max_tokens"));
    assert_eq!(value["output"][0]["type"], json!("message"));
    assert_eq!(value["output"][0]["status"], json!("incomplete"));
    assert_eq!(
        value["output"][0]["content"][0]["text"],
        json!("partial answer")
    );
}

#[test]
fn responses_response_to_anthropic_maps_encrypted_reasoning_to_redacted_thinking() {
    let input = bytes_from_json(json!({
        "id": "resp_redacted",
        "model": "gpt-4.1",
        "output": [
            {
                "id": "rs_1",
                "type": "reasoning",
                "summary": [
                    { "type": "summary_text", "text": "first analyze then answer" }
                ],
                "encrypted_content": "ENC456"
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "final answer" }
                ]
            }
        ],
        "usage": { "input_tokens": 3, "output_tokens": 5 }
    }));

    let output = responses_response_to_anthropic(&input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["content"][0]["type"], json!("thinking"));
    assert_eq!(
        value["content"][0]["thinking"],
        json!("first analyze then answer")
    );
    assert_eq!(value["content"][1]["type"], json!("redacted_thinking"));
    assert_eq!(value["content"][1]["data"], json!("ENC456"));
    assert_eq!(value["content"][2]["type"], json!("text"));
    assert_eq!(value["content"][2]["text"], json!("final answer"));
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
