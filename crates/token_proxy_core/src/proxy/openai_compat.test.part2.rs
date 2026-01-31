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
    assert_eq!(responses_value["contents"][0]["parts"][0]["text"], json!("hi"));
    assert_eq!(responses_value["systemInstruction"]["parts"][0]["text"], json!("sys"));
    assert_eq!(responses_value["generationConfig"]["maxOutputTokens"], json!(128));
    assert_eq!(responses_value["generationConfig"]["stopSequences"], json!(["a", "b"]));
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
    assert_eq!(gemini_value["input"][0]["content"][0]["text"], json!("hello"));
    assert_eq!(gemini_value["max_output_tokens"], json!(64));
    assert_eq!(gemini_value["top_p"], json!(0.8));
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
    assert_eq!(gemini_value["messages"][0]["content"][0]["text"], json!("ping"));
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
    assert_eq!(anthropic_value["systemInstruction"]["parts"][0]["text"], json!("guard"));
    assert_eq!(anthropic_value["contents"][0]["parts"][0]["text"], json!("yo"));
    assert_eq!(anthropic_value["generationConfig"]["maxOutputTokens"], json!(321));
    assert_eq!(anthropic_value["generationConfig"]["stopSequences"], json!(["x"]));
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
    assert_eq!(responses_value["candidates"][0]["content"]["parts"][0]["text"], json!("Hello"));
    assert_eq!(responses_value["usageMetadata"]["promptTokenCount"], json!(2));
    assert_eq!(responses_value["usageMetadata"]["candidatesTokenCount"], json!(3));
    assert_eq!(responses_value["usageMetadata"]["totalTokenCount"], json!(5));
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
    assert_eq!(anthropic_value["candidates"][0]["content"]["parts"][0]["text"], json!("Yo"));
    assert_eq!(anthropic_value["usageMetadata"]["promptTokenCount"], json!(4));
    assert_eq!(anthropic_value["usageMetadata"]["candidatesTokenCount"], json!(6));
    assert_eq!(anthropic_value["usageMetadata"]["totalTokenCount"], json!(10));
}
