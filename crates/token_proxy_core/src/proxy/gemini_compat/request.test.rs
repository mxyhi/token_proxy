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
