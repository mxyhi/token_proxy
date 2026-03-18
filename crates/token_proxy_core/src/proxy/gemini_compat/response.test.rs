use super::*;

#[test]
fn chat_response_to_gemini_maps_tool_calls_and_text() {
    let input = json!({
        "id": "chatcmpl_x",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "hello",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "getFoo", "arguments": "{\"a\":1}" }
                }]
            },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 }
    });

    let output = chat_response_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap()), None)
        .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(
        value["candidates"][0]["content"]["parts"][0]["text"],
        json!("hello")
    );
    assert_eq!(
        value["candidates"][0]["content"]["parts"][1]["functionCall"]["name"],
        json!("getFoo")
    );
    assert_eq!(value["usageMetadata"]["totalTokenCount"], json!(3));
}

#[test]
fn gemini_response_to_chat_preserves_reasoning_images_annotations_and_tool_calls() {
    let input = json!({
        "candidates": [{
            "content": {
                "parts": [
                    {
                        "text": "reason first",
                        "thought": true,
                        "thoughtSignature": "sig_reason"
                    },
                    { "text": "final answer" },
                    {
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": "aGVsbG8="
                        }
                    },
                    {
                        "functionCall": {
                            "name": "search",
                            "args": { "q": "x" }
                        },
                        "thoughtSignature": "sig_tool"
                    }
                ]
            },
            "finishReason": "STOP",
            "groundingMetadata": {
                "groundingSupports": [{
                    "segment": { "startIndex": 0, "endIndex": 5 },
                    "groundingChunkIndices": [0]
                }],
                "groundingChunks": [{
                    "web": {
                        "uri": "https://example.com",
                        "title": "Example"
                    }
                }]
            }
        }],
        "usageMetadata": {
            "promptTokenCount": 1,
            "candidatesTokenCount": 2,
            "totalTokenCount": 3
        }
    });

    let output = gemini_response_to_chat(
        &Bytes::from(serde_json::to_vec(&input).unwrap()),
        Some("gemini-2.0-flash"),
    )
    .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");

    let message = &value["choices"][0]["message"];
    let content = message["content"].as_array().expect("structured content");
    assert_eq!(
        content[0],
        json!({ "type": "text", "text": "final answer" })
    );
    assert_eq!(
        content[1]["image_url"]["url"],
        json!("data:image/png;base64,aGVsbG8=")
    );
    assert_eq!(message["reasoning_content"], json!("reason first"));
    assert_eq!(
        message["tool_calls"][0]["function"]["name"],
        json!("search")
    );
    assert_eq!(message["annotations"][0]["type"], json!("url_citation"));
    assert_eq!(
        message["annotations"][0]["url"],
        json!("https://example.com")
    );
    assert_eq!(
        message["provider_specific_fields"]["thought_signatures"],
        json!(["sig_reason", "sig_tool"])
    );
    assert_eq!(value["usage"]["total_tokens"], json!(3));
}

#[test]
fn gemini_response_to_chat_maps_audio_inline_data() {
    let input = json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "inlineData": {
                        "mimeType": "audio/wav",
                        "data": "UklGRg=="
                    }
                }]
            },
            "finishReason": "STOP"
        }]
    });

    let output = gemini_response_to_chat(&Bytes::from(serde_json::to_vec(&input).unwrap()), None)
        .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");

    let message = &value["choices"][0]["message"];
    assert_eq!(message["content"], Value::Null);
    assert_eq!(message["audio"]["data"], json!("UklGRg=="));
    assert_eq!(message["audio"]["transcript"], json!(""));
}
