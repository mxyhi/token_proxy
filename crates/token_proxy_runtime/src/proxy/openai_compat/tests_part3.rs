use super::*;

#[test]
fn anthropic_to_chat_request_preserves_chat_token_limit_without_responses_floor() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::AnthropicToChat,
        json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 8,
            "stream": true,
            "system": "guard",
            "thinking": { "type": "adaptive" },
            "output_config": { "effort": "high" },
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] }
            ]
        }),
        &http_clients,
        None,
    );

    assert_eq!(value["messages"][0]["role"], json!("developer"));
    assert_eq!(value["messages"][0]["content"], json!("guard"));
    assert_eq!(value["messages"][1]["role"], json!("user"));
    assert_eq!(value["messages"][1]["content"], json!("hi"));
    assert_eq!(value["max_completion_tokens"], json!(8));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["reasoning_effort"], json!("high"));
}

#[test]
fn anthropic_to_gemini_request_preserves_gemini_token_limit_without_responses_floor() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::AnthropicToGemini,
        json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 8,
            "system": "guard",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] }
            ]
        }),
        &http_clients,
        None,
    );

    assert_eq!(
        value["systemInstruction"]["parts"][0]["text"],
        json!("guard")
    );
    assert_eq!(value["contents"][0]["parts"][0]["text"], json!("hi"));
    assert_eq!(value["generationConfig"]["maxOutputTokens"], json!(8));
}

#[test]
fn chat_to_anthropic_request_preserves_anthropic_token_limit_without_responses_floor() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ChatToAnthropic,
        json!({
            "model": "gpt-5",
            "max_completion_tokens": 8,
            "stream": true,
            "messages": [
                { "role": "system", "content": "guard" },
                { "role": "user", "content": "hi" }
            ]
        }),
        &http_clients,
        Some("claude-3-5-sonnet"),
    );

    assert_eq!(value["model"], json!("gpt-5"));
    assert_eq!(value["system"][0]["text"], json!("guard"));
    assert_eq!(value["messages"][0]["role"], json!("user"));
    assert_eq!(value["messages"][0]["content"][0]["text"], json!("hi"));
    assert_eq!(value["max_tokens"], json!(8));
    assert_eq!(value["stream"], json!(true));
}

#[test]
fn anthropic_to_codex_request_uses_codex_model_hint_and_sanitizes_payload() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::AnthropicToCodex,
        json!({
            "model": "claude-3-5-sonnet",
            "max_tokens": 8,
            "system": "guard",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] }
            ]
        }),
        &http_clients,
        Some("gpt-5.5"),
    );

    assert_eq!(value["model"], json!("gpt-5.5"));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["store"], json!(false));
    assert_eq!(value["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(value["instructions"], json!("guard"));
    assert_eq!(value["input"][0]["role"], json!("developer"));
    assert_eq!(value["input"][1]["role"], json!("user"));
    assert!(value.get("max_output_tokens").is_none());
    assert!(value["prompt_cache_key"]
        .as_str()
        .is_some_and(|key| !key.is_empty()));
}

#[test]
fn images_generations_to_codex_request_builds_responses_image_tool_payload() {
    let http_clients = ProxyHttpClients::new().expect("http clients");
    let value = transform_request_value(
        FormatTransform::ImagesGenerationsToCodex,
        json!({
            "model": "gpt-image-2",
            "prompt": "draw a cat",
            "size": "1024x1024",
            "quality": "high",
            "background": "transparent",
            "output_format": "webp",
            "partial_images": 2,
            "n": 1
        }),
        &http_clients,
        None,
    );

    assert_eq!(value["model"], json!("gpt-5.4-mini"));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["store"], json!(false));
    assert_eq!(value["tool_choice"]["type"], json!("image_generation"));
    assert_eq!(value["tools"][0]["type"], json!("image_generation"));
    assert_eq!(value["tools"][0]["model"], json!("gpt-image-2"));
    assert_eq!(value["tools"][0]["output_format"], json!("webp"));
    assert_eq!(value["tools"][0]["partial_images"], json!(2));
    assert_eq!(value["input"][0]["content"][0]["text"], json!("draw a cat"));
}

#[test]
fn codex_to_anthropic_response_maps_responses_output_to_claude_message() {
    let value = transform_response_value(
        FormatTransform::CodexToAnthropic,
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 1710000000,
                "model": "gpt-5.5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": "hello" }]
                    }
                ],
                "usage": { "input_tokens": 2, "output_tokens": 3, "total_tokens": 5 }
            }
        }),
        Some("claude-3-5-sonnet"),
    );

    assert_eq!(value["type"], json!("message"));
    assert_eq!(value["model"], json!("claude-3-5-sonnet"));
    assert_eq!(value["content"][0]["type"], json!("text"));
    assert_eq!(value["content"][0]["text"], json!("hello"));
    assert_eq!(value["usage"]["input_tokens"], json!(2));
    assert_eq!(value["usage"]["output_tokens"], json!(3));
}

#[test]
fn codex_to_images_generations_response_maps_image_call_to_openai_images_shape() {
    let value = transform_response_value(
        FormatTransform::CodexToImagesGenerations,
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_img",
                "object": "response",
                "created_at": 1710000000,
                "model": "gpt-5.4-mini",
                "status": "completed",
                "tools": [
                    {
                        "type": "image_generation",
                        "model": "gpt-image-2",
                        "size": "1024x1024",
                        "quality": "high",
                        "output_format": "png"
                    }
                ],
                "output": [
                    {
                        "type": "image_generation_call",
                        "id": "ig_1",
                        "result": "BASE64PNG",
                        "revised_prompt": "draw cat"
                    }
                ],
                "usage": { "input_tokens": 2, "output_tokens": 4, "total_tokens": 6 }
            }
        }),
        None,
    );

    assert_eq!(value["created"], json!(1710000000));
    assert_eq!(value["model"], json!("gpt-image-2"));
    assert_eq!(value["size"], json!("1024x1024"));
    assert_eq!(value["quality"], json!("high"));
    assert_eq!(value["output_format"], json!("png"));
    assert_eq!(value["data"][0]["b64_json"], json!("BASE64PNG"));
    assert_eq!(value["data"][0]["revised_prompt"], json!("draw cat"));
    assert_eq!(value["usage"]["total_tokens"], json!(6));
}

#[test]
fn anthropic_disabled_thinking_survives_openai_bridges() {
    let clients = ProxyHttpClients::new().expect("http clients");
    for transform in [
        FormatTransform::AnthropicToResponses,
        FormatTransform::AnthropicToChat,
        FormatTransform::AnthropicToCodex,
    ] {
        let value = transform_request_value(
            transform,
            json!({
                "model": "gpt-6-astra", "max_tokens": 123,
                "thinking": {"type": "disabled", "display": "summarized"},
                "output_config": {"effort": "high"},
                "messages": [{"role": "user", "content": "hi"}]
            }),
            &clients,
            None,
        );
        if matches!(transform, FormatTransform::AnthropicToChat) {
            assert_eq!(value["reasoning_effort"], "none");
        } else {
            assert_eq!(value["reasoning"], json!({"effort": "none"}));
        }
    }
    let unspecified = transform_request_value(
        FormatTransform::AnthropicToResponses,
        json!({
            "model": "gpt-6-astra", "max_tokens": 123,
            "messages": [{"role": "user", "content": "hi"}]
        }),
        &clients,
        None,
    );
    assert!(unspecified.get("reasoning").is_none());
}

#[test]
fn gpt_generation_sampling_contract_survives_openai_bridges() {
    let clients = ProxyHttpClients::new().expect("http clients");
    for (model, strip) in [
        ("gpt-5.4-mini", true),
        ("gpt-6", true),
        ("gpt-6-astra", true),
        ("openai/gpt-6-astra-20260901", true),
        (" GPT-6-ASTRA ", true),
        ("gpt-7", true),
        ("gpt-4.1", false),
        ("gpt-image-1", false),
        ("gpt-audio", false),
        ("custom-gpt-6", false),
    ] {
        for transform in [
            FormatTransform::ChatToResponses,
            FormatTransform::AnthropicToResponses,
        ] {
            let value = transform_request_value(
                transform,
                json!({
                    "model": model, "max_tokens": 123, "temperature": 0.7, "top_p": 0.9,
                    "messages": [{"role": "user", "content": "hi"}]
                }),
                &clients,
                None,
            );
            assert_eq!(value.get("temperature").is_none(), strip, "model={model}");
            assert_eq!(value.get("top_p").is_none(), strip, "model={model}");
        }
    }
}
