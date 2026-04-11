use super::*;

#[test]
fn responses_and_gemini_request_conversions() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let responses_value = transform_request_value(
        FormatTransform::ResponsesToGemini,
        json!({
            "model": "gpt-4.1",
            "input": "hi",
            "instructions": "sys",
            "temperature": 0.5,
            "top_p": 0.9,
            "max_output_tokens": 128,
            "stop": ["a", "b"],
            "seed": 7
        }),
        &http_clients,
        None,
    );
    assert_eq!(
        responses_value["contents"][0]["parts"][0]["text"],
        json!("hi")
    );
    assert_eq!(
        responses_value["systemInstruction"]["parts"][0]["text"],
        json!("sys")
    );
    assert_eq!(
        responses_value["generationConfig"]["maxOutputTokens"],
        json!(128)
    );
    assert_eq!(
        responses_value["generationConfig"]["stopSequences"],
        json!(["a", "b"])
    );
    assert_eq!(responses_value["generationConfig"]["seed"], json!(7));
    let gemini_value = transform_request_value(
        FormatTransform::GeminiToResponses,
        json!({
            "model": "gemini-1.5-flash",
            "contents": [{ "role": "user", "parts": [{ "text": "hello" }] }],
            "systemInstruction": { "parts": [{ "text": "rules" }] },
            "generationConfig": { "maxOutputTokens": 64, "topP": 0.8 }
        }),
        &http_clients,
        None,
    );
    assert_eq!(gemini_value["model"], json!("gemini-1.5-flash"));
    assert_eq!(gemini_value["instructions"], json!("rules"));
    assert_eq!(
        gemini_value["input"][0]["content"][0]["text"],
        json!("hello")
    );
    assert_eq!(gemini_value["max_output_tokens"], json!(64));
    assert_eq!(gemini_value["top_p"], json!(0.8));
}

#[test]
fn gemini_to_responses_request_preserves_audio_and_file_parts() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let gemini_value = transform_request_value(
        FormatTransform::GeminiToResponses,
        json!({
            "contents": [{
                "role": "user",
                "parts": [
                    {
                        "inlineData": {
                            "mimeType": "audio/wav",
                            "data": "UklGRg=="
                        }
                    },
                    {
                        "fileData": {
                            "mimeType": "application/pdf",
                            "fileUri": "https://example.com/spec.pdf"
                        }
                    }
                ]
            }]
        }),
        &http_clients,
        Some("gemini-2.0-flash"),
    );

    let content = gemini_value["input"][0]["content"]
        .as_array()
        .expect("responses content");
    assert_eq!(content[0]["type"], json!("input_audio"));
    assert_eq!(content[0]["input_audio"]["data"], json!("UklGRg=="));
    assert_eq!(content[1]["type"], json!("input_file"));
    assert_eq!(
        content[1]["file_url"],
        json!("https://example.com/spec.pdf")
    );
}

#[test]
fn chat_request_to_responses_maps_advanced_optional_params() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let schema = json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"]
    });
    let value = transform_request_value(
        FormatTransform::ChatToResponses,
        json!({
            "model": "gpt-5",
            "messages": [{ "role": "user", "content": "hi" }],
            "reasoning_effort": "high",
            "previous_response_id": "resp_prev_123",
            "web_search_options": { "search_context_size": "high" },
            "store": false,
            "background": true,
            "include": ["reasoning.encrypted_content"],
            "truncation": "auto",
            "service_tier": "flex",
            "safety_identifier": "sid_123",
            "prompt": { "id": "pmpt_123", "version": "42" },
            "max_tool_calls": 3,
            "prompt_cache_key": "cache-key",
            "prompt_cache_retention": "24h",
            "stream_options": { "include_usage": true },
            "top_logprobs": 5,
            "partial_images": 2,
            "context_management": [{ "type": "compaction", "compact_threshold": 4096 }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "answer_schema",
                    "schema": schema,
                    "strict": true
                }
            }
        }),
        &http_clients,
        None,
    );

    assert_eq!(value["reasoning"]["effort"], json!("high"));
    assert_eq!(value["previous_response_id"], json!("resp_prev_123"));
    assert_eq!(value["store"], json!(false));
    assert_eq!(value["background"], json!(true));
    assert_eq!(value["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(value["truncation"], json!("auto"));
    assert_eq!(value["service_tier"], json!("flex"));
    assert_eq!(value["safety_identifier"], json!("sid_123"));
    assert_eq!(value["prompt"]["id"], json!("pmpt_123"));
    assert_eq!(value["max_tool_calls"], json!(3));
    assert_eq!(value["prompt_cache_key"], json!("cache-key"));
    assert_eq!(value["prompt_cache_retention"], json!("24h"));
    assert_eq!(value["stream_options"]["include_usage"], json!(true));
    assert_eq!(value["top_logprobs"], json!(5));
    assert_eq!(value["partial_images"], json!(2));
    assert_eq!(
        value["context_management"][0]["compact_threshold"],
        json!(4096)
    );
    assert_eq!(value["text"]["format"]["name"], json!("answer_schema"));
    assert_eq!(value["text"]["format"]["schema"], schema);
    assert_eq!(value["text"]["format"]["strict"], json!(true));
    assert_eq!(value["tools"][0]["type"], json!("web_search"));
    assert_eq!(value["tools"][0]["search_context_size"], json!("high"));
}

#[test]
fn responses_request_to_chat_maps_reasoning_web_search_and_tool_metadata() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let schema = json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } }
    });
    let value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": "hi",
            "reasoning": { "effort": "medium", "summary": "detailed" },
            "service_tier": "priority",
            "context_management": [{ "type": "compaction", "compact_threshold": 3000 }],
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "answer_schema",
                    "schema": schema,
                    "strict": true
                }
            },
            "tools": [
                {
                    "type": "web_search",
                    "search_context_size": "high",
                    "user_location": {
                        "type": "approximate",
                        "country": "US",
                        "city": "San Francisco"
                    }
                },
                {
                    "type": "function",
                    "name": "lookup",
                    "description": "Lookup info",
                    "parameters": { "properties": { "q": { "type": "string" } } },
                    "strict": true,
                    "cache_control": { "type": "ephemeral" },
                    "allowed_callers": ["ui"],
                    "input_examples": [{ "q": "hello" }]
                }
            ]
        }),
        &http_clients,
        None,
    );

    assert_eq!(value["reasoning_effort"], json!("medium"));
    assert_eq!(value["service_tier"], json!("priority"));
    assert_eq!(
        value["context_management"][0]["compact_threshold"],
        json!(3000)
    );
    assert_eq!(
        value["web_search_options"]["search_context_size"],
        json!("high")
    );
    assert_eq!(
        value["web_search_options"]["user_location"]["city"],
        json!("San Francisco")
    );
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
    assert_eq!(value["tools"][0]["function"]["name"], json!("lookup"));
    assert_eq!(value["tools"][0]["function"]["strict"], json!(true));
    assert_eq!(
        value["tools"][0]["cache_control"]["type"],
        json!("ephemeral")
    );
    assert_eq!(value["tools"][0]["allowed_callers"], json!(["ui"]));
    assert_eq!(value["tools"][0]["input_examples"][0]["q"], json!("hello"));
}

#[test]
fn chat_request_to_responses_preserves_tool_metadata() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ChatToResponses,
        json!({
            "model": "gpt-4.1",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                {
                    "type": "function",
                    "cache_control": { "type": "ephemeral" },
                    "allowed_callers": ["ui"],
                    "input_examples": [{ "q": "hello" }],
                    "function": {
                        "name": "lookup",
                        "description": "Lookup info",
                        "parameters": { "properties": { "q": { "type": "string" } } },
                        "strict": true
                    }
                }
            ]
        }),
        &http_clients,
        None,
    );

    assert_eq!(value["tools"][0]["name"], json!("lookup"));
    assert_eq!(value["tools"][0]["strict"], json!(true));
    assert_eq!(
        value["tools"][0]["cache_control"]["type"],
        json!("ephemeral")
    );
    assert_eq!(value["tools"][0]["allowed_callers"], json!(["ui"]));
    assert_eq!(value["tools"][0]["input_examples"][0]["q"], json!("hello"));
}

#[test]
fn responses_request_to_chat_normalizes_tool_choice_object() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let auto_value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": "hi",
            "tool_choice": { "type": "auto" }
        }),
        &http_clients,
        None,
    );
    assert_eq!(auto_value["tool_choice"], json!("auto"));

    let required_value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": "hi",
            "tool_choice": { "type": "tool" }
        }),
        &http_clients,
        None,
    );
    assert_eq!(required_value["tool_choice"], json!("required"));

    let nameless_function_value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": "hi",
            "tool_choice": { "type": "function" }
        }),
        &http_clients,
        None,
    );
    assert_eq!(nameless_function_value["tool_choice"], json!("required"));
}

#[test]
fn chat_request_to_responses_preserves_structured_tool_output() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ChatToResponses,
        json!({
            "model": "gpt-4.1",
            "messages": [
                { "role": "user", "content": "show image result" },
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": [
                        { "type": "text", "text": "done" },
                        { "type": "image_url", "image_url": { "url": "https://example.com/result.png" } }
                    ]
                }
            ]
        }),
        &http_clients,
        None,
    );

    assert_eq!(value["input"][1]["type"], json!("function_call_output"));
    assert_eq!(value["input"][1]["call_id"], json!("call_123"));
    assert_eq!(value["input"][1]["output"][0]["type"], json!("input_text"));
    assert_eq!(value["input"][1]["output"][0]["text"], json!("done"));
    assert_eq!(value["input"][1]["output"][1]["type"], json!("input_image"));
    assert_eq!(
        value["input"][1]["output"][1]["image_url"]["url"],
        json!("https://example.com/result.png")
    );
}
#[test]
fn responses_request_to_chat_accepts_additional_tool_output_item_types() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": [
                { "type": "tool_result", "call_id": "call_tool", "output": "tool ok" },
                { "type": "computer_call_output", "call_id": "call_computer", "output": "computer ok" },
                { "type": "web_search_call", "call_id": "call_search", "output": "search ok" }
            ]
        }),
        &http_clients,
        None,
    );

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], json!("tool"));
    assert_eq!(messages[0]["tool_call_id"], json!("call_tool"));
    assert_eq!(messages[0]["content"], json!("tool ok"));
    assert_eq!(messages[1]["tool_call_id"], json!("call_computer"));
    assert_eq!(messages[2]["tool_call_id"], json!("call_search"));
}

#[test]
fn responses_request_to_chat_skips_tool_output_without_call_id() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": [
                { "type": "function_call_output", "output": "ignored" },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }
            ]
        }),
        &http_clients,
        None,
    );

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(messages[0]["content"], json!("hi"));
}

#[test]
fn responses_request_to_chat_normalizes_object_tool_output_to_string() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": [
                {
                    "type": "function_call_output",
                    "call_id": "call_123",
                    "output": { "status": "ok", "count": 2 }
                }
            ]
        }),
        &http_clients,
        None,
    );

    let messages = value["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], json!("tool"));
    assert_eq!(messages[0]["tool_call_id"], json!("call_123"));
    let content = messages[0]["content"].as_str().expect("string content");
    assert_eq!(
        serde_json::from_str::<Value>(content).expect("json string"),
        json!({ "status": "ok", "count": 2 })
    );
}

#[test]
fn chat_request_to_responses_drops_unknown_response_format() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ChatToResponses,
        json!({
            "model": "gpt-5",
            "messages": [{ "role": "user", "content": "hi" }],
            "response_format": { "type": "xml_schema", "schema": { "type": "string" } }
        }),
        &http_clients,
        None,
    );

    assert!(value.get("text").is_none());
}

#[test]
fn responses_request_to_chat_drops_unknown_text_format() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ResponsesToChat,
        json!({
            "model": "gpt-5",
            "input": "hi",
            "text": { "format": { "type": "xml_schema", "schema": { "type": "string" } } }
        }),
        &http_clients,
        None,
    );

    assert!(value.get("response_format").is_none());
}

#[test]
fn gemini_and_anthropic_request_conversions() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let gemini_value = transform_request_value(
        FormatTransform::GeminiToAnthropic,
        json!({
            "contents": [{ "role": "user", "parts": [{ "text": "ping" }] }],
            "systemInstruction": { "parts": [{ "text": "sys" }] },
            "generationConfig": { "maxOutputTokens": 42 }
        }),
        &http_clients,
        Some("claude-3-5-sonnet"),
    );
    assert_eq!(gemini_value["model"], json!("claude-3-5-sonnet"));
    assert_eq!(gemini_value["system"][0]["text"], json!("sys"));
    assert_eq!(
        gemini_value["messages"][0]["content"][0]["text"],
        json!("ping")
    );
    assert_eq!(gemini_value["max_tokens"], json!(42));
    let anthropic_value = transform_request_value(
        FormatTransform::AnthropicToGemini,
        json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 321,
            "system": "guard",
            "stop_sequences": ["x"],
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "yo" }] }]
        }),
        &http_clients,
        None,
    );
    assert_eq!(
        anthropic_value["systemInstruction"]["parts"][0]["text"],
        json!("guard")
    );
    assert_eq!(
        anthropic_value["contents"][0]["parts"][0]["text"],
        json!("yo")
    );
    assert_eq!(
        anthropic_value["generationConfig"]["maxOutputTokens"],
        json!(321)
    );
    assert_eq!(
        anthropic_value["generationConfig"]["stopSequences"],
        json!(["x"])
    );
}
#[test]
fn responses_and_gemini_response_conversions() {
    let responses_value = transform_response_value(
        FormatTransform::ResponsesToGemini,
        json!({
            "id": "resp_1",
            "created_at": 1700000000,
            "model": "gpt-4.1",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Hello", "annotations": [] }]
                }
            ],
            "usage": { "input_tokens": 2, "output_tokens": 3, "total_tokens": 5 }
        }),
        None,
    );
    assert_eq!(
        responses_value["candidates"][0]["content"]["parts"][0]["text"],
        json!("Hello")
    );
    assert_eq!(
        responses_value["usageMetadata"]["promptTokenCount"],
        json!(2)
    );
    assert_eq!(
        responses_value["usageMetadata"]["candidatesTokenCount"],
        json!(3)
    );
    assert_eq!(
        responses_value["usageMetadata"]["totalTokenCount"],
        json!(5)
    );
    let gemini_value = transform_response_value(
        FormatTransform::GeminiToResponses,
        json!({
            "candidates": [
                { "content": { "role": "model", "parts": [{ "text": "Hi" }] }, "finishReason": "STOP" }
            ],
            "usageMetadata": {
                "promptTokenCount": 4,
                "candidatesTokenCount": 6,
                "totalTokenCount": 10
            }
        }),
        Some("gemini-1.5-pro"),
    );
    assert_eq!(gemini_value["output"][0]["content"][0]["text"], json!("Hi"));
    assert_eq!(gemini_value["usage"]["input_tokens"], json!(4));
    assert_eq!(gemini_value["usage"]["output_tokens"], json!(6));
    assert_eq!(gemini_value["usage"]["total_tokens"], json!(10));
}
#[test]
fn gemini_and_anthropic_response_conversions() {
    let gemini_value = transform_response_value(
        FormatTransform::GeminiToAnthropic,
        json!({
            "candidates": [
                { "content": { "role": "model", "parts": [{ "text": "Howdy" }] }, "finishReason": "STOP" }
            ],
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        }),
        Some("claude-3-5-sonnet"),
    );
    assert_eq!(gemini_value["model"], json!("claude-3-5-sonnet"));
    assert_eq!(gemini_value["content"][0]["text"], json!("Howdy"));
    assert_eq!(gemini_value["usage"]["input_tokens"], json!(1));
    assert_eq!(gemini_value["usage"]["output_tokens"], json!(2));
    assert_eq!(gemini_value["stop_reason"], json!("end_turn"));
    let anthropic_value = transform_response_value(
        FormatTransform::AnthropicToGemini,
        json!({
            "id": "msg_1",
            "model": "claude-3-5-sonnet",
            "content": [{ "type": "text", "text": "Yo" }],
            "usage": { "input_tokens": 4, "output_tokens": 6 }
        }),
        None,
    );
    assert_eq!(
        anthropic_value["candidates"][0]["content"]["parts"][0]["text"],
        json!("Yo")
    );
    assert_eq!(
        anthropic_value["usageMetadata"]["promptTokenCount"],
        json!(4)
    );
    assert_eq!(
        anthropic_value["usageMetadata"]["candidatesTokenCount"],
        json!(6)
    );
    assert_eq!(
        anthropic_value["usageMetadata"]["totalTokenCount"],
        json!(10)
    );
}
