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
    assert_eq!(value["candidates"][0]["content"]["parts"][0]["text"], json!("hello"));
    assert_eq!(
        value["candidates"][0]["content"]["parts"][1]["functionCall"]["name"],
        json!("getFoo")
    );
    assert_eq!(value["usageMetadata"]["totalTokenCount"], json!(3));
}
