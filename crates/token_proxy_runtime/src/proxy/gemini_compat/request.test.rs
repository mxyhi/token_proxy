use super::*;
use serde_json::json;

#[test]
fn chat_to_gemini_rejects_explicit_non_object_tool_parameters() {
    for parameters in [
        json!({"type":"string","properties":{"x":{"type":"string"}}}),
        json!({"type":"array","items":{"type":"string"}}),
        json!({"type":["string","null"]}),
        json!({"type":"null"}),
        json!(false),
    ] {
        let input = json!({"messages":[{"role":"user","content":"hello"}],"tools":[{
            "type":"function","function":{"name":"bad_schema","parameters":parameters}
        }]});
        let error = chat_request_to_gemini(&Bytes::from(input.to_string()))
            .expect_err("non-object parameters must not be sent upstream");
        assert!(error.contains("bad_schema"), "{error}");
        assert!(error.contains("object"), "{error}");
    }
}

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
    assert!(value["messages"][0]["tool_call_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("call_gemini_")));
}

#[test]
fn gemini_request_to_chat_filters_hidden_thought_parts_everywhere() {
    let input = json!({
        "systemInstruction": {
            "parts": [
                { "text": "hidden system", "thought": true },
                { "text": "visible system" }
            ]
        },
        "contents": [{
            "role": "user",
            "parts": [
                { "text": "hidden user", "thought": true },
                { "functionResponse": { "name": "secret", "response": "hidden" }, "thought": true },
                { "text": "visible user" }
            ]
        }]
    });

    let output = gemini_request_to_chat(&Bytes::from(input.to_string()), None).expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["messages"].as_array().expect("messages").len(), 2);
    assert_eq!(value["messages"][0]["content"], "visible system");
    assert_eq!(value["messages"][1]["content"], "visible user");
}

#[test]
fn gemini_request_to_chat_pairs_same_name_calls_across_contents_fifo() {
    let input = json!({
        "contents": [
            {
                "role": "model",
                "parts": [
                    { "functionCall": { "id": "call-a", "name": "lookup", "args": { "q": "a" } } },
                    { "functionCall": { "call_id": "call-b", "name": "lookup", "args": { "q": "b" } } }
                ]
            },
            {
                "role": "model",
                "parts": [
                    { "functionCall": { "callId": "call-c", "name": "lookup", "args": { "q": "c" } } }
                ]
            },
            {
                "role": "user",
                "parts": [
                    { "functionResponse": { "name": "lookup", "response": { "result": "a" } } },
                    { "functionResponse": { "name": "lookup", "response": { "result": "b" } } },
                    { "functionResponse": { "name": "lookup", "response": { "result": "c" } } }
                ]
            }
        ]
    });

    let output = gemini_request_to_chat(&Bytes::from(input.to_string()), None).expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let calls = value["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message.get("tool_calls"))
        .flat_map(|calls| calls.as_array().expect("tool calls"))
        .map(|call| call["id"].as_str().expect("call id"))
        .collect::<Vec<_>>();
    let results = value["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().expect("result id"))
        .collect::<Vec<_>>();

    assert_eq!(calls, ["call-a", "call-b", "call-c"]);
    assert_eq!(results, calls);
}

#[test]
fn gemini_request_to_chat_generates_stable_ids_for_missing_and_orphan_calls() {
    let input = json!({
        "contents": [
            { "role": "model", "parts": [{ "functionCall": { "name": "lookup", "args": { "q": "x" } } }] },
            { "role": "user", "parts": [
                { "functionResponse": { "name": "lookup", "response": { "result": "x" } } },
                { "functionResponse": { "name": "orphan", "response": { "result": "y" } } }
            ] }
        ]
    });

    let first = gemini_request_to_chat(&Bytes::from(input.to_string()), None).expect("convert");
    let second = gemini_request_to_chat(&Bytes::from(input.to_string()), None).expect("repeat");
    let first: Value = serde_json::from_slice(&first).expect("json");
    let second: Value = serde_json::from_slice(&second).expect("json");

    assert_eq!(first["messages"], second["messages"]);
    assert_eq!(
        first["messages"][0]["tool_calls"][0]["id"],
        first["messages"][1]["tool_call_id"]
    );
    assert_ne!(
        first["messages"][1]["tool_call_id"],
        first["messages"][2]["tool_call_id"]
    );
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
fn chat_request_to_gemini_cleans_unsupported_tool_schema_fields() {
    let input = json!({
        "messages": [
            { "role": "user", "content": "find file" }
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read a file",
                    "parameters": {
                        "type": "object",
                        "$defs": { "unused": { "type": "string" } },
                        "definitions": { "legacy": { "type": "number" } },
                        "additionalProperties": false,
                        "properties": {
                            "path": { "type": ["string", "null"], "minLength": 1 },
                            "count": { "type": ["null", "integer"] },
                            "empty": { "type": ["null"] }
                        }
                    }
                }
            }
        ]
    });

    let output =
        chat_request_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap())).expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let parameters = &value["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"];

    assert_eq!(parameters["type"], json!("object"));
    assert!(parameters.get("$defs").is_none());
    assert!(parameters.get("definitions").is_none());
    assert_eq!(parameters["additionalProperties"], false);
    assert_eq!(
        parameters["properties"]["path"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(parameters["properties"]["path"]["minLength"], 1);
    assert_eq!(
        parameters["properties"]["count"]["type"],
        json!(["null", "integer"])
    );
    assert_eq!(parameters["properties"]["empty"]["type"], json!(["null"]));
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
fn chat_request_to_gemini_maps_video_parts_to_gemini_media() {
    let input = json!({
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "video_url", "video_url": { "url": "https://example.com/demo.mp4", "processing": "agentic" } },
                    { "type": "input_video", "video_url": "data:video/webm;base64,AAECAwQ=" }
                ]
            }
        ]
    });

    let output =
        chat_request_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap())).expect("convert");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let parts = value["contents"][0]["parts"].as_array().expect("parts");

    assert_eq!(
        parts[0]["fileData"]["fileUri"],
        json!("https://example.com/demo.mp4")
    );
    assert_eq!(parts[0]["fileData"]["mimeType"], json!("video/mp4"));
    assert_eq!(parts[1]["inlineData"]["mimeType"], json!("video/webm"));
    assert_eq!(parts[1]["inlineData"]["data"], json!("AAECAwQ="));
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

// 只封装字符串 $ref，普通 JSON 和实例数据必须保持原样。
#[test]
fn chat_request_to_gemini_wraps_tool_results_with_json_references() {
    let cases = [
        (json!({"$ref":"#/Schema"}), true),
        (json!({"nested":[{"$ref":"#/Schema"}]}), true),
        (json!([{"$ref":"#/Schema"}]), true),
        (json!({"$ref":12,"nested":{"ok":true}}), false),
        (json!({"text":"contains $ref as ordinary text"}), false),
    ];
    for (result, wrap) in cases {
        for content in [result.clone(), json!(result.to_string())] {
            let input = json!({"messages":[
                {"role":"assistant", "tool_calls":[{"id":"call_1", "type":"function",
                    "function":{"name":"schema", "arguments":"{}"}}]},
                {"role":"tool", "tool_call_id":"call_1", "content":content}
            ]});
            let output = chat_request_to_gemini(&Bytes::from(input.to_string())).expect("convert");
            let output: Value = serde_json::from_slice(&output).expect("json");
            let response = &output["contents"][1]["parts"][0]["functionResponse"]["response"];
            if wrap {
                assert_eq!(
                    serde_json::from_str::<Value>(
                        response["result"].as_str().expect("opaque JSON")
                    )
                    .unwrap(),
                    result
                );
            } else {
                assert_eq!(response, &result);
            }
        }
    }
}

// 验证运行时入口实际使用严格模式配置，避免仅协议辅助函数正确。
#[test]
fn chat_request_to_gemini_preserves_strict_tool_contract() {
    for (choice, mode) in [
        (None, "VALIDATED"),
        (Some("auto"), "VALIDATED"),
        (Some("none"), "NONE"),
        (Some("required"), "ANY"),
    ] {
        let mut input = json!({"messages":[{"role":"user","content":"look up"}],"tools":[
            {"type":"function","function":{"name":"lookup","strict":true,"parameters":{
                "type":"object","additionalProperties":false,"properties":{"q":{"type":"string","minLength":2}},"required":["q"]
            }}}
        ]});
        if let Some(choice) = choice {
            input["tool_choice"] = json!(choice);
        }
        let output = chat_request_to_gemini(&Bytes::from(input.to_string())).expect("convert");
        let value: Value = serde_json::from_slice(&output).expect("json");
        assert_eq!(value["toolConfig"]["functionCallingConfig"]["mode"], mode);
        let declaration = &value["tools"][0]["functionDeclarations"][0];
        assert!(declaration.get("parameters").is_none());
        assert_eq!(
            declaration["parametersJsonSchema"],
            input["tools"][0]["function"]["parameters"]
        );
    }
}
