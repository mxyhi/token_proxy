use axum::body::Bytes;
use serde_json::{json, Map, Value};

use super::media;
use super::tools;
use super::super::http_client::ProxyHttpClients;

pub(super) async fn responses_request_to_anthropic(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
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

    let stream = object.get("stream").and_then(Value::as_bool).unwrap_or(false);

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

    let input = object.get("input").ok_or_else(|| "Request must include input.".to_string())?;
    let mut messages = Vec::new();
    responses_input_to_claude_messages(input, &mut system_texts, &mut messages, http_clients).await?;

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

    if let Some(stop_sequences) = tools::map_openai_stop_to_anthropic_stop_sequences(object.get("stop")) {
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

    let stream = object.get("stream").and_then(Value::as_bool).unwrap_or(false);

    let max_output_tokens = object
        .get("max_tokens")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(4096);

    let mut input_items = Vec::new();

    let mut instructions_texts = Vec::new();
    if let Some(system) = object.get("system") {
        if let Some(text) = claude_system_to_text(system) {
            if !text.trim().is_empty() {
                instructions_texts.push(text);
            }
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
        Value::Number(max_output_tokens.into()),
    );
    out.insert("stream".to_string(), Value::Bool(stream));
    out.insert("input".to_string(), Value::Array(input_items));

    if let Some(instructions) = join_system_texts(instructions_texts) {
        out.insert("instructions".to_string(), Value::String(instructions));
    }

    if let Some(temperature) = object.get("temperature") {
        out.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = object.get("top_p") {
        out.insert("top_p".to_string(), top_p.clone());
    }

    if let Some(stop) = tools::map_anthropic_stop_sequences_to_openai_stop(object.get("stop_sequences")) {
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
        if role == "system" {
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
            if role == "system" {
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
        "function_call" => {
            let tool_use_id = object
                .get("call_id")
                .or_else(|| object.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("tool_use_proxy");
            let name = object.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = object.get("arguments").and_then(Value::as_str).unwrap_or("");
            let input = parse_tool_input_object(arguments);
            let block = json!({
                "type": "tool_use",
                "id": tool_use_id,
                "name": name,
                "input": input
            });
            push_tool_use_block(messages, block);
        }
        "function_call_output" => {
            let tool_use_id = object.get("call_id").and_then(Value::as_str).unwrap_or("");
            let output = object.get("output").and_then(Value::as_str).unwrap_or("");
            let block = json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output
            });
            push_tool_result_block(messages, block);
        }
        _ => {}
    }
    Ok(())
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
                        if let Some(block) = media::input_image_part_to_claude_block(part, http_clients).await? {
                            blocks.push(block);
                        }
                    }
                    "input_file" => {
                        if let Some(block) = media::input_file_part_to_claude_block(part, http_clients).await? {
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

fn claude_message_to_responses_input_items(message: &Value, input_items: &mut Vec<Value>) -> Result<(), String> {
    let Some(message) = message.as_object() else {
        return Ok(());
    };
    let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
    if role == "system" {
        return Ok(());
    }

    let content = message.get("content");
    let blocks = claude_content_to_blocks(content);

    let mut message_parts = Vec::new();
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
    if !message_parts.is_empty() {
        input_items.push(json!({
            "type": "message",
            "role": role,
            "content": message_parts
        }));
    }

    for block in blocks {
        let Some(block) = block.as_object() else {
            continue;
        };
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "tool_use" => {
                let call_id = block.get("id").and_then(Value::as_str).unwrap_or("call_proxy");
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                input_items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments
                }));
            }
            "tool_result" => {
                let call_id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                let output_raw = block.get("content").cloned().unwrap_or_else(|| json!(""));
                let output_text = match &output_raw {
                    Value::String(text) => text.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let mut item = Map::new();
                item.insert("type".to_string(), json!("function_call_output"));
                item.insert("call_id".to_string(), Value::String(call_id.to_string()));
                item.insert("output".to_string(), Value::String(output_text));
                if is_error {
                    item.insert("is_error".to_string(), Value::Bool(true));
                }
                if !matches!(output_raw, Value::String(_)) {
                    item.insert("output_parts".to_string(), output_raw);
                }
                input_items.push(Value::Object(item));
            }
            _ => {}
        }
    }

    Ok(())
}

fn claude_system_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.to_string()),
        Value::Array(items) => {
            let texts = items
                .iter()
                .filter_map(|item| item.as_object())
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .map(|text| text.to_string())
                .collect::<Vec<_>>();
            join_system_texts(texts)
        }
        _ => None,
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
            if combined.is_empty() { None } else { Some(combined) }
        }
        Value::Object(object) => object.get("text").and_then(Value::as_str).map(|t| t.to_string()),
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
