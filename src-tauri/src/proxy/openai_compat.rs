use axum::body::Bytes;
use serde_json::{json, Map, Value};

use super::{anthropic_compat, http_client::ProxyHttpClients};

mod extract;
mod input;
mod tools;
mod usage;

pub(crate) const CHAT_PATH: &str = "/v1/chat/completions";
pub(crate) const RESPONSES_PATH: &str = "/v1/responses";

pub(crate) const PROVIDER_CHAT: &str = "openai";
pub(crate) const PROVIDER_RESPONSES: &str = "openai-response";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApiFormat {
    ChatCompletions,
    Responses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatTransform {
    None,
    ChatToResponses,
    ResponsesToChat,
    ResponsesToAnthropic,
    AnthropicToResponses,
    ChatToAnthropic,
    AnthropicToChat,
}

pub(crate) fn inbound_format(path: &str) -> Option<ApiFormat> {
    match path {
        CHAT_PATH => Some(ApiFormat::ChatCompletions),
        RESPONSES_PATH => Some(ApiFormat::Responses),
        _ => None,
    }
}

pub(crate) async fn transform_request_body(
    transform: FormatTransform,
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    match transform {
        FormatTransform::None => Ok(body.clone()),
        FormatTransform::ChatToResponses => chat_request_to_responses(body),
        FormatTransform::ResponsesToChat => responses_request_to_chat(body),
        FormatTransform::ResponsesToAnthropic => {
            anthropic_compat::responses_request_to_anthropic(body, http_clients).await
        }
        FormatTransform::AnthropicToResponses => {
            anthropic_compat::anthropic_request_to_responses(body, http_clients).await
        }
        FormatTransform::ChatToAnthropic => {
            let intermediate = chat_request_to_responses(body)?;
            anthropic_compat::responses_request_to_anthropic(&intermediate, http_clients).await
        }
        FormatTransform::AnthropicToChat => {
            let intermediate = anthropic_compat::anthropic_request_to_responses(body, http_clients).await?;
            responses_request_to_chat(&intermediate)
        }
    }
}

pub(crate) fn transform_response_body(
    transform: FormatTransform,
    bytes: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    match transform {
        FormatTransform::None => Ok(bytes.clone()),
        FormatTransform::ChatToResponses => chat_response_to_responses(bytes),
        FormatTransform::ResponsesToChat => responses_response_to_chat(bytes, model_hint),
        FormatTransform::ResponsesToAnthropic => {
            anthropic_compat::responses_response_to_anthropic(bytes, model_hint)
        }
        FormatTransform::AnthropicToResponses => anthropic_compat::anthropic_response_to_responses(bytes),
        FormatTransform::ChatToAnthropic => {
            let intermediate = chat_response_to_responses(bytes)?;
            anthropic_compat::responses_response_to_anthropic(&intermediate, model_hint)
        }
        FormatTransform::AnthropicToChat => {
            let intermediate = anthropic_compat::anthropic_response_to_responses(bytes)?;
            responses_response_to_chat(&intermediate, model_hint)
        }
    }
}

fn chat_request_to_responses(body: &Bytes) -> Result<Bytes, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Err("Chat request must include messages.".to_string());
    };

    let (input, instructions) = chat_messages_to_responses_input(messages)?;

    // Responses API uses `input` (string or structured items).
    let mut output = Map::new();
    copy_key(object, &mut output, "model");
    output.insert("input".to_string(), Value::Array(input));
    if let Some(instructions) = instructions {
        output.insert("instructions".to_string(), Value::String(instructions));
    }
    copy_key(object, &mut output, "stream");
    copy_key(object, &mut output, "temperature");
    copy_key(object, &mut output, "top_p");
    copy_key(object, &mut output, "stop");
    copy_key(object, &mut output, "metadata");
    copy_key(object, &mut output, "user");
    copy_key(object, &mut output, "seed");
    copy_key(object, &mut output, "parallel_tool_calls");

    if let Some(max_output_tokens) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
        .and_then(Value::as_i64)
    {
        output.insert("max_output_tokens".to_string(), Value::Number(max_output_tokens.into()));
    }

    if let Some(tools) = object.get("tools") {
        output.insert("tools".to_string(), tools::map_chat_tools_to_responses(tools));
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            tools::map_chat_tool_choice_to_responses(tool_choice),
        );
    }

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn responses_request_to_chat(body: &Bytes) -> Result<Bytes, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    let mut messages = match object.get("input") {
        Some(Value::String(text)) => vec![json!({ "role": "user", "content": text })],
        Some(Value::Array(items)) => input::responses_input_to_chat_messages(items)?,
        _ => return Err("Responses request must include input.".to_string()),
    };

    // Responses API supports `instructions`; translate it to a system message.
    if let Some(instructions) = object.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            messages.insert(0, json!({ "role": "system", "content": instructions }));
        }
    }

    let mut output = Map::new();
    copy_key(object, &mut output, "model");
    output.insert("messages".to_string(), Value::Array(messages));
    copy_key(object, &mut output, "stream");
    copy_key(object, &mut output, "temperature");
    copy_key(object, &mut output, "top_p");
    copy_key(object, &mut output, "stop");
    copy_key(object, &mut output, "metadata");
    copy_key(object, &mut output, "user");
    copy_key(object, &mut output, "seed");
    copy_key(object, &mut output, "parallel_tool_calls");

    if let Some(max_output_tokens) = object.get("max_output_tokens").and_then(Value::as_i64) {
        // Prefer the modern chat parameter.
        output.insert(
            "max_completion_tokens".to_string(),
            Value::Number(max_output_tokens.into()),
        );
    }

    if let Some(tools) = object.get("tools") {
        output.insert("tools".to_string(), tools::map_responses_tools_to_chat(tools));
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            tools::map_responses_tool_choice_to_chat(tool_choice),
        );
    }

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn chat_messages_to_responses_input(
    messages: &[Value],
) -> Result<(Vec<Value>, Option<String>), String> {
    let mut system_texts = Vec::new();
    let mut input = Vec::new();
    let mut has_user_message = false;

    for message in messages {
        let Some(message) = message.as_object() else {
            continue;
        };

        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        match role {
            "system" => push_chat_system_message(&mut system_texts, message),
            "user" => push_chat_user_message(&mut input, &mut has_user_message, message)?,
            "assistant" => push_chat_assistant_message(&mut input, &mut has_user_message, message)?,
            "tool" => push_chat_tool_message(&mut input, message),
            _ => {}
        }
    }

    let instructions = join_non_empty_lines(system_texts);
    Ok((input, instructions))
}

fn push_chat_system_message(system_texts: &mut Vec<String>, message: &Map<String, Value>) {
    if let Some(text) = extract_text_from_chat_content(message.get("content")) {
        system_texts.push(text);
    }
}

fn push_chat_user_message(
    input: &mut Vec<Value>,
    has_user_message: &mut bool,
    message: &Map<String, Value>,
) -> Result<(), String> {
    let parts = chat_content_to_responses_message_parts(message.get("content"), "input_text")?;
    if parts.is_empty() {
        return Ok(());
    }
    input.push(json!({ "type": "message", "role": "user", "content": parts }));
    *has_user_message = true;
    Ok(())
}

fn push_chat_assistant_message(
    input: &mut Vec<Value>,
    has_user_message: &mut bool,
    message: &Map<String, Value>,
) -> Result<(), String> {
    // Responses API expects assistant message content parts to use output types.
    // This matches OpenAI's schema and avoids errors like: "supported values are output_text/refusal".
    let parts = chat_content_to_responses_message_parts(message.get("content"), "output_text")?;
    let tool_calls = chat_tool_calls_to_responses_items(message.get("tool_calls"));
    let legacy_call = chat_function_call_to_responses_item(message.get("function_call"));

    let has_payload =
        !parts.is_empty() || !tool_calls.is_empty() || legacy_call.is_some();
    if has_payload && !*has_user_message {
        input.push(user_placeholder_item());
        *has_user_message = true;
    }

    if !parts.is_empty() {
        input.push(json!({ "type": "message", "role": "assistant", "content": parts }));
    }
    input.extend(tool_calls);
    if let Some(item) = legacy_call {
        input.push(item);
    }
    Ok(())
}

fn push_chat_tool_message(input: &mut Vec<Value>, message: &Map<String, Value>) {
    let call_id = message.get("tool_call_id").and_then(Value::as_str).unwrap_or("");
    let output = stringify_any_json(message.get("content"));
    input.push(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    }));
}

fn extract_text_from_chat_content(content: Option<&Value>) -> Option<String> {
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
            if combined.trim().is_empty() { None } else { Some(combined) }
        }
        Value::Object(object) => object.get("text").and_then(Value::as_str).map(|t| t.to_string()),
        _ => None,
    }
}

fn chat_content_to_responses_message_parts(
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

fn chat_tool_calls_to_responses_items(value: Option<&Value>) -> Vec<Value> {
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

fn chat_function_call_to_responses_item(value: Option<&Value>) -> Option<Value> {
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

fn stringify_any_json(value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(Value::String(text)) => text.to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn user_placeholder_item() -> Value {
    json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": "..." }]
    })
}

fn join_non_empty_lines(texts: Vec<String>) -> Option<String> {
    let combined = texts
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if combined.is_empty() { None } else { Some(combined) }
}

fn responses_response_to_chat(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let extracted = extract::extract_responses_output(&value);
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-proxy");
    let created = object.get("created_at").and_then(Value::as_i64).unwrap_or(0);
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .or(model_hint)
        .unwrap_or("unknown");

    let usage = object
        .get("usage")
        .and_then(|usage| usage::map_usage_responses_to_chat(usage));

    let finish_reason = if extracted.tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };

    let mut message = json!({
        "role": "assistant",
        "content": extracted.content
    });
    if let Some(parts) = extracted.content_parts {
        if let Some(message) = message.as_object_mut() {
            message.insert("content_parts".to_string(), Value::Array(parts));
        }
    }
    if !extracted.tool_calls.is_empty() {
        if let Some(message) = message.as_object_mut() {
            message.insert("tool_calls".to_string(), Value::Array(extracted.tool_calls));
        }
    }

    let output = json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": message,
                "finish_reason": finish_reason
            }
        ],
        "usage": usage
    });

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn chat_response_to_responses(bytes: &Bytes) -> Result<Bytes, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let content = extract::extract_chat_choice_text(&value).unwrap_or_default();
    let tool_calls = extract::extract_chat_tool_calls(&value);
    let parallel_tool_calls = tool_calls.len() > 1;
    let id = object.get("id").and_then(Value::as_str).unwrap_or("resp-proxy");
    let created = object.get("created").and_then(Value::as_i64).unwrap_or(0);
    let model = object.get("model").and_then(Value::as_str).unwrap_or("unknown");

    let usage = object
        .get("usage")
        .and_then(|usage| usage::map_usage_chat_to_responses(usage));

    let mut output = Vec::new();
    if !content.trim().is_empty() || tool_calls.is_empty() {
        output.push(json!({
            "type": "message",
            "id": "msg_proxy",
            "status": "completed",
            "role": "assistant",
            "content": [
                { "type": "output_text", "text": content, "annotations": [] }
            ]
        }));
    }
    for call in tool_calls {
        output.push(json!({
            "id": call.item_id,
            "type": "function_call",
            "status": "completed",
            "arguments": call.arguments,
            "call_id": call.call_id,
            "name": call.name
        }));
    }

    let output = json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": "completed",
        "error": null,
        "model": model,
        "parallel_tool_calls": parallel_tool_calls,
        "output": output,
        "usage": usage
    });

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn copy_key(source: &serde_json::Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key) {
        target.insert(key.to_string(), value.clone());
    }
}

#[cfg(test)]
mod tests;
