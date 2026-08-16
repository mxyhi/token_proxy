use super::*;

#[test]
fn chat_request_to_responses_maps_common_fields() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let chat_messages = json!([
        { "role": "user", "content": "hi" },
        { "role": "assistant", "content": "hello" }
    ]);
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "messages": chat_messages,
        "stream": true,
        "temperature": 0.7,
        "top_p": 0.9,
        // Prefer `max_completion_tokens` over `max_tokens`.
        "max_tokens": 111,
        "max_completion_tokens": 222
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    let expected_input = json!([
        {
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "hi" }]
        },
        {
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "hello" }]
        }
    ]);

    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["input"], expected_input);
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["temperature"], json!(0.7));
    assert_eq!(value["top_p"], json!(0.9));
    assert_eq!(value["max_output_tokens"], json!(222));
    assert!(value.get("messages").is_none());
}

#[test]
fn chat_request_to_responses_strips_sampling_params_for_reasoning_models() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-5.4-mini",
        "messages": [{ "role": "user", "content": "hi" }],
        "temperature": 0.7,
        "top_p": 0.9
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("gpt-5.4-mini"));
    assert!(value.get("temperature").is_none());
    assert!(value.get("top_p").is_none());
}

#[test]
fn chat_request_to_responses_accepts_responses_shaped_body_when_transforming() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input_items = json!([
        {
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "hi" }]
        }
    ]);
    let input = bytes_from_json(json!({
        "model": "gpt-5.5",
        "input": input_items,
        "stream": true,
        "temperature": 0.7,
        "top_p": 0.9,
        "service_tier": "priority",
        "metadata": { "client": "cursor" },
        "stream_options": { "include_usage": true },
        "prompt_cache_retention": "24h",
        "safety_identifier": "sid_1"
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("gpt-5.5"));
    assert_eq!(value["input"], input_items);
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["service_tier"], json!("priority"));
    assert!(value.get("temperature").is_none());
    assert!(value.get("top_p").is_none());
    assert!(value.get("messages").is_none());
    assert!(value.get("metadata").is_none());
    assert!(value.get("stream_options").is_none());
    assert!(value.get("prompt_cache_retention").is_none());
    assert!(value.get("safety_identifier").is_none());
}

#[test]
fn anthropic_count_tokens_request_to_responses_input_tokens_filters_generation_fields() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-5.4",
        "system": "count only",
        "messages": [
            {
                "role": "user",
                "content": [{ "type": "text", "text": "hi from claude" }]
            }
        ],
        "temperature": 0.7,
        "top_p": 0.9,
        "stream": true
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::AnthropicCountTokensToResponsesInputTokens,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("gpt-5.4"));
    assert!(value.get("instructions").is_none());
    assert_eq!(value["input"][0]["role"], json!("developer"));
    assert_eq!(value["input"][0]["content"][0]["text"], json!("count only"));
    assert_eq!(value["input"][1]["role"], json!("user"));
    assert!(value.get("temperature").is_none());
    assert!(value.get("top_p").is_none());
    assert!(value.get("stream").is_none());
}

#[test]
fn responses_input_tokens_response_to_anthropic_count_tokens_maps_shape() {
    let value = transform_response_value(
        FormatTransform::ResponsesInputTokensToAnthropicCountTokens,
        json!({ "input_tokens": 42 }),
        None,
    );

    assert_eq!(value, json!({ "input_tokens": 42 }));
}

#[test]
fn responses_request_to_chat_maps_tools_and_tool_choice() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let parameters = json!({
        "type": "object",
        "properties": { "q": { "type": "string" } },
        "required": ["q"]
    });
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": "hello",
        "tools": [
            {
                "type": "function",
                "name": "search",
                "description": "Search something",
                "parameters": parameters
            }
        ],
        "tool_choice": { "type": "function", "name": "search" },
        "stream": false
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["tools"][0]["type"], json!("function"));
    assert_eq!(value["tools"][0]["function"]["name"], json!("search"));
    assert_eq!(
        value["tools"][0]["function"]["description"],
        json!("Search something")
    );
    assert_eq!(value["tools"][0]["function"]["parameters"], parameters);
    assert_eq!(value["tool_choice"]["type"], json!("function"));
    assert_eq!(value["tool_choice"]["function"]["name"], json!("search"));
}

#[test]
fn responses_namespace_identity_stays_aligned_for_chat_and_gemini() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let request = json!({
        "model": "model",
        "input": [
            { "type": "function_call", "call_id": "call_1", "namespace": "mcp__github", "name": "get_me", "arguments": "{}" },
            { "type": "function_call_output", "call_id": "call_1", "output": "ok" }
        ],
        "tools": [{
            "type": "namespace",
            "name": "mcp__github",
            "tools": [{ "type": "function", "name": "get_me", "parameters": { "type": "object" } }]
        }]
    });

    let chat = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &bytes_from_json(request.clone()),
            &http_clients,
            None,
        )
        .await
        .expect("chat transform")
    });
    let chat = json_from_bytes(chat);
    assert_eq!(chat["tools"][0]["function"]["name"], "mcp__github__get_me");
    assert_eq!(
        chat["messages"][0]["tool_calls"][0]["function"]["name"],
        chat["tools"][0]["function"]["name"]
    );

    let gemini = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToGemini,
            &bytes_from_json(request),
            &http_clients,
            None,
        )
        .await
        .expect("gemini transform")
    });
    let gemini = json_from_bytes(gemini);
    assert_eq!(
        gemini["tools"][0]["functionDeclarations"][0]["name"],
        "mcp__github__get_me"
    );
    assert_eq!(
        gemini["contents"][0]["parts"][1]["functionCall"]["name"],
        gemini["tools"][0]["functionDeclarations"][0]["name"]
    );
}

#[test]
fn responses_namespace_flattening_rejects_ambiguous_names() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "model",
        "input": "hi",
        "tools": [
            { "type": "function", "name": "mcp__github__get_me" },
            { "type": "namespace", "name": "mcp__github", "tools": [
                { "type": "function", "name": "get_me" }
            ] }
        ]
    }));

    let error = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect_err("ambiguous identity must fail")
    });
    assert!(error.contains("conflict"), "error={error}");
}

#[test]
fn chat_request_to_responses_maps_tools_and_tool_choice() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let parameters = json!({
        "type": "object",
        "properties": { "q": { "type": "string" } },
        "required": ["q"]
    });
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "search",
                    "description": "Search something",
                    "parameters": parameters
                }
            }
        ],
        "tool_choice": { "type": "function", "function": { "name": "search" } },
        "stream": false
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["tools"][0]["type"], json!("function"));
    assert_eq!(value["tools"][0]["name"], json!("search"));
    assert_eq!(value["tools"][0]["description"], json!("Search something"));
    assert_eq!(value["tools"][0]["parameters"], parameters);
    assert_eq!(value["tools"][0]["strict"], json!(false));
    assert_eq!(value["tool_choice"]["type"], json!("function"));
    assert_eq!(value["tool_choice"]["name"], json!("search"));
}

#[test]
fn responses_request_to_chat_instructions_becomes_system_message() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": "hello",
        "instructions": "be concise",
        "stream": false,
        "max_output_tokens": 99
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);
    let messages = value["messages"].as_array().expect("messages array");

    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["stream"], json!(false));
    assert_eq!(value["max_completion_tokens"], json!(99));
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], json!("system"));
    assert_eq!(messages[0]["content"], json!("be concise"));
    assert_eq!(messages[1]["role"], json!("user"));
    assert_eq!(messages[1]["content"], json!("hello"));
}

#[test]
fn responses_request_to_chat_accepts_message_array_input() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input_messages = json!([{ "role": "user", "content": "hi" }]);
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": input_messages,
        "stream": true
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["messages"], input_messages);
}

#[test]
fn responses_request_to_chat_converts_input_text_content_parts_to_string() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input_messages = json!([{
        "role": "user",
        "content": [
            { "type": "input_text", "text": "分析项目的逻辑缺陷和性能缺陷" }
        ]
    }]);
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": input_messages,
        "stream": false
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["messages"][0]["role"], json!("user"));
    assert_eq!(
        value["messages"][0]["content"],
        json!("分析项目的逻辑缺陷和性能缺陷")
    );
}

#[test]
fn chat_request_to_responses_maps_response_format() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let schema = json!({ "type": "object", "properties": { "ok": { "type": "boolean" } } });
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "messages": [{ "role": "user", "content": "hi" }],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "example",
                "schema": schema,
                "strict": true
            }
        }
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(value["text"]["format"]["name"], json!("example"));
    assert_eq!(value["text"]["format"]["schema"], schema);
    assert_eq!(value["text"]["format"]["strict"], json!(true));
}

#[test]
fn responses_request_to_chat_maps_text_format_to_response_format() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": "hi",
        "text": { "format": { "type": "json_object" } }
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["response_format"]["type"], json!("json_object"));
}

#[test]
fn responses_request_to_chat_maps_json_schema_text_format() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let schema = json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"]
    });
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": "hi",
        "text": {
            "format": {
                "type": "json_schema",
                "name": "answer_schema",
                "schema": schema,
                "strict": true
            }
        }
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["response_format"]["type"], json!("json_schema"));
    assert_eq!(
        value["response_format"]["json_schema"]["name"],
        json!("answer_schema")
    );
    assert_eq!(value["response_format"]["json_schema"]["schema"], schema);
    assert_eq!(
        value["response_format"]["json_schema"]["strict"],
        json!(true)
    );
}

#[test]
fn responses_response_to_chat_extracts_output_text_and_maps_usage() {
    let input = bytes_from_json(json!({
        "id": "resp_123",
        "created_at": 1700000000,
        "model": "gpt-4.1",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "Hello", "annotations": [] },
                    { "type": "output_text", "text": " world", "annotations": [] }
                ]
            }
        ],
        "usage": {
            "input_tokens": 1,
            "output_tokens": 2,
            "total_tokens": 3,
            "input_tokens_details": {
                "cached_tokens": 4,
                "cached_creation_tokens": 6,
                "audio_tokens": 5
            },
            "output_tokens_details": {
                "reasoning_tokens": 7,
                "audio_tokens": 8,
                "accepted_prediction_tokens": 9,
                "rejected_prediction_tokens": 10
            }
        }
    }));

    let output =
        transform_response_body(FormatTransform::ResponsesToChat, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["id"], json!("resp_123"));
    assert_eq!(value["object"], json!("chat.completion"));
    assert_eq!(value["created"], json!(1700000000));
    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["choices"][0]["message"]["role"], json!("assistant"));
    assert_eq!(
        value["choices"][0]["message"]["content"],
        json!("Hello world")
    );
    assert_eq!(value["choices"][0]["finish_reason"], json!("stop"));
    assert_eq!(value["usage"]["prompt_tokens"], json!(1));
    assert_eq!(value["usage"]["completion_tokens"], json!(2));
    assert_eq!(value["usage"]["total_tokens"], json!(3));
    assert_eq!(
        value["usage"]["prompt_tokens_details"]["cached_tokens"],
        json!(4)
    );
    assert_eq!(
        value["usage"]["prompt_tokens_details"]["audio_tokens"],
        json!(5)
    );
    assert_eq!(
        value["usage"]["prompt_tokens_details"]["cached_creation_tokens"],
        json!(6)
    );
    assert_eq!(
        value["usage"]["completion_tokens_details"]["reasoning_tokens"],
        json!(7)
    );
    assert_eq!(
        value["usage"]["completion_tokens_details"]["audio_tokens"],
        json!(8)
    );
    assert_eq!(
        value["usage"]["completion_tokens_details"]["accepted_prediction_tokens"],
        json!(9)
    );
    assert_eq!(
        value["usage"]["completion_tokens_details"]["rejected_prediction_tokens"],
        json!(10)
    );
}

#[test]
fn responses_response_to_chat_omits_empty_usage_details() {
    let input = bytes_from_json(json!({
        "id": "resp_zero_usage_details",
        "created_at": 1700000001,
        "model": "gpt-4.1",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "ok", "annotations": [] }
                ]
            }
        ],
        "usage": {
            "input_tokens": 1,
            "output_tokens": 2,
            "total_tokens": 3,
            "input_tokens_details": {
                "cached_tokens": 0,
                "cached_creation_tokens": 0,
                "audio_tokens": 0
            },
            "output_tokens_details": {
                "reasoning_tokens": 0,
                "audio_tokens": 0,
                "accepted_prediction_tokens": 0,
                "rejected_prediction_tokens": 0
            }
        }
    }));

    let output =
        transform_response_body(FormatTransform::ResponsesToChat, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert!(value["usage"].get("prompt_tokens_details").is_none());
    assert!(value["usage"].get("completion_tokens_details").is_none());
}

#[test]
fn responses_response_to_chat_maps_reasoning_content() {
    let input = bytes_from_json(json!({
        "id": "resp_reason",
        "created_at": 1700000002,
        "model": "gpt-4.1",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "reasoning_text", "text": "think", "annotations": [] },
                    { "type": "output_text", "text": "ok", "annotations": [] }
                ]
            }
        ]
    }));

    let output =
        transform_response_body(FormatTransform::ResponsesToChat, &input, None).expect("transform");
    let value = json_from_bytes(output);

    let message = &value["choices"][0]["message"];
    assert_eq!(message["content"], json!("ok"));
    assert_eq!(message["reasoning_content"], json!("think"));
}

#[test]
fn responses_response_to_chat_maps_reasoning_summary_and_annotations() {
    let input = bytes_from_json(json!({
        "id": "resp_reason_summary",
        "created_at": 1700000003,
        "model": "gpt-4.1",
        "output": [
            {
                "id": "rs_1",
                "type": "reasoning",
                "summary": [
                    { "type": "summary_text", "text": "analyze first" }
                ]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "done",
                        "annotations": [
                            {
                                "type": "url_citation",
                                "url": "https://example.com",
                                "title": "Example",
                                "start_index": 0,
                                "end_index": 4
                            }
                        ]
                    }
                ]
            }
        ]
    }));

    let output =
        transform_response_body(FormatTransform::ResponsesToChat, &input, None).expect("transform");
    let value = json_from_bytes(output);

    let message = &value["choices"][0]["message"];
    assert_eq!(message["content"], json!("done"));
    assert_eq!(message["reasoning_content"], json!("analyze first"));
    assert_eq!(message["annotations"][0]["type"], json!("url_citation"));
    assert_eq!(
        message["annotations"][0]["url"],
        json!("https://example.com")
    );
}

#[test]
fn responses_response_to_chat_maps_output_audio_and_thinking_blocks() {
    let input = bytes_from_json(json!({
        "id": "resp_audio_reasoning",
        "created_at": 1700000004,
        "model": "gpt-4.1",
        "output": [
            {
                "id": "rs_1",
                "type": "reasoning",
                "summary": [
                    { "type": "summary_text", "text": "analyze first" }
                ],
                "encrypted_content": "ENC123"
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_audio",
                        "audio": {
                            "data": "UklGRg==",
                            "transcript": "spoken"
                        }
                    },
                    {
                        "type": "output_text",
                        "text": "final answer",
                        "annotations": []
                    }
                ]
            }
        ]
    }));

    let output =
        transform_response_body(FormatTransform::ResponsesToChat, &input, None).expect("transform");
    let value = json_from_bytes(output);

    let message = &value["choices"][0]["message"];
    assert_eq!(message["content"], json!("final answer"));
    assert_eq!(message["audio"]["data"], json!("UklGRg=="));
    assert_eq!(message["audio"]["transcript"], json!("spoken"));
    assert_eq!(message["reasoning_content"], json!("analyze first"));
    assert_eq!(message["thinking_blocks"][0]["type"], json!("thinking"));
    assert_eq!(
        message["thinking_blocks"][0]["thinking"],
        json!("analyze first")
    );
    assert_eq!(
        message["thinking_blocks"][1],
        json!({ "type": "redacted_thinking", "data": "ENC123" })
    );
}

#[test]
fn responses_response_to_chat_includes_tool_calls_and_multimodal_content() {
    let input = bytes_from_json(json!({
        "id": "resp_456",
        "created_at": 1700000001,
        "model": "gpt-4.1",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "Hello", "annotations": [] },
                    { "type": "output_image", "image_url": { "url": "https://example.com/a.png" } }
                ]
            },
            {
                "type": "function_call",
                "call_id": "call_foo",
                "name": "doThing",
                "arguments": "{\"a\":1}"
            }
        ],
        "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
    }));

    let output =
        transform_response_body(FormatTransform::ResponsesToChat, &input, None).expect("transform");
    let value = json_from_bytes(output);

    let message = &value["choices"][0]["message"];
    assert_eq!(message["role"], json!("assistant"));
    assert_eq!(message["content"][0]["type"], json!("text"));
    assert_eq!(message["content"][0]["text"], json!("Hello"));
    assert_eq!(message["content"][1]["type"], json!("image_url"));
    assert_eq!(
        message["content"][1]["image_url"]["url"],
        json!("https://example.com/a.png")
    );
    assert_eq!(message["tool_calls"][0]["id"], json!("call_foo"));
    assert_eq!(
        message["tool_calls"][0]["function"]["name"],
        json!("doThing")
    );
    assert_eq!(
        message["tool_calls"][0]["function"]["arguments"],
        json!("{\"a\":1}")
    );
    assert_eq!(value["choices"][0]["finish_reason"], json!("tool_calls"));
}

#[test]
fn chat_response_to_responses_maps_audio_and_thinking_blocks() {
    let input = bytes_from_json(json!({
        "id": "chatcmpl_audio_reasoning",
        "created": 1700000005,
        "model": "gpt-4.1",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "final answer",
                    "audio": {
                        "data": "UklGRg==",
                        "transcript": "spoken"
                    },
                    "thinking_blocks": [
                        {
                            "type": "thinking",
                            "thinking": "analyze first",
                            "signature": "sig_1"
                        },
                        {
                            "type": "redacted_thinking",
                            "data": "ENC123"
                        }
                    ]
                }
            }
        ]
    }));

    let output =
        transform_response_body(FormatTransform::ChatToResponses, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["output"][0]["type"], json!("reasoning"));
    assert_eq!(
        value["output"][0]["summary"][0]["text"],
        json!("analyze first")
    );
    assert_eq!(value["output"][0]["encrypted_content"], json!("ENC123"));
    assert_eq!(value["output"][1]["type"], json!("message"));
    assert_eq!(
        value["output"][1]["content"][0],
        json!({
            "type": "output_text",
            "text": "final answer",
            "annotations": []
        })
    );
    assert_eq!(
        value["output"][1]["content"][1],
        json!({
            "type": "output_audio",
            "audio": {
                "data": "UklGRg==",
                "transcript": "spoken"
            }
        })
    );
}

#[test]
fn chat_response_to_responses_extracts_choice_text_and_maps_usage() {
    let input = bytes_from_json(json!({
        "id": "chatcmpl_123",
        "created": 1700000000,
        "model": "gpt-4.1",
        "choices": [
            { "index": 0, "message": { "role": "assistant", "content": "Hello" } }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 2,
            "total_tokens": 3,
            "completion_tokens_details": { "reasoning_tokens": 5 }
        }
    }));

    let output =
        transform_response_body(FormatTransform::ChatToResponses, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["id"], json!("chatcmpl_123"));
    assert_eq!(value["object"], json!("response"));
    assert_eq!(value["created_at"], json!(1700000000));
    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["output"][0]["type"], json!("message"));
    assert_eq!(value["output"][0]["role"], json!("assistant"));
    assert_eq!(
        value["output"][0]["content"][0]["type"],
        json!("output_text")
    );
    assert_eq!(value["output"][0]["content"][0]["text"], json!("Hello"));
    assert_eq!(value["usage"]["input_tokens"], json!(1));
    assert_eq!(value["usage"]["output_tokens"], json!(2));
    assert_eq!(value["usage"]["total_tokens"], json!(3));
    assert_eq!(
        value["usage"]["output_tokens_details"]["reasoning_tokens"],
        json!(5)
    );
}

#[test]
fn chat_response_to_responses_preserves_reasoning_and_annotations() {
    let input = bytes_from_json(json!({
        "id": "chatcmpl_reasoning",
        "created": 1700000001,
        "model": "gpt-4.1",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                    "reasoning_content": "think before answer",
                    "annotations": [
                        {
                            "type": "url_citation",
                            "url": "https://example.com",
                            "title": "Example",
                            "start_index": 0,
                            "end_index": 5
                        }
                    ]
                }
            }
        ]
    }));

    let output =
        transform_response_body(FormatTransform::ChatToResponses, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["output"][0]["type"], json!("reasoning"));
    assert_eq!(
        value["output"][0]["summary"][0]["text"],
        json!("think before answer")
    );
    assert_eq!(value["output"][1]["type"], json!("message"));
    assert_eq!(
        value["output"][1]["content"][0]["type"],
        json!("output_text")
    );
    assert_eq!(value["output"][1]["content"][0]["text"], json!("Hello"));
    assert_eq!(
        value["output"][1]["content"][0]["annotations"][0]["type"],
        json!("url_citation")
    );
}

#[test]
fn chat_response_to_responses_maps_finish_reason_to_incomplete_details() {
    let input = bytes_from_json(json!({
        "id": "chatcmpl_456",
        "created": 1700000002,
        "model": "gpt-4.1",
        "choices": [
            { "index": 0, "message": { "role": "assistant", "content": "Hello" }, "finish_reason": "length" }
        ],
        "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 }
    }));

    let output =
        transform_response_body(FormatTransform::ChatToResponses, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["status"], json!("incomplete"));
    assert_eq!(value["incomplete_details"]["reason"], json!("max_tokens"));
}

#[test]
fn responses_request_to_chat_converts_function_call_output_to_tool_message() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            { "type": "function_call_output", "call_id": "call_123", "output": "ok" }
        ],
        "stream": false
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);
    let messages = value["messages"].as_array().expect("messages array");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], json!("tool"));
    assert_eq!(messages[0]["tool_call_id"], json!("call_123"));
    assert_eq!(messages[0]["content"], json!("ok"));
}

#[test]
fn responses_request_to_chat_converts_new_tool_output_types_to_tool_messages() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-4.1",
        "input": [
            { "type": "tool_search_output", "call_id": "call_search", "output": "search ok" },
            { "type": "custom_tool_call_output", "call_id": "call_custom", "output": "custom ok" },
            { "type": "mcp_tool_call_output", "call_id": "call_mcp", "output": "mcp ok" }
        ],
        "stream": false
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToChat,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);
    let messages = value["messages"].as_array().expect("messages array");

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], json!("tool"));
    assert_eq!(messages[0]["tool_call_id"], json!("call_search"));
    assert_eq!(messages[0]["content"], json!("search ok"));
    assert_eq!(messages[1]["tool_call_id"], json!("call_custom"));
    assert_eq!(messages[1]["content"], json!("custom ok"));
    assert_eq!(messages[2]["tool_call_id"], json!("call_mcp"));
    assert_eq!(messages[2]["content"], json!("mcp ok"));
}

#[test]
fn chat_response_to_responses_maps_tool_calls_into_output() {
    let input = bytes_from_json(json!({
        "id": "chatcmpl_123",
        "created": 1700000000,
        "model": "gpt-4.1",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_foo",
                            "type": "function",
                            "function": {
                                "name": "getRandomNumber",
                                "arguments": "{\"a\":\"0\"}"
                            }
                        }
                    ]
                }
            }
        ],
        "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 }
    }));

    let output =
        transform_response_body(FormatTransform::ChatToResponses, &input, None).expect("transform");
    let value = json_from_bytes(output);

    assert_eq!(value["id"], json!("chatcmpl_123"));
    assert_eq!(value["object"], json!("response"));
    assert_eq!(value["created_at"], json!(1700000000));
    assert_eq!(value["model"], json!("gpt-4.1"));
    assert_eq!(value["output"][0]["type"], json!("function_call"));
    assert_eq!(value["output"][0]["call_id"], json!("call_foo"));
    assert_eq!(value["output"][0]["name"], json!("getRandomNumber"));
    assert_eq!(value["output"][0]["arguments"], json!("{\"a\":\"0\"}"));
    assert_eq!(value["usage"]["input_tokens"], json!(1));
    assert_eq!(value["usage"]["output_tokens"], json!(2));
    assert_eq!(value["usage"]["total_tokens"], json!(3));
}

#[test]
fn chat_request_to_responses_rejects_missing_messages() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({ "model": "gpt-4.1" }));
    let err = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect_err("should fail")
    });
    assert!(err.contains("messages"));
}

#[test]
fn transform_request_body_rejects_non_json() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = Bytes::from_static(b"not-json");
    let err = run_async(async {
        transform_request_body(
            FormatTransform::ChatToResponses,
            &input,
            &http_clients,
            None,
        )
        .await
        .expect_err("should fail")
    });
    assert!(err.contains("JSON"));
}

#[test]
fn responses_to_codex_keeps_reasoning_include() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let input = bytes_from_json(json!({
        "model": "gpt-5.4",
        "input": "keep include"
    }));

    let output = run_async(async {
        transform_request_body(
            FormatTransform::ResponsesToCodex,
            &input,
            &http_clients,
            Some("gpt-5-codex"),
        )
        .await
        .expect("transform")
    });
    let value = json_from_bytes(output);

    assert_eq!(value["include"], json!(["reasoning.encrypted_content"]));
}
