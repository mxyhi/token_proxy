//! OpenAI Chat ↔ Gemini 工具定义转换

use serde_json::{json, Value};

/// 将 OpenAI Chat 格式的 tools 转换为 Gemini 格式的 functionDeclarations
pub(super) fn map_chat_tools_to_gemini(tools: &Value) -> Value {
    let Some(tools) = tools.as_array() else {
        return json!([]);
    };

    let declarations: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            let tool = tool.as_object()?;
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return None;
            }
            let function = tool.get("function")?.as_object()?;
            let name = function.get("name").and_then(Value::as_str)?;
            let description = function.get("description").and_then(Value::as_str).unwrap_or("");
            let parameters = function.get("parameters").cloned().unwrap_or_else(|| json!({}));
            Some(json!({
                "name": name,
                "description": description,
                "parameters": parameters
            }))
        })
        .collect();

    json!([{
        "functionDeclarations": declarations
    }])
}

/// 将 OpenAI Chat 格式的 tool_choice 转换为 Gemini 格式的 toolConfig
pub(super) fn map_chat_tool_choice_to_gemini(tool_choice: &Value) -> Option<Value> {
    match tool_choice {
        Value::String(s) => match s.as_str() {
            "none" => Some(json!({ "functionCallingConfig": { "mode": "NONE" } })),
            "auto" => Some(json!({ "functionCallingConfig": { "mode": "AUTO" } })),
            "required" => Some(json!({ "functionCallingConfig": { "mode": "ANY" } })),
            _ => None,
        },
        Value::Object(obj) => {
            // { "type": "function", "function": { "name": "..." } }
            if obj.get("type").and_then(Value::as_str) == Some("function") {
                if let Some(function) = obj.get("function").and_then(Value::as_object) {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        return Some(json!({
                            "functionCallingConfig": {
                                "mode": "ANY",
                                "allowedFunctionNames": [name]
                            }
                        }));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// 将 Gemini 格式的 functionCall 转换为 OpenAI Chat 格式的 tool_call
pub(super) fn gemini_function_call_to_chat_tool_call(
    function_call: &serde_json::Map<String, Value>,
    index: usize,
) -> Value {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let args = function_call
        .get("args")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arguments = match args {
        Value::String(s) => s,
        other => serde_json::to_string(&other).unwrap_or_else(|_| "{}".to_string()),
    };

    json!({
        "id": format!("call_gemini_{index}"),
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    })
}
