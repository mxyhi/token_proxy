use serde_json::{json, Value};

pub(super) fn extract_text_from_chat_content(content: Option<&Value>) -> Option<String> {
    let Some(content) = content else {
        return None;
    };
    match content {
        Value::String(text) => Some(text.to_string()),
        Value::Array(parts) => {
            let mut combined = String::new();
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                if !matches!(part_type, "text" | "input_text") {
                    continue;
                }
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    combined.push_str(text);
                }
            }
            if combined.trim().is_empty() {
                None
            } else {
                Some(combined)
            }
        }
        Value::Object(object) => object.get("text").and_then(Value::as_str).map(|t| t.to_string()),
        _ => None,
    }
}

pub(super) fn chat_content_to_responses_message_parts(
    content: Option<&Value>,
    text_part_type: &str,
) -> Result<Vec<Value>, String> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    match content {
        Value::String(text) => Ok(vec![json!({ "type": text_part_type, "text": text })]),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                match part_type {
                    "text" | "input_text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push(json!({ "type": text_part_type, "text": text }));
                        }
                    }
                    "image_url" => {
                        let url = match part.get("image_url") {
                            Some(Value::String(url)) => Some(json!({ "url": url })),
                            Some(Value::Object(object)) => object
                                .get("url")
                                .and_then(Value::as_str)
                                .map(|url| json!({ "url": url })),
                            _ => None,
                        };
                        if let Some(image_url) = url {
                            out.push(json!({ "type": "input_image", "image_url": image_url }));
                        }
                    }
                    "input_image" => {
                        if let Some(image_url) = part.get("image_url") {
                            out.push(json!({ "type": "input_image", "image_url": image_url.clone() }));
                        }
                    }
                    _ => {}
                }
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

pub(super) fn chat_tool_calls_to_responses_items(value: Option<&Value>) -> Vec<Value> {
    let Some(tool_calls) = value.and_then(Value::as_array) else {
        return Vec::new();
    };

    tool_calls
        .iter()
        .enumerate()
        .filter_map(|(idx, call)| chat_tool_call_to_responses_item(call, idx))
        .collect()
}

fn chat_tool_call_to_responses_item(value: &Value, idx: usize) -> Option<Value> {
    let call = value.as_object()?;
    let call_id = call
        .get("id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| format!("call_proxy_{idx}"));
    let function = call.get("function").and_then(Value::as_object)?;
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = stringify_any_json(function.get("arguments"));

    Some(json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments
    }))
}

pub(super) fn chat_function_call_to_responses_item(value: Option<&Value>) -> Option<Value> {
    let Some(value) = value else {
        return None;
    };
    let Some(function) = value.as_object() else {
        return None;
    };
    let name = function.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return None;
    }
    let arguments = stringify_any_json(function.get("arguments"));
    Some(json!({
        "type": "function_call",
        "call_id": "call_legacy",
        "name": name,
        "arguments": arguments
    }))
}

pub(super) fn stringify_any_json(value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(Value::String(text)) => text.to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub(super) fn user_placeholder_item() -> Value {
    json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": "..." }]
    })
}

pub(super) fn join_non_empty_lines(texts: Vec<String>) -> Option<String> {
    let combined = texts
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if combined.is_empty() {
        None
    } else {
        Some(combined)
    }
}
