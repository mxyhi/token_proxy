use axum::body::Bytes;
use serde_json::{json, Map, Value};

use super::{
    anthropic_compat,
    compat_content,
    compat_reason,
    codex_compat,
    gemini_compat,
    http_client::ProxyHttpClients,
};

mod extract;
mod input;
mod message;
mod tools;
mod usage;
pub(crate) use usage::map_usage_chat_to_responses;

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
    GeminiToAnthropic,
    AnthropicToGemini,
    ChatToGemini,
    GeminiToChat,
    ResponsesToGemini,
    GeminiToResponses,
    KiroToAnthropic,
    ChatToCodex,
    ResponsesToCodex,
    CodexToChat,
    CodexToResponses,
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
    model_hint: Option<&str>,
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
        FormatTransform::GeminiToAnthropic => {
            gemini_request_to_anthropic(body, http_clients, model_hint).await
        }
        FormatTransform::AnthropicToGemini => anthropic_request_to_gemini(body, http_clients).await,
        FormatTransform::ChatToGemini => gemini_compat::chat_request_to_gemini(body),
        FormatTransform::GeminiToChat => gemini_compat::gemini_request_to_chat(body, model_hint),
        FormatTransform::ResponsesToGemini => responses_request_to_gemini(body),
        FormatTransform::GeminiToResponses => gemini_request_to_responses(body, model_hint),
        FormatTransform::KiroToAnthropic => Ok(body.clone()),
        FormatTransform::ChatToCodex => codex_compat::chat_request_to_codex(body, model_hint),
        FormatTransform::ResponsesToCodex => codex_compat::responses_request_to_codex(body, model_hint),
        FormatTransform::CodexToChat | FormatTransform::CodexToResponses => Ok(body.clone()),
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
        FormatTransform::GeminiToAnthropic => gemini_response_to_anthropic(bytes, model_hint),
        FormatTransform::AnthropicToGemini => anthropic_response_to_gemini(bytes, model_hint),
        FormatTransform::ChatToGemini => gemini_compat::chat_response_to_gemini(bytes, model_hint),
        FormatTransform::GeminiToChat => gemini_compat::gemini_response_to_chat(bytes, model_hint),
        FormatTransform::ResponsesToGemini => responses_response_to_gemini(bytes, model_hint),
        FormatTransform::GeminiToResponses => gemini_response_to_responses(bytes, model_hint),
        FormatTransform::KiroToAnthropic => {
            Err("Kiro response conversion is handled upstream.".to_string())
        }
        FormatTransform::CodexToChat | FormatTransform::CodexToResponses => {
            Err("Codex response conversion is handled upstream.".to_string())
        }
        FormatTransform::ChatToCodex | FormatTransform::ResponsesToCodex => {
            Err("Codex response conversion is handled upstream.".to_string())
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
    copy_key(object, &mut output, "modalities");
    copy_key(object, &mut output, "audio");

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
    if let Some(response_format) = object.get("response_format") {
        let mut text_obj = Map::new();
        text_obj.insert("format".to_string(), response_format.clone());
        output.insert("text".to_string(), Value::Object(text_obj));
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
    copy_key(object, &mut output, "modalities");
    copy_key(object, &mut output, "audio");

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
    if let Some(text_format) = object
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))
    {
        output.insert("response_format".to_string(), text_format.clone());
    }

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn responses_request_to_gemini(body: &Bytes) -> Result<Bytes, String> {
    let intermediate = responses_request_to_chat(body)?;
    gemini_compat::chat_request_to_gemini(&intermediate)
}

fn gemini_request_to_responses(body: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let intermediate = gemini_compat::gemini_request_to_chat(body, model_hint)?;
    chat_request_to_responses(&intermediate)
}

fn responses_response_to_gemini(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let intermediate = responses_response_to_chat(bytes, model_hint)?;
    gemini_compat::chat_response_to_gemini(&intermediate, model_hint)
}

fn gemini_response_to_responses(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let intermediate = gemini_compat::gemini_response_to_chat(bytes, model_hint)?;
    chat_response_to_responses(&intermediate)
}

async fn gemini_request_to_anthropic(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let intermediate = gemini_compat::gemini_request_to_chat(body, model_hint)?;
    let intermediate = chat_request_to_responses(&intermediate)?;
    anthropic_compat::responses_request_to_anthropic(&intermediate, http_clients).await
}

async fn anthropic_request_to_gemini(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    let intermediate = anthropic_compat::anthropic_request_to_responses(body, http_clients).await?;
    let intermediate = responses_request_to_chat(&intermediate)?;
    gemini_compat::chat_request_to_gemini(&intermediate)
}

fn gemini_response_to_anthropic(
    bytes: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let intermediate = gemini_compat::gemini_response_to_chat(bytes, model_hint)?;
    let intermediate = chat_response_to_responses(&intermediate)?;
    anthropic_compat::responses_response_to_anthropic(&intermediate, model_hint)
}

fn anthropic_response_to_gemini(
    bytes: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let intermediate = anthropic_compat::anthropic_response_to_responses(bytes)?;
    let intermediate = responses_response_to_chat(&intermediate, model_hint)?;
    gemini_compat::chat_response_to_gemini(&intermediate, model_hint)
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

    let instructions = message::join_non_empty_lines(system_texts);
    Ok((input, instructions))
}

fn push_chat_system_message(system_texts: &mut Vec<String>, message: &Map<String, Value>) {
    if let Some(text) = message::extract_text_from_chat_content(message.get("content")) {
        system_texts.push(text);
    }
}

fn push_chat_user_message(
    input: &mut Vec<Value>,
    has_user_message: &mut bool,
    message: &Map<String, Value>,
) -> Result<(), String> {
    let parts = message::chat_content_to_responses_message_parts(message.get("content"), "input_text")?;
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
    let parts = message::chat_content_to_responses_message_parts(message.get("content"), "output_text")?;
    let tool_calls = message::chat_tool_calls_to_responses_items(message.get("tool_calls"));
    let legacy_call = message::chat_function_call_to_responses_item(message.get("function_call"));

    let has_payload =
        !parts.is_empty() || !tool_calls.is_empty() || legacy_call.is_some();
    if has_payload && !*has_user_message {
        input.push(message::user_placeholder_item());
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
    let output = message::stringify_any_json(message.get("content"));
    input.push(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    }));
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

    let finish_reason =
        compat_reason::chat_finish_reason_from_response_object(object, !extracted.tool_calls.is_empty());

    let reasoning_text = extracted.reasoning_text.clone();
    let mut message = json!({
        "role": "assistant",
        "content": compat_content::chat_message_content_from_responses_parts(
            &extracted.content_parts,
        )
    });
    if let Some(message) = message.as_object_mut() {
        if !reasoning_text.trim().is_empty() {
            message.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning_text),
            );
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
    let finish_reason = object
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str);
    let (status, incomplete_reason) =
        compat_reason::responses_status_from_chat_finish_reason(finish_reason);
    let status = status.unwrap_or("completed");
    let incomplete_details = incomplete_reason
        .map(|reason| json!({ "reason": reason }))
        .unwrap_or(Value::Null);

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
        "status": status,
        "error": null,
        "incomplete_details": incomplete_details,
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

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "openai_compat.test.rs"]
mod tests;
