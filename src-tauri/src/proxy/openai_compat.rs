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

    // Responses API uses `input` for message arrays.
    let mut output = Map::new();
    copy_key(object, &mut output, "model");
    output.insert("input".to_string(), Value::Array(messages.clone()));
    copy_key(object, &mut output, "stream");
    copy_key(object, &mut output, "temperature");
    copy_key(object, &mut output, "top_p");
    copy_key(object, &mut output, "stop");
    copy_key(object, &mut output, "metadata");
    copy_key(object, &mut output, "user");
    copy_key(object, &mut output, "seed");

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
