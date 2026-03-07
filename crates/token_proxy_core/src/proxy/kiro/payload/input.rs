use serde_json::{Map, Value};

pub(super) fn extract_input_messages(object: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let input = object.get("input");
    match input {
        Some(Value::String(text)) => {
            Ok(vec![serde_json::json!({ "role": "user", "content": text })])
        }
        Some(Value::Array(items)) => responses_input_to_chat_messages(items),
        Some(Value::Null) | None => Ok(Vec::new()),
        _ => Err("Responses input must be a string or array.".to_string()),
    }
}

fn responses_input_to_chat_messages(items: &[Value]) -> Result<Vec<Value>, String> {
    let mut messages = Vec::with_capacity(items.len());
    for item in items {
        messages.push(responses_input_item_to_chat_message(item)?);
    }
    Ok(messages)
}

fn responses_input_item_to_chat_message(item: &Value) -> Result<Value, String> {
    let Some(item) = item.as_object() else {
        return Err("Responses input item must be an object.".to_string());
    };

    if item.get("role").and_then(Value::as_str).is_some() {
        let mut output = item.clone();
        if let Some(content) = item
            .get("content")
            .and_then(responses_message_content_to_chat_content)
        {
            output.insert("content".to_string(), content);
        }
        return Ok(Value::Object(output));
    }

    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
        return Err("Responses input item must include role or type.".to_string());
    };

    match item_type {
        "message" => responses_message_item_to_chat_message(item),
        "function_call_output" => responses_function_call_output_item_to_chat_message(item),
        "function_call" => responses_function_call_item_to_chat_message(item),
        other => Err(format!("Unsupported Responses input item type: {other}")),
    }
}

fn responses_message_item_to_chat_message(item: &Map<String, Value>) -> Result<Value, String> {
    let role = item
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| "Responses message item must include role.".to_string())?;
    let content = item
        .get("content")
        .and_then(responses_message_content_to_chat_content)
        .unwrap_or_else(|| Value::String(String::new()));
    Ok(serde_json::json!({ "role": role, "content": content }))
}

fn responses_function_call_output_item_to_chat_message(
    item: &Map<String, Value>,
) -> Result<Value, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "function_call_output must include call_id.".to_string())?;
    let output = item.get("output").and_then(Value::as_str).unwrap_or("");
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("tool".to_string()));
    message.insert(
        "tool_call_id".to_string(),
        Value::String(call_id.to_string()),
    );
    message.insert("content".to_string(), Value::String(output.to_string()));
    if let Some(is_error) = item.get("is_error").and_then(Value::as_bool) {
        if is_error {
            message.insert("is_error".to_string(), Value::Bool(true));
        }
    }
    if let Some(parts) = item.get("output_parts") {
        message.insert("content_parts".to_string(), parts.clone());
    }
    Ok(Value::Object(message))
}

fn responses_function_call_item_to_chat_message(
    item: &Map<String, Value>,
) -> Result<Value, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "function_call must include call_id.".to_string())?;
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    Ok(serde_json::json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [
            {
                "id": call_id,
                "type": "function",
                "function": { "name": name, "arguments": arguments }
            }
        ]
    }))
}

fn responses_message_content_to_chat_content(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => Some(Value::String(text.to_string())),
        Value::Array(parts) => {
            let mut output_parts = Vec::new();
            let mut combined = String::new();
            let mut text_only = true;
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                let part_type = part.get("type").and_then(Value::as_str);
                match part_type {
                    Some("input_text") | Some("text") | Some("output_text") => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            combined.push_str(text);
                            output_parts.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                    Some("refusal") => {
                        let text = part
                            .get("refusal")
                            .or_else(|| part.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if !text.is_empty() {
                            combined.push_str(text);
                            output_parts.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                    Some("input_image") | Some("output_image") => {
                        if let Some(image_url) = part.get("image_url") {
                            text_only = false;
                            output_parts.push(serde_json::json!({
                                "type": "image_url",
                                "image_url": image_url
                            }));
                        }
                    }
                    _ => {
                        text_only = false;
                    }
                }
            }
            if text_only {
                Some(Value::String(combined))
            } else {
                Some(Value::Array(output_parts))
            }
        }
        Value::Null => None,
        _ => Some(Value::String(String::new())),
    }
}
