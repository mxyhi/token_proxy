use serde_json::{json, Map, Value};

use crate::proxy::codex_tool_types::is_codex_tool_call_output_item_type;

use super::message::extract_text_from_part;

pub(super) fn responses_input_to_chat_messages(items: &[Value]) -> Result<Vec<Value>, String> {
    let mut messages = Vec::with_capacity(items.len());
    let mut pending_reasoning = String::new();
    let mut pending_gemini_signature = None;
    for item in items {
        append_responses_input_item_to_chat_messages(
            item,
            &mut messages,
            &mut pending_reasoning,
            &mut pending_gemini_signature,
        )?;
    }
    Ok(messages)
}

fn append_responses_input_item_to_chat_messages(
    item: &Value,
    messages: &mut Vec<Value>,
    pending_reasoning: &mut String,
    pending_gemini_signature: &mut Option<String>,
) -> Result<(), String> {
    if let Some(text) = item.as_str() {
        messages.push(json!({ "role": "user", "content": text }));
        pending_reasoning.clear();
        *pending_gemini_signature = None;
        return Ok(());
    }

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
        if output.get("role").and_then(Value::as_str) == Some("assistant") {
            attach_pending_reasoning(&mut output, pending_reasoning);
        } else {
            pending_reasoning.clear();
            *pending_gemini_signature = None;
        }
        messages.push(Value::Object(output));
        return Ok(());
    }

    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
        return Err("Responses input item must include role or type.".to_string());
    };

    match item_type {
        "message" => {
            let mut message = responses_message_item_to_chat_message(item)?;
            if message.get("role").and_then(Value::as_str) == Some("assistant") {
                if let Some(message) = message.as_object_mut() {
                    attach_pending_reasoning(message, pending_reasoning);
                }
            } else {
                pending_reasoning.clear();
            }
            messages.push(message);
        }
        "reasoning" => {
            let reasoning = extract_reasoning_text(item);
            if !reasoning.trim().is_empty() {
                pending_reasoning.push_str(&reasoning);
            }
            if let Some(signature) = item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .and_then(crate::proxy::gemini_compat::decode_function_signature_carrier)
            {
                *pending_gemini_signature = Some(signature);
            }
        }
        item_type if is_codex_tool_call_output_item_type(item_type) => {
            if let Some(message) = responses_tool_output_item_to_chat_message(item) {
                messages.push(message);
            }
            pending_reasoning.clear();
            *pending_gemini_signature = None;
        }
        "web_search_call" | "computer_call_output" | "tool_result" => {
            if let Some(message) = responses_tool_output_item_to_chat_message(item) {
                messages.push(message);
            }
            pending_reasoning.clear();
        }
        "function_call" | "custom_tool_call" => {
            append_responses_function_call(
                messages,
                item,
                pending_reasoning,
                pending_gemini_signature,
            )?;
        }
        // Built-in tool call records have no Chat Completions message equivalent.
        // Skipping them preserves assistant tool_calls -> tool output adjacency.
        _ => pending_reasoning.clear(),
    }
    Ok(())
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

fn append_responses_function_call(
    messages: &mut Vec<Value>,
    item: &Map<String, Value>,
    pending_reasoning: &mut String,
    pending_gemini_signature: &mut Option<String>,
) -> Result<(), String> {
    let mut tool_call = responses_tool_call_item_to_chat_tool_call(item)?;
    if let Some(signature) = item
        .get("provider_specific_fields")
        .and_then(Value::as_object)
        .filter(|fields| fields.get("provider").and_then(Value::as_str) == Some("gemini"))
        .and_then(|fields| fields.get("thought_signature"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| pending_gemini_signature.take())
    {
        tool_call["provider_specific_fields"] = json!({
            "provider": "gemini",
            "thought_signature": signature,
        });
    }
    if let Some(last) = messages
        .last_mut()
        .and_then(Value::as_object_mut)
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
    {
        ensure_assistant_tool_calls(last).push(tool_call);
        attach_pending_reasoning(last, pending_reasoning);
        return Ok(());
    }

    let mut message = Map::new();
    message.insert("role".to_string(), json!("assistant"));
    message.insert("content".to_string(), Value::String(String::new()));
    message.insert("tool_calls".to_string(), Value::Array(vec![tool_call]));
    attach_pending_reasoning(&mut message, pending_reasoning);
    messages.push(Value::Object(message));
    Ok(())
}

fn responses_tool_call_item_to_chat_tool_call(item: &Map<String, Value>) -> Result<Value, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "function_call must include call_id.".to_string())?;
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let arguments = if item_type == "custom_tool_call" {
        let input = item
            .get("input")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        serde_json::to_string(&json!({ "input": input }))
            .map_err(|_| "custom_tool_call input must be JSON serializable.".to_string())?
    } else {
        item.get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .to_string()
    };
    Ok(json!({
        "id": call_id,
        "type": "function",
        "function": { "name": name, "arguments": arguments }
    }))
}

fn ensure_assistant_tool_calls(message: &mut Map<String, Value>) -> &mut Vec<Value> {
    let tool_calls = message
        .entry("tool_calls".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !matches!(tool_calls, Value::Array(_)) {
        *tool_calls = Value::Array(Vec::new());
    }
    match tool_calls {
        Value::Array(items) => items,
        _ => unreachable!("tool_calls normalized to array"),
    }
}

fn attach_pending_reasoning(message: &mut Map<String, Value>, pending_reasoning: &mut String) {
    if pending_reasoning.trim().is_empty() {
        return;
    }
    if !message.contains_key("reasoning_content") {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(std::mem::take(pending_reasoning)),
        );
    } else {
        pending_reasoning.clear();
    }
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

fn extract_reasoning_text(item: &Map<String, Value>) -> String {
    let mut output = String::new();
    if let Some(summary) = item.get("summary").and_then(Value::as_array) {
        for part in summary {
            let Some(part) = part.as_object() else {
                continue;
            };
            if part.get("type").and_then(Value::as_str) != Some("summary_text") {
                continue;
            }
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                output.push_str(text);
            }
        }
    }
    if let Some(content) = item.get("content").and_then(Value::as_array) {
        for part in content {
            let Some(part) = part.as_object() else {
                continue;
            };
            if part.get("type").and_then(Value::as_str) != Some("reasoning_text") {
                continue;
            }
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                output.push_str(text);
            }
        }
    }
    output
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
