use super::*;
use serde_json::json;

#[test]
fn gemini_request_to_chat_maps_system_tools_and_format() {
    let input = json!({
        "systemInstruction": { "parts": [{ "text": "sys" }] },
        "contents": [
            { "role": "user", "parts": [{ "text": "hi" }] }
        ],
        "generationConfig": {
            "temperature": 0.2,
            "topP": 0.8,
            "maxOutputTokens": 12,
            "responseMimeType": "application/json"
        },
        "tools": [{
            "functionDeclarations": [
                { "name": "getFoo", "description": "x", "parameters": { "type": "object" } }
            ]
        }],
        "toolConfig": { "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["getFoo"] } }
    });

    let output = gemini_request_to_chat(
        &Bytes::from(serde_json::to_vec(&input).unwrap()),
        Some("gemini-1.5-flash"),
    )
    .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(value["model"], json!("gemini-1.5-flash"));
    assert_eq!(value["messages"][0]["role"], json!("system"));
    assert_eq!(value["messages"][1]["role"], json!("user"));
    assert_eq!(value["messages"][1]["content"], json!("hi"));
    assert_eq!(value["tools"][0]["function"]["name"], json!("getFoo"));
    assert_eq!(value["tool_choice"]["function"]["name"], json!("getFoo"));
    assert_eq!(value["response_format"]["type"], json!("json_object"));
    assert_eq!(value["max_completion_tokens"], json!(12));
}

#[test]
fn gemini_request_to_chat_maps_function_response() {
    let input = json!({
        "contents": [
            {
                "role": "user",
                "parts": [
                    { "functionResponse": { "name": "getFoo", "response": { "ok": true } } }
                ]
            }
        ]
    });
    let output = gemini_request_to_chat(&Bytes::from(serde_json::to_vec(&input).unwrap()), None)
        .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(value["messages"][0]["role"], json!("tool"));
    assert_eq!(value["messages"][0]["name"], json!("getFoo"));
    assert_eq!(value["messages"][0]["tool_call_id"], json!("call_getFoo"));
}

#[test]
fn gemini_request_to_chat_maps_parameters_json_schema() {
    let input = json!({
        "contents": [
            { "role": "user", "parts": [{ "text": "hi" }] }
        ],
        "tools": [{
            "functionDeclarations": [
                {
                    "name": "getFoo",
                    "description": "x",
                    "parametersJsonSchema": {
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    }
                }
            ]
        }]
    });

    let output = gemini_request_to_chat(
        &Bytes::from(serde_json::to_vec(&input).unwrap()),
        Some("gemini-1.5-flash"),
    )
    .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(
        value["tools"][0]["function"]["parameters"]["properties"]["query"]["type"],
        json!("string")
    );
}

#[test]
fn chat_request_to_gemini_maps_tool_result_name_from_prior_tool_call() {
    let input = json!({
        "messages": [
            { "role": "user", "content": "hi" },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "getFoo",
                            "arguments": "{\"query\":\"x\"}"
                        }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "{\"ok\":true}"
            }
        ]
    });

    let output =
        chat_request_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap())).expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(
        value["contents"][2]["parts"][0]["functionResponse"]["name"],
        json!("getFoo")
    );
    assert_eq!(
        value["contents"][2]["parts"][0]["functionResponse"]["response"]["ok"],
        json!(true)
    );
}

#[test]
fn chat_request_to_gemini_preserves_remote_images_and_input_audio() {
    let input = json!({
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": "look" },
                    {
                        "type": "image_url",
                        "image_url": { "url": "https://example.com/cat.png", "format": "image/png" }
                    },
                    {
                        "type": "input_audio",
                        "input_audio": { "data": "UklGRg==", "format": "wav" }
                    }
                ]
            }
        ]
    });

    let output =
        chat_request_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap())).expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let parts = value["contents"][0]["parts"].as_array().expect("parts");

    assert_eq!(parts[0]["text"], json!("look"));
    assert_eq!(
        parts[1]["fileData"]["fileUri"],
        json!("https://example.com/cat.png")
    );
    assert_eq!(parts[1]["fileData"]["mimeType"], json!("image/png"));
    assert_eq!(parts[2]["inlineData"]["mimeType"], json!("audio/wav"));
    assert_eq!(parts[2]["inlineData"]["data"], json!("UklGRg=="));
}

#[test]
fn chat_request_to_gemini_rejects_unsupported_content_parts_instead_of_dropping_them() {
    let input = json!({
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "video_url", "video_url": { "url": "https://example.com/demo.mp4" } }
                ]
            }
        ]
    });

    let error =
        chat_request_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap())).unwrap_err();
    assert!(error.contains("Unsupported Chat content part type for Gemini"));
}

#[test]
fn gemini_request_to_chat_preserves_audio_and_file_parts() {
    let input = json!({
        "contents": [
            {
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
            }
        ]
    });

    let output = gemini_request_to_chat(&Bytes::from(serde_json::to_vec(&input).unwrap()), None)
        .expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let content = value["messages"][0]["content"].as_array().expect("content");

    assert_eq!(content[0]["type"], json!("input_audio"));
    assert_eq!(content[0]["input_audio"]["data"], json!("UklGRg=="));
    assert_eq!(content[1]["type"], json!("input_file"));
    assert_eq!(
        content[1]["file_url"],
        json!("https://example.com/spec.pdf")
    );
}
