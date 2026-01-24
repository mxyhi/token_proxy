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
    assert_eq!(map_antigravity_model("  gemini-1.5-pro  "), "gemini-1.5-pro");
}

#[test]
fn returns_default_on_empty_model() {
    assert_eq!(map_antigravity_model(""), "gemini-1.5-flash");
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
        Some("ANY")
    );
}
