use serde_json::{json, Map, Value};

use super::message::extract_text_from_part;

pub(super) fn responses_input_to_chat_messages(items: &[Value]) -> Result<Vec<Value>, String> {
    let mut messages = Vec::with_capacity(items.len());
    for item in items {
        messages.extend(responses_input_item_to_chat_messages(item)?);
    }
    Ok(messages)
}

fn responses_input_item_to_chat_messages(item: &Value) -> Result<Vec<Value>, String> {
    let Some(item) = item.as_object() else {
        return Err("Responses input item must be an object.".to_string());
    };

    // Cherry Studio / Codex CLI 可能会直接传 `[{ role, content:[{type,text}...] }]`，
    // 这里需要把 content parts 归一化成 Chat API 需要的字符串/多模态数组。
    if item.get("role").and_then(Value::as_str).is_some() {
        let mut output = item.clone();
        if let Some(content) = item
            .get("content")
            .and_then(responses_message_content_to_chat_content)
        {
            output.insert("content".to_string(), content);
        }
        return Ok(vec![Value::Object(output)]);
    }

    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
        return Err("Responses input item must include role or type.".to_string());
    };

    match item_type {
        "message" => responses_message_item_to_chat_message(item).map(|message| vec![message]),
        "function_call_output" | "web_search_call" | "computer_call_output" | "tool_result" => {
            Ok(responses_tool_output_item_to_chat_message(item)
                .into_iter()
                .collect())
        }
        "function_call" => {
            responses_function_call_item_to_chat_message(item).map(|message| vec![message])
        }
        _ => Err(format!(
            "Unsupported Responses input item type: {item_type}"
        )),
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
    Ok(json!({ "role": role, "content": content }))
}

fn responses_tool_output_item_to_chat_message(item: &Map<String, Value>) -> Option<Value> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    if call_id.is_empty() {
        return None;
    }

    Some(json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": normalize_tool_output_to_chat_content(item.get("output"))
    }))
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
    Ok(json!({
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

fn normalize_tool_output_to_chat_content(output: Option<&Value>) -> Value {
    match output {
        None | Some(Value::Null) => Value::String(String::new()),
        Some(Value::String(text)) => Value::String(text.to_string()),
        // Tools output often arrives as Responses content parts. Keep multimodal
        // blocks when they are meaningful, otherwise stringify the original JSON
        // instead of silently dropping structured payloads.
        Some(Value::Array(parts)) => {
            match responses_message_content_to_chat_content(&Value::Array(parts.clone())) {
                Some(Value::Array(content)) => Value::Array(content),
                Some(Value::String(text)) if !text.is_empty() => Value::String(text),
                _ => Value::String(Value::Array(parts.clone()).to_string()),
            }
        }
        Some(value) => Value::String(value.to_string()),
    }
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
                        if let Some(text) = extract_text_from_part(part) {
                            combined.push_str(&text);
                            output_parts.push(json!({ "type": "text", "text": text }));
                        }
                    }
                    Some("refusal") => {
                        // Responses may represent refusals as a dedicated content part.
                        let text = part
                            .get("refusal")
                            .or_else(|| part.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if text.is_empty() {
                            continue;
                        }
                        combined.push_str(text);
                        output_parts.push(json!({ "type": "text", "text": text }));
                    }
                    Some("input_image") => {
                        // Chat Completions expects `{type:"image_url", image_url:{url:"..."}}`.
                        let url = match part.get("image_url") {
                            Some(Value::String(url)) => Some(json!({ "url": url })),
                            Some(Value::Object(object)) => object
                                .get("url")
                                .and_then(Value::as_str)
                                .map(|url| json!({ "url": url })),
                            _ => None,
                        };
                        let Some(image_url) = url else {
                            continue;
                        };
                        text_only = false;
                        output_parts.push(json!({ "type": "image_url", "image_url": image_url }));
                    }
                    Some("input_file") => {
                        text_only = false;
                        if let Some(file_url) = part.get("file_url") {
                            output_parts.push(
                                json!({ "type": "input_file", "file_url": file_url.clone() }),
                            );
                        }
                    }
                    Some("input_audio") => {
                        text_only = false;
                        if let Some(audio) = part.get("input_audio") {
                            output_parts.push(
                                json!({ "type": "input_audio", "input_audio": audio.clone() }),
                            );
                        }
                    }
                    _ => continue,
                }
            }
            if text_only {
                Some(Value::String(combined))
            } else {
                Some(Value::Array(output_parts))
            }
        }
        _ => Some(Value::String(String::new())),
    }
}
