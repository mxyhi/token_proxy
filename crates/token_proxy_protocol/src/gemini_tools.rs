//! OpenAI Chat ↔ Gemini 工具定义转换

use serde_json::{json, Value};

mod schema;

use schema::clean_tool_schema;

#[cfg(test)]
#[path = "gemini_tools.test.rs"]
mod tests;

/// 将 OpenAI Chat 格式的 tools 转换为 Gemini 格式的 functionDeclarations
pub fn map_chat_tools_to_gemini(tools: &Value) -> Result<Value, String> {
    let Some(tools) = tools.as_array() else {
        return Ok(json!([]));
    };

    let mut declarations = Vec::new();
    for tool in tools {
        let Some(tool) = tool.as_object() else {
            continue;
        };
        if !matches!(
            tool.get("type").and_then(Value::as_str),
            Some("function" | "custom")
        ) {
            continue;
        }
        let function = tool
            .get("function")
            .and_then(Value::as_object)
            .unwrap_or(tool);
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let parameters = function
            .get("parameters")
            .or_else(|| function.get("format"))
            .map(clean_tool_schema)
            .unwrap_or_else(|| json!({"type":"object","properties":{}}));
        if schema::explicit_non_object_root(&parameters) {
            tracing::warn!(
                tool_name = name,
                "rejected non-object Gemini tool parameters"
            );
            return Err(format!(
                "Gemini tool '{name}' parameters must describe an object."
            ));
        }
        declarations.push(json!({
            "name": name,
            "description": description,
            "parametersJsonSchema": parameters
        }));
    }

    Ok(json!([{
        "functionDeclarations": declarations
    }]))
}

/// 将 OpenAI Chat 格式的 tool_choice 转换为 Gemini 格式的 toolConfig
pub fn map_chat_tool_choice_to_gemini(
    tool_choice: Option<&Value>,
    tools: Option<&Value>,
) -> Option<Value> {
    // 严格参数约束不等于强制调用；仅替换 auto/缺省，保留调用选择的优先级。
    let has_strict_tool = tools.and_then(Value::as_array).is_some_and(|tools| {
        tools.iter().any(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return false;
            }
            let function = tool.get("function").unwrap_or(tool);
            function.get("name").and_then(Value::as_str).is_some()
                && function
                    .get("strict")
                    .or_else(|| tool.get("strict"))
                    .and_then(Value::as_bool)
                    == Some(true)
        })
    });
    let auto_mode = if has_strict_tool { "VALIDATED" } else { "AUTO" };
    if has_strict_tool {
        tracing::debug!("detected strict tool declarations for Gemini calling mode selection");
    }
    match tool_choice {
        None | Some(Value::Null) if has_strict_tool => {
            Some(json!({ "functionCallingConfig": { "mode": auto_mode } }))
        }
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => match s.as_str() {
            "none" => Some(json!({ "functionCallingConfig": { "mode": "NONE" } })),
            "auto" => Some(json!({ "functionCallingConfig": { "mode": auto_mode } })),
            "required" => Some(json!({ "functionCallingConfig": { "mode": "ANY" } })),
            _ => None,
        },
        Some(Value::Object(obj)) => {
            // { "type": "function", "function": { "name": "..." } }
            if obj.get("type").and_then(Value::as_str) == Some("function") {
                let function = obj
                    .get("function")
                    .and_then(Value::as_object)
                    .unwrap_or(obj);
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    return Some(json!({
                        "functionCallingConfig": {
                            "mode": "ANY",
                            "allowedFunctionNames": [name]
                        }
                    }));
                }
            }
            None
        }
        _ => None,
    }
}

/// 将 Gemini 格式的 tools 转换为 OpenAI Chat 格式的 tools
pub fn map_gemini_tools_to_chat(value: &Value, tool_config: Option<&Value>) -> Value {
    let Some(groups) = value.as_array() else {
        return json!([]);
    };

    let calling_config = tool_config.and_then(|value| value.get("functionCallingConfig"));
    let validated = calling_config
        .and_then(|config| config.get("mode"))
        .and_then(Value::as_str)
        == Some("VALIDATED");
    let allowed = calling_config
        .and_then(|config| config.get("allowedFunctionNames"))
        .and_then(Value::as_array);
    let mut tools = Vec::new();
    for group in groups {
        let Some(group) = group.as_object() else {
            continue;
        };
        let Some(declarations) = group.get("functionDeclarations").and_then(Value::as_array) else {
            continue;
        };
        for declaration in declarations {
            let Some(declaration) = declaration.as_object() else {
                continue;
            };
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            // VALIDATED 允许普通回复；通过缩小声明集合保留 allowedFunctionNames 的限制。
            if validated
                && allowed.is_some_and(|names| {
                    !names.is_empty() && !names.iter().any(|n| n.as_str() == Some(name))
                })
            {
                continue;
            }
            let description = declaration
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let parameters = declaration
                .get("parameters")
                .or_else(|| declaration.get("parametersJsonSchema"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut tool = json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": parameters
                }
            });
            if validated {
                tool["function"]["strict"] = Value::Bool(true);
            }
            tools.push(tool);
        }
    }

    Value::Array(tools)
}

/// 将 Gemini 格式的 toolConfig 转换为 OpenAI Chat 格式的 tool_choice
pub fn map_gemini_tool_config_to_chat(value: &Value) -> Option<Value> {
    let tool_config = value.as_object()?;
    let config = tool_config
        .get("functionCallingConfig")
        .and_then(Value::as_object)?;

    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("");
    match mode {
        "NONE" => Some(Value::String("none".to_string())),
        "AUTO" | "VALIDATED" => Some(Value::String("auto".to_string())),
        "ANY" => {
            let allowed = config
                .get("allowedFunctionNames")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if allowed.len() == 1 {
                let name = allowed
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() {
                    return Some(json!({
                        "type": "function",
                        "function": { "name": name }
                    }));
                }
            }
            Some(Value::String("required".to_string()))
        }
        _ => None,
    }
}

/// 将 Gemini 格式的 functionCall 转换为 OpenAI Chat 格式的 tool_call
pub fn gemini_function_call_to_chat_tool_call(
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
