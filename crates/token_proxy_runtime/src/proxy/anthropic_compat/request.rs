use axum::body::Bytes;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use token_proxy_protocol::compat_reason::SummaryVisibility;

use super::super::{http_client::ProxyHttpClients, model};
use super::media;
use super::tools;
use crate::proxy::codex_tool_types::is_codex_tool_call_output_item_type;

const MIN_RESPONSES_MAX_OUTPUT_TOKENS: i64 = 128;

pub(super) async fn responses_request_to_anthropic(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    let mut value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object_mut() else {
        return Err("Request body must be a JSON object.".to_string());
    };
    token_proxy_protocol::tool_identity::flatten_responses_namespaces(object, &[])?;

    let model = object
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| "Request must include model.".to_string())?;

    let stream = object
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let max_tokens = object
        .get("max_output_tokens")
        .or_else(|| object.get("max_tokens"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(4096);

    let mut system_texts = Vec::new();
    if let Some(instructions) = object.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            system_texts.push(instructions.to_string());
        }
    }

    let input = object
        .get("input")
        .ok_or_else(|| "Request must include input.".to_string())?;
    let mut messages = Vec::new();
    responses_input_to_claude_messages(input, &mut system_texts, &mut messages, http_clients)
        .await?;
    messages = sanitize_claude_messages_for_anthropic(messages);

    let mut out = Map::new();
    out.insert("model".to_string(), Value::String(model.to_string()));
    out.insert("max_tokens".to_string(), Value::Number(max_tokens.into()));
    out.insert("stream".to_string(), Value::Bool(stream));
    out.insert("messages".to_string(), Value::Array(messages));

    if let Some(system) = join_system_texts(system_texts) {
        out.insert("system".to_string(), system_blocks_from_text(system));
    }

    if let Some(temperature) = object.get("temperature") {
        out.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = object.get("top_p") {
        out.insert("top_p".to_string(), top_p.clone());
    }

    map_responses_reasoning_to_anthropic(object.get("reasoning"), &mut out);

    if let Some(stop_sequences) =
        tools::map_openai_stop_to_anthropic_stop_sequences(object.get("stop"))
    {
        out.insert("stop_sequences".to_string(), stop_sequences);
    }

    if let Some(tools_value) = object.get("tools") {
        out.insert(
            "tools".to_string(),
            tools::map_responses_tools_to_anthropic(tools_value),
        );
    }

    let parallel_tool_calls = object.get("parallel_tool_calls").and_then(Value::as_bool);
    if let Some(tool_choice) = tools::map_responses_tool_choice_to_anthropic(
        object.get("tool_choice"),
        parallel_tool_calls,
    ) {
        out.insert("tool_choice".to_string(), tool_choice);
    }

    serde_json::to_vec(&Value::Object(out))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

pub(super) async fn anthropic_request_to_responses(
    body: &Bytes,
    _http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    let model = object
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| "Request must include model.".to_string())?;

    let stream = object
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let max_output_tokens = object
        .get("max_tokens")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(4096);

    let mut input_items = Vec::new();

    if let Some(system) = object.get("system") {
        let system_parts = claude_system_to_responses_parts(system);
        if !system_parts.is_empty() {
            input_items.push(json!({
                "type": "message",
                "role": "developer",
                "content": system_parts
            }));
        }
    }

    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Err("Request must include messages.".to_string());
    };
    for message in messages {
        claude_message_to_responses_input_items(message, &mut input_items)?;
    }

    let mut out = Map::new();
    out.insert("model".to_string(), Value::String(model.to_string()));
    out.insert(
        "max_output_tokens".to_string(),
        Value::Number(
            max_output_tokens
                .max(MIN_RESPONSES_MAX_OUTPUT_TOKENS)
                .into(),
        ),
    );
    out.insert("stream".to_string(), Value::Bool(stream));
    out.insert("input".to_string(), Value::Array(input_items));
    out.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    out.insert("store".to_string(), Value::Bool(false));
    out.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    out.insert("text".to_string(), json!({ "verbosity": "medium" }));

    if !model::is_openai_responses_reasoning_model(model) {
        if let Some(temperature) = object.get("temperature") {
            out.insert("temperature".to_string(), temperature.clone());
        }
        if let Some(top_p) = object.get("top_p") {
            out.insert("top_p".to_string(), top_p.clone());
        }
    }

    if let Some(reasoning) = map_anthropic_thinking_to_responses_reasoning(
        object.get("thinking"),
        object.get("output_config"),
    ) {
        out.insert("reasoning".to_string(), reasoning);
    }

    if let Some(text_format) = map_anthropic_output_format_to_responses_text(
        object.get("output_format"),
        object.get("output_config"),
    ) {
        out.insert(
            "text".to_string(),
            merge_responses_text_defaults(text_format),
        );
    }

    if let Some(context_management) =
        map_anthropic_context_management_to_responses(object.get("context_management"))
    {
        out.insert("context_management".to_string(), context_management);
    }

    if let Some(user) = map_anthropic_metadata_to_responses_user(object.get("metadata")) {
        out.insert("user".to_string(), Value::String(user));
    }

    if let Some(stop) =
        tools::map_anthropic_stop_sequences_to_openai_stop(object.get("stop_sequences"))
    {
        out.insert("stop".to_string(), stop);
    }

    if let Some(tools_value) = object.get("tools") {
        out.insert(
            "tools".to_string(),
            tools::map_anthropic_tools_to_responses(tools_value),
        );
    }

    let (tool_choice, parallel_tool_calls) =
        tools::map_anthropic_tool_choice_to_responses(object.get("tool_choice"));
    if let Some(tool_choice) = tool_choice {
        out.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(parallel_tool_calls) = parallel_tool_calls {
        out.insert(
            "parallel_tool_calls".to_string(),
            Value::Bool(parallel_tool_calls),
        );
    }

    serde_json::to_vec(&Value::Object(out))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

async fn responses_input_to_claude_messages(
    input: &Value,
    system_texts: &mut Vec<String>,
    messages: &mut Vec<Value>,
    http_clients: &ProxyHttpClients,
) -> Result<(), String> {
    match input {
        Value::String(text) => {
            let content = vec![json!({ "type": "text", "text": text })];
            messages.push(json!({ "role": "user", "content": content }));
        }
        Value::Array(items) => {
            for item in items {
                responses_input_item_to_claude_messages(item, system_texts, messages, http_clients)
                    .await?;
            }
        }
        _ => return Err("Responses input must be a string or array.".to_string()),
    }
    Ok(())
}

async fn responses_input_item_to_claude_messages(
    item: &Value,
    system_texts: &mut Vec<String>,
    messages: &mut Vec<Value>,
    http_clients: &ProxyHttpClients,
) -> Result<(), String> {
    // Accept Chat-style `{role, content}` items, as some clients send that into /v1/responses.
    if item.get("role").and_then(Value::as_str).is_some() {
        let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = item.get("content");
        // Anthropic messages only accept user/assistant roles; instruction roles belong in system.
        if matches!(role, "system" | "developer") {
            if let Some(text) = extract_text_from_any_content(content) {
                if !text.trim().is_empty() {
                    system_texts.push(text);
                }
            }
            return Ok(());
        }
        let blocks = responses_message_content_to_claude_blocks(content, http_clients).await?;
        push_claude_message(messages, role, blocks);
        return Ok(());
    }

    let Some(object) = item.as_object() else {
        return Ok(());
    };
    let item_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    match item_type {
        "message" => {
            let role = object.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = object.get("content");
            if matches!(role, "system" | "developer") {
                if let Some(text) = extract_text_from_any_content(content) {
                    if !text.trim().is_empty() {
                        system_texts.push(text);
                    }
                }
                return Ok(());
            }
            let blocks = responses_message_content_to_claude_blocks(content, http_clients).await?;
            push_claude_message(messages, role, blocks);
        }
        "function_call" | "custom_tool_call" => {
            let tool_use_id = object
                .get("call_id")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("tool_use_proxy");
            let name = object.get("name").and_then(Value::as_str).unwrap_or("");
            let input = if item_type == "custom_tool_call" {
                custom_tool_input_to_claude_object(object.get("input"))
            } else {
                let arguments = object
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                parse_tool_input_object(arguments)
            };
            let block = json!({
                "type": "tool_use",
                "id": tool_use_id,
                "name": name,
                "input": input
            });
            push_tool_use_block(messages, block);
            if item_type == "custom_tool_call" {
                tracing::debug!("mapped Responses custom tool call to Anthropic tool use");
            }
        }
        item_type if is_codex_tool_call_output_item_type(item_type) => {
            let tool_use_id = object.get("call_id").and_then(Value::as_str).unwrap_or("");
            let content =
                responses_function_call_output_to_claude_content(object, http_clients).await?;
            let mut block = Map::new();
            block.insert("type".to_string(), json!("tool_result"));
            block.insert("tool_use_id".to_string(), json!(tool_use_id));
            block.insert("content".to_string(), content);
            if object
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                block.insert("is_error".to_string(), Value::Bool(true));
            }
            push_tool_result_block(messages, Value::Object(block));
        }
        "web_search_call" => {
            if let Some((use_block, result_block)) =
                super::web_search::responses_search_call_to_claude(object)
            {
                push_tool_use_block(messages, use_block);
                push_tool_result_block(messages, result_block);
                tracing::debug!("mapped Responses web_search_call to Claude server tool blocks");
            }
        }
        _ => {}
    }
    Ok(())
}

async fn responses_function_call_output_to_claude_content(
    item: &Map<String, Value>,
    http_clients: &ProxyHttpClients,
) -> Result<Value, String> {
    // Tool results carry looser structured payloads than normal message content.
    // Prefer `output_parts` when present so we preserve text/media/error semantics instead of
    // collapsing everything into one string.
    if let Some(output_parts) = item.get("output_parts") {
        return responses_tool_result_content_to_claude(output_parts, http_clients).await;
    }

    match item.get("output") {
        Some(Value::String(text)) => Ok(Value::String(text.clone())),
        Some(other) => responses_tool_result_content_to_claude(other, http_clients).await,
        None => Ok(Value::String(String::new())),
    }
}

async fn responses_tool_result_content_to_claude(
    value: &Value,
    http_clients: &ProxyHttpClients,
) -> Result<Value, String> {
    match value {
        Value::Null => Ok(Value::String(String::new())),
        Value::String(text) => Ok(Value::String(text.clone())),
        Value::Array(parts) => Ok(Value::Array(
            responses_tool_result_parts_to_claude_blocks(parts, http_clients).await?,
        )),
        Value::Object(part) => {
            if let Some(block) =
                responses_tool_result_part_to_claude_block(part, http_clients).await?
            {
                return Ok(Value::Array(vec![block]));
            }
            Ok(Value::String(
                serde_json::to_string(value).unwrap_or_default(),
            ))
        }
        _ => Ok(Value::String(
            serde_json::to_string(value).unwrap_or_default(),
        )),
    }
}

async fn responses_tool_result_parts_to_claude_blocks(
    parts: &[Value],
    http_clients: &ProxyHttpClients,
) -> Result<Vec<Value>, String> {
    let mut blocks = Vec::new();
    for part in parts {
        match part {
            Value::String(text) => blocks.push(tool_result_text_block(text)),
            Value::Object(object) => {
                if let Some(block) =
                    responses_tool_result_part_to_claude_block(object, http_clients).await?
                {
                    blocks.push(block);
                } else {
                    blocks.push(tool_result_text_block(
                        &serde_json::to_string(part).unwrap_or_default(),
                    ));
                }
            }
            other => blocks.push(tool_result_text_block(
                &serde_json::to_string(other).unwrap_or_default(),
            )),
        }
    }
    Ok(blocks)
}

async fn responses_tool_result_part_to_claude_block(
    part: &Map<String, Value>,
    http_clients: &ProxyHttpClients,
) -> Result<Option<Value>, String> {
    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
    match part_type {
        "" => Ok(part
            .get("text")
            .and_then(Value::as_str)
            .map(tool_result_text_block)),
        "input_text" | "output_text" | "text" => Ok(part
            .get("text")
            .and_then(Value::as_str)
            .map(tool_result_text_block)),
        "refusal" => Ok(part
            .get("refusal")
            .or_else(|| part.get("text"))
            .and_then(Value::as_str)
            .map(tool_result_text_block)),
        "input_image" | "image_url" => {
            media::input_image_part_to_claude_block(part, http_clients).await
        }
        "input_file" => media::input_file_part_to_claude_block(part, http_clients).await,
        "image" | "document" => Ok(Some(Value::Object(part.clone()))),
        _ => Ok(part
            .get("text")
            .and_then(Value::as_str)
            .map(tool_result_text_block)),
    }
}

fn tool_result_text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

async fn responses_message_content_to_claude_blocks(
    content: Option<&Value>,
    http_clients: &ProxyHttpClients,
) -> Result<Vec<Value>, String> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    match content {
        Value::String(text) => Ok(vec![json!({ "type": "text", "text": text })]),
        Value::Array(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                match part_type {
                    "input_text" | "output_text" | "text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                    }
                    "refusal" => {
                        // Some OpenAI Responses payloads represent refusals as dedicated parts.
                        let text = part
                            .get("refusal")
                            .or_else(|| part.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if !text.is_empty() {
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                    }
                    "input_image" => {
                        if let Some(block) =
                            media::input_image_part_to_claude_block(part, http_clients).await?
                        {
                            blocks.push(block);
                        }
                    }
                    "input_file" => {
                        if let Some(block) =
                            media::input_file_part_to_claude_block(part, http_clients).await?
                        {
                            blocks.push(block);
                        }
                    }
                    _ => {}
                }
            }
            Ok(blocks)
        }
        _ => Ok(Vec::new()),
    }
}

fn claude_message_to_responses_input_items(
    message: &Value,
    input_items: &mut Vec<Value>,
) -> Result<(), String> {
    let Some(message) = message.as_object() else {
        return Ok(());
    };
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    if role == "system" {
        return Ok(());
    }

    let content = message.get("content");
    let blocks = claude_content_to_blocks(content);

    let mut message_parts = Vec::new();
    let mut function_call_items = Vec::new();
    let mut function_output_items = Vec::new();
    let text_part_type = match role {
        // OpenAI Responses schema expects assistant messages in `input` to use output types.
        // This avoids errors like: "Invalid value: 'input_text'. Supported values are: 'output_text' and 'refusal'."
        "assistant" => "output_text",
        _ => "input_text",
    };
    for block in &blocks {
        let Some(block) = block.as_object() else {
            continue;
        };
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    message_parts.push(json!({ "type": text_part_type, "text": text }));
                }
            }
            "thinking" => {
                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                    if !text.is_empty() {
                        message_parts.push(json!({ "type": "output_text", "text": text }));
                    }
                }
            }
            "image" => {
                if let Some(part) = media::claude_image_block_to_input_image_part(block) {
                    message_parts.push(part);
                }
            }
            "document" => {
                if let Some(part) = media::claude_document_block_to_input_file_part(block) {
                    message_parts.push(part);
                }
            }
            "tool_use" => {}
            "tool_result" => {}
            _ => {}
        }
    }
    for block in blocks {
        let Some(block) = block.as_object() else {
            continue;
        };
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "tool_use" => {
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_proxy");
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                function_call_items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments
                }));
            }
            "tool_result" => {
                let call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let output_raw = block.get("content").cloned().unwrap_or_else(|| json!(""));
                let (output_text, mut media_parts) =
                    claude_tool_result_content_to_responses_output(&output_raw);
                message_parts.append(&mut media_parts);
                let is_error = block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut item = Map::new();
                item.insert("type".to_string(), json!("function_call_output"));
                item.insert("call_id".to_string(), Value::String(call_id.to_string()));
                item.insert("output".to_string(), Value::String(output_text));
                if is_error {
                    item.insert("is_error".to_string(), Value::Bool(true));
                }
                function_output_items.push(Value::Object(item));
            }
            _ => {}
        }
    }

    if role == "user" {
        input_items.append(&mut function_output_items);
    }
    if !message_parts.is_empty() {
        input_items.push(json!({
            "type": "message",
            "role": role,
            "content": message_parts
        }));
    }
    if role != "user" {
        input_items.append(&mut function_output_items);
    }
    input_items.append(&mut function_call_items);

    Ok(())
}

fn claude_system_to_responses_parts(value: &Value) -> Vec<Value> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() || is_anthropic_billing_header(text) {
                Vec::new()
            } else {
                vec![json!({ "type": "input_text", "text": text })]
            }
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_object())
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty() && !is_anthropic_billing_header(text))
            .map(|text| json!({ "type": "input_text", "text": text }))
            .collect(),
        _ => Vec::new(),
    }
}

fn is_anthropic_billing_header(text: &str) -> bool {
    text.starts_with("x-anthropic-billing-header: ")
}

fn claude_tool_result_content_to_responses_output(content: &Value) -> (String, Vec<Value>) {
    match content {
        Value::String(text) => (non_empty_tool_output(text), Vec::new()),
        Value::Array(blocks) => {
            let mut text_parts = Vec::new();
            let mut media_parts = Vec::new();
            for block in blocks {
                let Some(block) = block.as_object() else {
                    continue;
                };
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                    "image" => {
                        if let Some(part) = media::claude_image_block_to_input_image_part(block) {
                            media_parts.push(part);
                        }
                    }
                    "document" => {
                        if let Some(part) = media::claude_document_block_to_input_file_part(block) {
                            media_parts.push(part);
                        }
                    }
                    _ => {}
                }
            }
            (non_empty_tool_output(&text_parts.join("\n\n")), media_parts)
        }
        other => (
            non_empty_tool_output(&serde_json::to_string(other).unwrap_or_default()),
            Vec::new(),
        ),
    }
}

fn non_empty_tool_output(text: &str) -> String {
    if text.is_empty() {
        "(empty)".to_string()
    } else {
        text.to_string()
    }
}

fn join_system_texts(texts: Vec<String>) -> Option<String> {
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

fn system_blocks_from_text(text: String) -> Value {
    // new-api style: `system` uses array blocks for better compatibility.
    // Keep the original newlines inside the single block (avoid splitting).
    json!([{ "type": "text", "text": text }])
}

fn extract_text_from_any_content(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    match value {
        Value::String(text) => Some(text.to_string()),
        Value::Array(parts) => {
            let mut combined = String::new();
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    combined.push_str(text);
                }
            }
            if combined.is_empty() {
                None
            } else {
                Some(combined)
            }
        }
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(|t| t.to_string()),
        _ => None,
    }
}

fn parse_tool_input_object(arguments: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(arguments).ok();
    match parsed {
        Some(Value::Object(object)) => Value::Object(object),
        Some(other) => json!({ "_": other }),
        None => json!({ "_raw": arguments }),
    }
}

// Anthropic requires every tool input to be an object, while custom tools may carry freeform JSON.
fn custom_tool_input_to_claude_object(input: Option<&Value>) -> Value {
    match input {
        Some(Value::String(input)) => parse_tool_input_object(input),
        Some(Value::Object(input)) => Value::Object(input.clone()),
        Some(input) => json!({ "_": input }),
        None => json!({}),
    }
}

const EMPTY_MESSAGE_PLACEHOLDER: &str =
    "[System: Empty message content sanitised to satisfy protocol]";
const MISSING_TOOL_RESULT_PREFIX: &str =
    "[System: Tool execution skipped/interrupted by user. No result provided for tool '";

fn sanitize_claude_messages_for_anthropic(messages: Vec<Value>) -> Vec<Value> {
    let mut sanitized = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        let Some(current) = sanitize_single_claude_message(&messages[index]) else {
            index += 1;
            continue;
        };
        let role = current
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");

        if role == "assistant" && sanitized.is_empty() {
            sanitized.push(claude_user_placeholder_message());
        }

        if role != "assistant" {
            sanitized.push(current);
            index += 1;
            continue;
        }

        let expected_tool_uses = collect_expected_tool_uses(&current);
        sanitized.push(current);
        index += 1;

        if expected_tool_uses.is_empty() {
            continue;
        }

        let mut following_messages = Vec::new();
        while index < messages.len() {
            let Some(next) = sanitize_single_claude_message(&messages[index]) else {
                index += 1;
                continue;
            };
            if next.get("role").and_then(Value::as_str) == Some("assistant") {
                break;
            }
            following_messages.push(next);
            index += 1;
        }

        let (mut cleaned_following, matched_tool_use_ids) =
            sanitize_following_tool_result_block(following_messages, &expected_tool_uses);
        sanitized.append(&mut cleaned_following);

        let missing_tool_results = expected_tool_uses
            .iter()
            .filter_map(|(tool_use_id, tool_name)| {
                if matched_tool_use_ids.contains(tool_use_id) {
                    None
                } else {
                    Some((tool_use_id.clone(), tool_name.clone()))
                }
            })
            .collect::<Vec<_>>();
        if !missing_tool_results.is_empty() {
            sanitized.push(dummy_tool_result_message(&missing_tool_results));
        }
    }

    sanitized
}

fn sanitize_single_claude_message(message: &Value) -> Option<Value> {
    let mut message = message.as_object()?.clone();
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user")
        .to_string();
    let content = message
        .get("content")
        .map(|content| claude_content_to_blocks(Some(content)))
        .unwrap_or_default();
    let has_tool_blocks = content.iter().any(is_claude_tool_block);

    let mut sanitized_content = Vec::new();
    for mut block in content {
        let Some(object) = block.as_object_mut() else {
            continue;
        };
        let block_type = object.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                let text = object.get("text").and_then(Value::as_str).unwrap_or("");
                if text.trim().is_empty() {
                    if has_tool_blocks {
                        continue;
                    }
                    continue;
                }
                sanitized_content.push(block);
            }
            "tool_use" => {
                let sanitized_id = sanitize_anthropic_tool_use_id(
                    object.get("id").and_then(Value::as_str).unwrap_or(""),
                );
                object.insert("id".to_string(), Value::String(sanitized_id));
                sanitized_content.push(block);
            }
            "tool_result" => {
                let sanitized_id = sanitize_anthropic_tool_use_id(
                    object
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                );
                object.insert("tool_use_id".to_string(), Value::String(sanitized_id));
                sanitized_content.push(block);
            }
            _ => sanitized_content.push(block),
        }
    }

    if sanitized_content.is_empty() && matches!(role.as_str(), "user" | "assistant") {
        sanitized_content.push(text_block(EMPTY_MESSAGE_PLACEHOLDER));
    }

    message.insert("content".to_string(), Value::Array(sanitized_content));
    Some(Value::Object(message))
}

fn sanitize_following_tool_result_block(
    messages: Vec<Value>,
    expected_tool_uses: &HashMap<String, String>,
) -> (Vec<Value>, HashSet<String>) {
    let mut matched_tool_use_ids = HashSet::new();
    let mut keep_positions = HashSet::new();

    for message_index in (0..messages.len()).rev() {
        let Some(content) = messages[message_index]
            .get("content")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block_index in (0..content.len()).rev() {
            let Some(block) = content[block_index].as_object() else {
                continue;
            };
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !expected_tool_uses.contains_key(&tool_use_id) {
                continue;
            }
            if matched_tool_use_ids.insert(tool_use_id) {
                keep_positions.insert((message_index, block_index));
            }
        }
    }

    let mut cleaned_messages = Vec::new();
    for (message_index, message) in messages.into_iter().enumerate() {
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            cleaned_messages.push(message);
            continue;
        };
        let filtered_content = content
            .iter()
            .enumerate()
            .filter_map(|(block_index, block)| {
                let Some(object) = block.as_object() else {
                    return Some(block.clone());
                };
                if object.get("type").and_then(Value::as_str) != Some("tool_result") {
                    return Some(block.clone());
                }
                let tool_use_id = object
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !expected_tool_uses.contains_key(tool_use_id) {
                    return None;
                }
                if keep_positions.contains(&(message_index, block_index)) {
                    return Some(block.clone());
                }
                None
            })
            .collect::<Vec<_>>();
        if filtered_content.is_empty() {
            continue;
        }
        let mut message = message;
        if let Some(object) = message.as_object_mut() {
            object.insert("content".to_string(), Value::Array(filtered_content));
        }
        cleaned_messages.push(message);
    }

    (cleaned_messages, matched_tool_use_ids)
}

fn collect_expected_tool_uses(message: &Value) -> HashMap<String, String> {
    let mut expected = HashMap::new();
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return expected;
    };
    for block in content {
        let Some(block) = block.as_object() else {
            continue;
        };
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let tool_use_id = block.get("id").and_then(Value::as_str).unwrap_or("");
        if tool_use_id.is_empty() {
            continue;
        }
        let tool_name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown_tool");
        expected.insert(tool_use_id.to_string(), tool_name.to_string());
    }
    expected
}

fn sanitize_anthropic_tool_use_id(tool_use_id: &str) -> String {
    let sanitized = tool_use_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "tool_use_id".to_string()
    } else {
        sanitized
    }
}

fn dummy_tool_result_message(missing_tool_results: &[(String, String)]) -> Value {
    let content = missing_tool_results
        .iter()
        .map(|(tool_use_id, tool_name)| {
            json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": format!("{MISSING_TOOL_RESULT_PREFIX}{tool_name}'.]")
            })
        })
        .collect::<Vec<_>>();
    json!({ "role": "user", "content": content })
}

fn claude_user_placeholder_message() -> Value {
    json!({
        "role": "user",
        "content": [text_block("...")]
    })
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn is_claude_tool_block(block: &Value) -> bool {
    let Some(object) = block.as_object() else {
        return false;
    };
    matches!(
        object.get("type").and_then(Value::as_str),
        Some("tool_use" | "tool_result")
    )
}

fn claude_content_to_blocks(content: Option<&Value>) -> Vec<Value> {
    let Some(content) = content else {
        return Vec::new();
    };
    match content {
        Value::String(text) => vec![json!({ "type": "text", "text": text })],
        Value::Array(items) => items
            .iter()
            .cloned()
            .map(|mut item| {
                normalize_text_block_in_place(&mut item);
                item
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_text_block_in_place(block: &mut Value) {
    let Some(object) = block.as_object_mut() else {
        return;
    };
    let block_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    if block_type != "text" {
        return;
    }
    let text_value = object.get("text");
    let new_text = text_value.and_then(extract_text_value);
    if let Some(new_text) = new_text {
        object.insert("text".to_string(), Value::String(new_text));
        return;
    }
    // If text exists but is not convertible, coerce to empty string to satisfy schema.
    if text_value.is_some() {
        object.insert("text".to_string(), Value::String(String::new()));
    }
}

fn extract_text_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.to_string()),
        Value::Object(object) => {
            if let Some(text) = object.get("text") {
                return extract_text_value(text);
            }
            if let Some(text) = object.get("value") {
                return extract_text_value(text);
            }
            None
        }
        _ => None,
    }
}

fn push_claude_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    let content = blocks;
    if content.is_empty() {
        return;
    }
    messages.push(json!({ "role": role, "content": content }));
}

fn push_tool_use_block(messages: &mut Vec<Value>, block: Value) {
    if let Some(last) = messages.last_mut().and_then(Value::as_object_mut) {
        if last.get("role").and_then(Value::as_str) == Some("assistant") {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.push(block);
                return;
            }
        }
    }
    messages.push(json!({ "role": "assistant", "content": [block] }));
}

fn push_tool_result_block(messages: &mut Vec<Value>, block: Value) {
    if let Some(last) = messages.last_mut().and_then(Value::as_object_mut) {
        if last.get("role").and_then(Value::as_str) == Some("user") {
            if let Some(content) = last.get_mut("content") {
                ensure_claude_content_array_in_place(content);
                if let Some(content) = content.as_array_mut() {
                    content.push(block);
                    return;
                }
            }
        }
    }
    messages.push(json!({ "role": "user", "content": [block] }));
}

fn ensure_claude_content_array_in_place(content: &mut Value) {
    if content.is_array() {
        return;
    }
    if let Some(text) = content.as_str() {
        *content = Value::Array(vec![json!({ "type": "text", "text": text })]);
        return;
    }
    *content = Value::Array(Vec::new());
}

fn map_anthropic_thinking_to_responses_reasoning(
    value: Option<&Value>,
    output_config: Option<&Value>,
) -> Option<Value> {
    let thinking = value?.as_object()?;
    let effort = match thinking.get("type").and_then(Value::as_str) {
        Some("enabled") => {
            let budget = thinking
                .get("budget_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if budget >= 10_000 {
                "high"
            } else if budget >= 5_000 {
                "medium"
            } else if budget >= 2_000 {
                "low"
            } else {
                "minimal"
            }
        }
        Some("adaptive") => output_config
            .and_then(Value::as_object)
            .and_then(|config| config.get("effort"))
            .and_then(Value::as_str)
            .unwrap_or("medium"),
        _ => return None,
    };

    let visibility = SummaryVisibility::from_anthropic_thinking(value);
    let mut reasoning = Map::new();
    reasoning.insert("effort".to_string(), json!(effort));
    if let Some(summary) = visibility.responses_summary() {
        reasoning.insert("summary".to_string(), summary);
    }
    tracing::debug!(?visibility, "mapped Anthropic reasoning summary visibility");
    Some(Value::Object(reasoning))
}

fn map_responses_reasoning_to_anthropic(value: Option<&Value>, out: &mut Map<String, Value>) {
    let Some(reasoning) = value.and_then(Value::as_object) else {
        return;
    };
    let visibility = SummaryVisibility::from_responses_reasoning(value);
    let effort = reasoning
        .get("effort")
        .and_then(Value::as_str)
        .and_then(normalize_anthropic_effort);
    if effort.is_none() && !visibility.is_specified() {
        return;
    }

    let mut thinking = Map::new();
    thinking.insert("type".to_string(), json!("adaptive"));
    if let Some(display) = visibility.anthropic_display() {
        thinking.insert("display".to_string(), display);
    }
    out.insert("thinking".to_string(), Value::Object(thinking));
    if let Some(effort) = effort {
        out.insert("output_config".to_string(), json!({ "effort": effort }));
    }
    tracing::debug!(
        ?visibility,
        effort,
        "mapped Responses reasoning to Anthropic"
    );
}

fn normalize_anthropic_effort(effort: &str) -> Option<&str> {
    match effort {
        "minimal" => Some("low"),
        "low" | "medium" | "high" | "xhigh" | "max" => Some(effort),
        _ => None,
    }
}

fn map_anthropic_output_format_to_responses_text(
    output_format: Option<&Value>,
    output_config: Option<&Value>,
) -> Option<Value> {
    let format = match output_format {
        Some(Value::Object(object)) => Some(object),
        _ => output_config
            .and_then(Value::as_object)
            .and_then(|config| config.get("format"))
            .and_then(Value::as_object),
    }?;

    if format.get("type").and_then(Value::as_str) != Some("json_schema") {
        return None;
    }
    let schema = format.get("schema")?;
    Some(json!({
        "format": {
            "type": "json_schema",
            "name": "structured_output",
            "schema": schema,
            "strict": true
        }
    }))
}

fn merge_responses_text_defaults(mut text: Value) -> Value {
    let Some(object) = text.as_object_mut() else {
        return json!({ "verbosity": "medium" });
    };
    object
        .entry("verbosity".to_string())
        .or_insert_with(|| Value::String("medium".to_string()));
    text
}

fn map_anthropic_context_management_to_responses(value: Option<&Value>) -> Option<Value> {
    let context_management = value?.as_object()?;
    let edits = context_management.get("edits")?.as_array()?;
    let mut mapped = Vec::new();
    for edit in edits {
        let Some(edit) = edit.as_object() else {
            continue;
        };
        if edit.get("type").and_then(Value::as_str) != Some("compact_20260112") {
            continue;
        }
        let mut item = Map::new();
        item.insert("type".to_string(), json!("compaction"));
        if let Some(value) = edit
            .get("trigger")
            .and_then(Value::as_object)
            .and_then(|trigger| trigger.get("value"))
            .and_then(Value::as_i64)
        {
            item.insert("compact_threshold".to_string(), json!(value));
        }
        mapped.push(Value::Object(item));
    }
    if mapped.is_empty() {
        None
    } else {
        Some(Value::Array(mapped))
    }
}

fn map_anthropic_metadata_to_responses_user(value: Option<&Value>) -> Option<String> {
    let metadata = value?.as_object()?;
    let user = metadata.get("user_id")?.as_str()?.trim();
    if user.is_empty() {
        None
    } else {
        Some(user.chars().take(64).collect())
    }
}
