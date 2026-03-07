use super::map_antigravity_model;
use super::wrap_gemini_request;
use axum::body::Bytes;
use serde_json::json;

#[test]
fn keeps_claude_model_unchanged() {
    assert_eq!(
        map_antigravity_model("claude-3-5-sonnet-20241022"),
        "claude-3-5-sonnet-20241022"
    );
}

#[test]
fn trims_model_name() {
    assert_eq!(
        map_antigravity_model("  gemini-1.5-pro  "),
        "gemini-1.5-pro"
    );
}

#[test]
fn returns_default_on_empty_model() {
    assert_eq!(map_antigravity_model(""), "gemini-1.5-flash");
}

#[test]
fn strips_gemini_prefix_for_claude_aliases() {
    assert_eq!(
        map_antigravity_model("gemini-claude-opus-4-5-thinking"),
        "claude-opus-4-5-thinking"
    );
}

#[test]
fn keeps_claude_opus_date_model_unchanged() {
    assert_eq!(
        map_antigravity_model("claude-opus-4-5-20251101"),
        "claude-opus-4-5-20251101"
    );
    assert_eq!(
        map_antigravity_model("claude-opus-4-5-20251101-thinking"),
        "claude-opus-4-5-20251101-thinking"
    );
}

#[test]
fn keeps_claude_sonnet_date_model_unchanged() {
    assert_eq!(
        map_antigravity_model("claude-sonnet-4-5-20250929"),
        "claude-sonnet-4-5-20250929"
    );
}

#[test]
fn keeps_claude_haiku_date_model_unchanged() {
    assert_eq!(
        map_antigravity_model("claude-haiku-4-5-20251001"),
        "claude-haiku-4-5-20251001"
    );
}

#[test]
fn maps_legacy_antigravity_aliases_to_canonical_ids() {
    assert_eq!(
        map_antigravity_model("gemini-2.5-computer-use-preview-10-2025"),
        "rev19-uic3-1p"
    );
    assert_eq!(
        map_antigravity_model("gemini-3-pro-image-preview"),
        "gemini-3-pro-image"
    );
    assert_eq!(
        map_antigravity_model("gemini-3-pro-preview"),
        "gemini-3-pro-high"
    );
    assert_eq!(
        map_antigravity_model("gemini-3-flash-preview"),
        "gemini-3-flash"
    );
}

#[test]
fn model_hint_overrides_body_model_in_wrap_request() {
    let request = json!({
        "model": "claude-haiku-4-5-20251001",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped =
        wrap_gemini_request(&bytes, Some("gemini-2.5-flash"), None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    assert_eq!(value["model"].as_str(), Some("gemini-2.5-flash"));
    assert!(value["request"].get("systemInstruction").is_none());
}

#[test]
fn injects_antigravity_system_instruction_for_claude() {
    let request = json!({
        "model": "claude-3-5-sonnet-20241022",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let system = value["request"]["systemInstruction"].clone();
    assert_eq!(system["role"].as_str(), Some("user"));
    let parts = system["parts"].as_array().expect("parts array");
    assert!(parts.len() >= 2);
}

#[test]
fn cleans_schema_unsupported_fields() {
    let request = json!({
        "model": "claude-3-5-sonnet-20241022",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ],
        "tools": [
            {
                "function_declarations": [
                    {
                        "name": "t",
                        "parametersJsonSchema": {
                            "type": "object",
                            "properties": {
                                "count": {
                                    "type": "number",
                                    "exclusiveMinimum": 0
                                },
                                "name": {
                                    "type": "string",
                                    "propertyNames": { "pattern": "^[a-z]+$" }
                                }
                            }
                        }
                    }
                ]
            }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let schema = &value["request"]["tools"][0]["function_declarations"][0]["parameters"];
    let count = schema.get("properties").and_then(|v| v.get("count"));
    let name = schema.get("properties").and_then(|v| v.get("name"));
    assert!(count.and_then(|v| v.get("exclusiveMinimum")).is_none());
    assert!(name.and_then(|v| v.get("propertyNames")).is_none());
}

#[test]
fn cleans_schema_for_gemini_3_pro_high() {
    let request = json!({
        "model": "gemini-3-pro-high",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ],
        "tools": [
            {
                "function_declarations": [
                    {
                        "name": "t",
                        "parametersJsonSchema": {
                            "type": "object",
                            "properties": {
                                "count": {
                                    "type": "number",
                                    "exclusiveMinimum": 0
                                }
                            }
                        }
                    }
                ]
            }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let schema = &value["request"]["tools"][0]["function_declarations"][0]["parameters"];
    let count = schema.get("properties").and_then(|v| v.get("count"));
    assert!(count.and_then(|v| v.get("exclusiveMinimum")).is_none());
}

#[test]
fn keeps_existing_tool_config_mode() {
    let request = json!({
        "model": "claude-3-5-sonnet-20241022",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ],
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "ANY"
            }
        }
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    assert_eq!(
        value["request"]["toolConfig"]["functionCallingConfig"]["mode"].as_str(),
        Some("VALIDATED")
    );
}

#[test]
fn gemini_schema_cleaner_renames_and_does_not_add_placeholders() {
    let request = json!({
        "model": "gemini-2.5-pro",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ],
        "tools": [
            {
                "function_declarations": [
                    {
                        "name": "t",
                        "parametersJsonSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                ]
            }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let schema = &value["request"]["tools"][0]["function_declarations"][0]["parameters"];
    assert!(schema
        .get("properties")
        .and_then(|v| v.get("reason"))
        .is_none());
    assert!(schema.get("required").is_none());
}

#[test]
fn session_id_fallback_is_dash_decimal() {
    let request = json!({
        "model": "gemini-2.5-pro",
        "contents": []
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let session = value["request"]["sessionId"].as_str().expect("sessionId");
    assert!(session.starts_with('-'));
    assert!(session[1..].chars().all(|ch| ch.is_ascii_digit()));
}

#[test]
fn request_id_and_project_match_reference_shapes() {
    let request = json!({
        "model": "gemini-2.5-pro",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");

    let request_id = value["requestId"].as_str().expect("requestId");
    assert!(request_id.starts_with("agent-"));
    assert!(is_uuid_like(&request_id["agent-".len()..]));

    let project = value["project"].as_str().expect("project");
    let parts: Vec<&str> = project.split('-').collect();
    assert_eq!(parts.len(), 3);
    assert!(matches!(
        parts[0],
        "useful" | "bright" | "swift" | "calm" | "bold"
    ));
    assert!(matches!(
        parts[1],
        "fuze" | "wave" | "spark" | "flow" | "core"
    ));
    assert_eq!(parts[2].len(), 5);
    assert!(parts[2].chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn overwrites_session_id_even_when_provided() {
    let request = json!({
        "model": "gemini-2.5-pro",
        "sessionId": "not-stable",
        "contents": [
            { "role": "user", "parts": [{ "text": "hello" }] }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let session = value["request"]["sessionId"].as_str().expect("sessionId");
    assert_ne!(session, "not-stable");
    assert!(session.starts_with('-'));
    assert!(session[1..].chars().all(|ch| ch.is_ascii_digit()));
}

#[test]
fn merges_function_responses_for_parallel_calls_and_does_not_set_response_signatures() {
    let request = json!({
        "model": "gemini-2.5-pro",
        "contents": [
            {
                "role": "model",
                "parts": [
                    { "functionCall": { "name": "tool_one", "args": { "a": "1" } } },
                    { "functionCall": { "name": "tool_two", "args": { "b": "2" } } }
                ]
            },
            {
                "role": "user",
                "parts": [
                    { "functionResponse": { "name": "tool_one", "response": { "result": "ok1" } } }
                ]
            },
            {
                "role": "user",
                "parts": [
                    { "functionResponse": { "name": "tool_two", "response": { "result": "ok2" } } }
                ]
            }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let contents = value["request"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0]["role"].as_str(), Some("model"));
    assert_eq!(contents[1]["role"].as_str(), Some("user"));

    let merged_parts = contents[1]["parts"].as_array().expect("parts array");
    assert_eq!(merged_parts.len(), 2);
    for part in merged_parts {
        assert!(part.get("functionResponse").is_some());
        assert!(part.get("thoughtSignature").is_none());
    }
}

#[test]
fn normalizes_invalid_roles_in_contents() {
    let request = json!({
        "model": "gemini-2.5-pro",
        "contents": [
            { "role": "assistant", "parts": [{ "text": "a" }] },
            { "role": "assistant", "parts": [{ "text": "b" }] }
        ]
    });
    let bytes = Bytes::from(request.to_string());
    let wrapped = wrap_gemini_request(&bytes, None, None, "ua").expect("wrap ok");
    let value: serde_json::Value = serde_json::from_slice(&wrapped).expect("wrapped json");
    let contents = value["request"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents[0]["role"].as_str(), Some("user"));
    assert_eq!(contents[1]["role"].as_str(), Some("model"));
}

fn is_uuid_like(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    let bytes: Vec<char> = value.chars().collect();
    for &idx in &[8_usize, 13, 18, 23] {
        if bytes.get(idx) != Some(&'-') {
            return false;
        }
    }
    value.chars().enumerate().all(|(idx, ch)| match idx {
        8 | 13 | 18 | 23 => ch == '-',
        _ => ch.is_ascii_hexdigit(),
    })
}
