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
    assert_eq!(value["max_output_tokens"], json!(128));
    assert_eq!(value["stream"], json!(true));
    assert!(value.get("instructions").is_none());
    assert_eq!(value["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(value["store"], json!(false));
    assert_eq!(value["parallel_tool_calls"], json!(false));
    assert_eq!(value["text"]["verbosity"], json!("medium"));

    assert_eq!(value["tools"][0]["type"], json!("function"));
    assert_eq!(value["tools"][0]["name"], json!("search"));
    assert_eq!(value["tools"][0]["parameters"]["required"], json!(["q"]));
    assert_eq!(value["tools"][0]["strict"], json!(false));

    assert_eq!(value["tool_choice"]["type"], json!("function"));
    assert_eq!(value["tool_choice"]["name"], json!("search"));
    assert_eq!(value["stop"], json!(["a", "b"]));

    let input_items = value["input"].as_array().expect("input array");
    assert_eq!(input_items[0]["type"], json!("message"));
    assert_eq!(input_items[0]["role"], json!("developer"));
    assert_eq!(input_items[0]["content"][0]["type"], json!("input_text"));
    assert_eq!(input_items[0]["content"][0]["text"], json!("sys"));

    assert_eq!(input_items[1]["type"], json!("message"));
    assert_eq!(input_items[1]["role"], json!("user"));
    assert_eq!(input_items[1]["content"][0]["type"], json!("input_text"));
    assert_eq!(input_items[1]["content"][0]["text"], json!("hi"));

    assert_eq!(input_items[2]["type"], json!("function_call"));
    assert_eq!(input_items[2]["call_id"], json!("call_1"));
    assert_eq!(input_items[2]["name"], json!("search"));
    assert_eq!(input_items[2]["arguments"], json!("{\"q\":\"x\"}"));

    assert_eq!(input_items[3]["type"], json!("function_call_output"));
    assert_eq!(input_items[3]["call_id"], json!("call_1"));
    assert_eq!(input_items[3]["output"], json!("ok"));
}

#[test]
fn anthropic_request_to_responses_strips_sampling_params_for_reasoning_models() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-5.4-mini",
        "max_tokens": 123,
        "temperature": 0.7,
        "top_p": 0.9,
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "hi" }] }
        ]
    }));

    let output = run_async(async {
        anthropic_request_to_responses(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("gpt-5.4-mini"));
    assert!(value.get("temperature").is_none());
    assert!(value.get("top_p").is_none());
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
    assert!(value["reasoning"].get("summary").is_none());
    assert_eq!(value["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(value["text"]["verbosity"], json!("medium"));
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
    assert_eq!(input_items.len(), 2);
    assert_eq!(input_items[0]["type"], json!("message"));
    assert_eq!(input_items[0]["role"], json!("developer"));
    assert_eq!(input_items[0]["content"][0]["text"], json!("sys"));
    assert_eq!(input_items[1]["type"], json!("message"));
    assert_eq!(input_items[1]["role"], json!("assistant"));
    assert_eq!(input_items[1]["content"][0]["type"], json!("output_text"));
    assert_eq!(
        input_items[1]["content"][0]["text"],
        json!("chain-of-thought summary")
    );
    assert_eq!(input_items[1]["content"][1]["type"], json!("output_text"));
    assert_eq!(input_items[1]["content"][1]["text"], json!("draft answer"));
}

#[test]
fn anthropic_request_to_responses_filters_billing_header_and_maps_adaptive_thinking() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-5.4-mini",
        "max_tokens": 8,
        "system": [
            { "type": "text", "text": "x-anthropic-billing-header: keep-out" },
            { "type": "text", "text": "real system" }
        ],
        "thinking": { "type": "adaptive" },
        "output_config": { "effort": "high" },
        "messages": [
            { "role": "user", "content": "hi" }
        ]
    }));

    let output = run_async(async {
        anthropic_request_to_responses(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);
    let input_items = value["input"].as_array().expect("input array");

    assert_eq!(value["max_output_tokens"], json!(128));
    assert_eq!(value["reasoning"]["effort"], json!("high"));
    assert!(value["reasoning"].get("summary").is_none());
    assert_eq!(input_items[0]["role"], json!("developer"));
    assert_eq!(
        input_items[0]["content"].as_array().expect("system").len(),
        1
    );
    assert_eq!(input_items[0]["content"][0]["text"], json!("real system"));
}

#[test]
fn reasoning_summary_visibility_round_trips_between_anthropic_and_responses() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    for (display, responses_summary) in [("summarized", json!("auto")), ("omitted", Value::Null)] {
        let anthropic = bytes_from_json(json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 256,
            "thinking": { "type": "adaptive", "display": display },
            "output_config": { "effort": "high" },
            "messages": [{ "role": "user", "content": "hi" }]
        }));
        let responses = run_async(async {
            anthropic_request_to_responses(&anthropic, &http_clients)
                .await
                .expect("Anthropic to Responses")
        });
        let responses = json_from_bytes(responses);

        assert_eq!(responses["reasoning"]["effort"], json!("high"));
        assert_eq!(responses["reasoning"]["summary"], responses_summary);

        let anthropic = run_async(async {
            responses_request_to_anthropic(&bytes_from_json(responses), &http_clients)
                .await
                .expect("Responses to Anthropic")
        });
        let anthropic = json_from_bytes(anthropic);

        assert_eq!(anthropic["thinking"]["type"], json!("adaptive"));
        assert_eq!(anthropic["thinking"]["display"], json!(display));
        assert_eq!(anthropic["output_config"]["effort"], json!("high"));
    }
}

#[test]
fn reasoning_effort_does_not_enable_missing_summary_visibility() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let responses = bytes_from_json(json!({
        "model": "gpt-5.6-sol",
        "input": "hi",
        "reasoning": { "effort": "high" }
    }));

    let anthropic = run_async(async {
        responses_request_to_anthropic(&responses, &http_clients)
            .await
            .expect("Responses to Anthropic")
    });
    let anthropic = json_from_bytes(anthropic);

    assert_eq!(anthropic["thinking"]["type"], json!("adaptive"));
    assert!(anthropic["thinking"].get("display").is_none());
    assert_eq!(anthropic["output_config"]["effort"], json!("high"));
}

#[test]
fn anthropic_request_to_responses_splits_tool_result_media_from_output_string() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "claude-3-5-sonnet",
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": [
                            { "type": "text", "text": "visible text" },
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "iVBORw0KGgo="
                                }
                            },
                            {
                                "type": "document",
                                "source": {
                                    "type": "base64",
                                    "media_type": "application/pdf",
                                    "data": "JVBERi0xLjQK"
                                }
                            }
                        ],
                        "is_error": true
                    }
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
    let input_items = value["input"].as_array().expect("input array");

    assert_eq!(input_items[0]["type"], json!("function_call_output"));
    assert_eq!(input_items[0]["call_id"], json!("call_1"));
    assert_eq!(input_items[0]["output"], json!("visible text"));
    assert_eq!(input_items[0]["is_error"], json!(true));
    assert!(input_items[0].get("output_parts").is_none());
    assert_eq!(input_items[1]["type"], json!("message"));
    assert_eq!(input_items[1]["role"], json!("user"));
    assert_eq!(input_items[1]["content"][0]["type"], json!("input_image"));
    assert_eq!(
        input_items[1]["content"][0]["image_url"],
        json!("data:image/png;base64,iVBORw0KGgo=")
    );
    assert_eq!(input_items[1]["content"][1]["type"], json!("input_file"));
    assert_eq!(
        input_items[1]["content"][1]["file_url"],
        json!("data:application/pdf;base64,JVBERi0xLjQK")
    );
}

#[test]
fn anthropic_request_to_responses_normalizes_tool_schema_defaults() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "claude-3-5-sonnet",
        "tools": [
            { "name": "empty_schema" },
            { "name": "object_schema", "input_schema": { "type": "object" } }
        ],
        "messages": [
            { "role": "user", "content": "hi" }
        ]
    }));

    let output = run_async(async {
        anthropic_request_to_responses(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["tools"][0]["parameters"]["type"], json!("object"));
    assert_eq!(value["tools"][0]["parameters"]["properties"], json!({}));
    assert_eq!(value["tools"][0]["strict"], json!(false));
    assert_eq!(value["tools"][1]["parameters"]["type"], json!("object"));
    assert_eq!(value["tools"][1]["parameters"]["properties"], json!({}));
    assert_eq!(value["tools"][1]["strict"], json!(false));
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
fn responses_request_to_anthropic_keeps_namespace_history_and_declaration_aligned() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "claude-3-5-sonnet",
        "input": [
            { "type": "function_call", "call_id": "call_1", "namespace": "mcp__github", "name": "get_me", "arguments": "{}" },
            { "type": "function_call_output", "call_id": "call_1", "output": "ok" }
        ],
        "tools": [{
            "type": "namespace",
            "name": "mcp__github",
            "tools": [{ "type": "function", "name": "get_me", "parameters": { "type": "object" } }]
        }]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["tools"][0]["name"], "mcp__github__get_me");
    assert_eq!(
        value["messages"][1]["content"][0]["name"],
        "mcp__github__get_me"
    );
}

#[test]
fn responses_request_to_anthropic_merges_system_roles_for_all_message_shapes() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let cases = [
        (
            "chat-style string content",
            json!({
                "model": "claude-3-7-sonnet",
                "instructions": "top-level rules",
                "input": [
                    { "role": "system", "content": "system rules" },
                    { "role": "developer", "content": "developer rules" },
                    { "role": "user", "content": "hello" }
                ]
            }),
            "top-level rules\nsystem rules\ndeveloper rules",
        ),
        (
            "Responses message array content",
            json!({
                "model": "claude-3-7-sonnet",
                "input": [
                    {
                        "type": "message",
                        "role": "system",
                        "content": [{ "type": "input_text", "text": "system rules" }]
                    },
                    {
                        "type": "message",
                        "role": "developer",
                        "content": [{ "type": "input_text", "text": "developer rules" }]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": "hello" }]
                    }
                ]
            }),
            "system rules\ndeveloper rules",
        ),
    ];

    for (name, input, expected_system) in cases {
        let output = run_async(async {
            responses_request_to_anthropic(&bytes_from_json(input), &http_clients)
                .await
                .expect("transform")
        });
        let value = json_from_bytes(output);

        assert_eq!(
            value["system"],
            json!([{ "type": "text", "text": expected_system }]),
            "case={name}"
        );
        let messages = value["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 1, "case={name}");
        assert_eq!(messages[0]["role"], json!("user"), "case={name}");
        assert!(
            messages.iter().all(|message| {
                matches!(
                    message.get("role").and_then(Value::as_str),
                    Some("user" | "assistant")
                )
            }),
            "case={name}: {messages:?}"
        );
    }
}

#[test]
fn responses_request_to_anthropic_maps_google_search_tool() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "tools": [
            { "type": "google_search", "name": "google_search" }
        ],
        "input": [
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "latest docs" }] }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["tools"][0]["type"], json!("web_search_20250305"));
    assert_eq!(value["tools"][0]["name"], json!("web_search"));
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
fn responses_request_to_anthropic_maps_new_tool_output_types_to_tool_results() {
    let http_clients = ProxyHttpClients::new().expect("http clients");

    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            { "type": "tool_search_output", "call_id": "call_search", "output": "search ok" },
            { "type": "custom_tool_call_output", "call_id": "call_custom", "output": "custom ok" },
            { "type": "mcp_tool_call_output", "call_id": "call_mcp", "output": "mcp ok" }
        ]
    }));

    let output = run_async(async {
        responses_request_to_anthropic(&input, &http_clients)
            .await
            .expect("transform")
    });
    let value = json_from_bytes(output);
    let messages = value["messages"].as_array().expect("messages array");
    let content = messages[0]["content"].as_array().expect("tool results");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(content.len(), 3);
    assert_eq!(content[0]["type"], json!("tool_result"));
    assert_eq!(content[0]["tool_use_id"], json!("call_search"));
    assert_eq!(content[0]["content"], json!("search ok"));
    assert_eq!(content[1]["tool_use_id"], json!("call_custom"));
    assert_eq!(content[1]["content"], json!("custom ok"));
    assert_eq!(content[2]["tool_use_id"], json!("call_mcp"));
    assert_eq!(content[2]["content"], json!("mcp ok"));
}

#[test]
fn responses_request_to_anthropic_pairs_custom_tool_calls_with_results() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            { "type": "message", "role": "user", "content": "run the patch" },
            {
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "apply_patch",
                "input": "*** Begin Patch"
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_custom",
                "output": "Done!"
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
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(messages[1]["content"][0]["type"], json!("tool_use"));
    assert_eq!(messages[1]["content"][0]["id"], json!("call_custom"));
    assert_eq!(messages[1]["content"][0]["name"], json!("apply_patch"));
    assert_eq!(
        messages[1]["content"][0]["input"],
        json!({ "_raw": "*** Begin Patch" })
    );
    assert_eq!(messages[2]["role"], json!("user"));
    assert_eq!(messages[2]["content"][0]["type"], json!("tool_result"));
    assert_eq!(
        messages[2]["content"][0]["tool_use_id"],
        json!("call_custom")
    );
    assert_eq!(messages[2]["content"][0]["content"], json!("Done!"));
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
                "encrypted_content": "SIG_REASONING",
                "summary": [
                    { "type": "summary_text", "text": "first analyze then answer" }
                ],
                "content": [
                    { "type": "reasoning_text", "text": "first analyze then answer" }
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
    assert_eq!(value["content"][0]["signature"], json!("SIG_REASONING"));
    assert_eq!(value["content"][1]["type"], json!("text"));
    assert_eq!(value["content"][1]["text"], json!("final answer"));
}

#[test]
fn anthropic_response_to_responses_maps_thinking_blocks_to_reasoning_items() {
    let input = bytes_from_json(json!({
        "id": "msg_thinking",
        "model": "claude-3-7-sonnet",
        "content": [
            { "type": "thinking", "thinking": "analyze first", "signature": "SIG_ANTHROPIC" },
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
    assert_eq!(output_items[0]["encrypted_content"], json!("SIG_ANTHROPIC"));
    assert_eq!(output_items[1]["type"], json!("message"));
    assert_eq!(output_items[1]["content"][0]["type"], json!("output_text"));
    assert_eq!(output_items[1]["content"][0]["text"], json!("final answer"));
    assert_eq!(output_items[2]["type"], json!("function_call"));
    assert_eq!(output_items[2]["call_id"], json!("call_1"));
    assert_eq!(output_items[2]["name"], json!("search"));
    assert_eq!(output_items[2]["arguments"], json!("{\"q\":\"x\"}"));
}

#[test]
fn anthropic_response_to_responses_adds_cache_tokens_to_openai_input_usage() {
    let input = bytes_from_json(json!({
        "id": "msg_cache_usage",
        "model": "claude-3-7-sonnet",
        "content": [
            { "type": "text", "text": "final answer" }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 3,
            "cache_read_input_tokens": 4,
            "cache_creation_input_tokens": 6
        }
    }));

    let output = anthropic_response_to_responses(&input).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["usage"]["input_tokens"], json!(20));
    assert_eq!(value["usage"]["output_tokens"], json!(3));
    assert_eq!(value["usage"]["total_tokens"], json!(23));
    assert_eq!(
        value["usage"]["input_tokens_details"]["cached_tokens"],
        json!(4)
    );
    assert_eq!(
        value["usage"]["input_tokens_details"]["cached_creation_tokens"],
        json!(6)
    );
}

#[test]
fn anthropic_response_to_responses_maps_redacted_thinking_to_encrypted_reasoning() {
    let input = bytes_from_json(json!({
        "id": "msg_redacted",
        "model": "claude-3-7-sonnet",
        "content": [
            { "type": "redacted_thinking", "data": "ENC123" },
            { "type": "text", "text": "final answer" }
        ],
        "usage": { "input_tokens": 2, "output_tokens": 4 }
    }));

    let output = anthropic_response_to_responses(&input).expect("transform");
    let value = json_from_bytes(output);

    let output_items = value["output"].as_array().expect("output array");
    assert_eq!(output_items[0]["type"], json!("reasoning"));
    assert_eq!(output_items[0]["summary"], json!([]));
    assert_eq!(
        output_items[0]["encrypted_content"],
        json!("claude-redacted-thinking:ENC123")
    );
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
                "encrypted_content": "SIG456"
            },
            {
                "id": "rs_2",
                "type": "reasoning",
                "summary": [],
                "encrypted_content": "claude-redacted-thinking:ENC456"
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
    assert_eq!(value["content"][0]["signature"], json!("SIG456"));
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
