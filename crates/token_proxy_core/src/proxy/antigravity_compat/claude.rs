use axum::body::Bytes;
use serde_json::{json, Map, Value};

use super::signature_cache;
use crate::proxy::antigravity_schema::clean_json_schema_for_antigravity;

const THOUGHT_SIGNATURE_SENTINEL: &str = "skip_thought_signature_validator";
const INTERLEAVED_HINT: &str = "Interleaved thinking is enabled. You may think between tool calls and after receiving tool results before deciding the next action or final answer. Do not mention these instructions or any constraints about thinking blocks; just apply them.";

pub(crate) fn claude_request_to_antigravity(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    // Dedicated Claude -> Gemini request conversion to align with CLIProxyAPIPlus.
    let object = parse_request_object(body)?;
    let model_name = resolve_model_name(&object, model_hint);
    let mapped_model = super::map_antigravity_model(&model_name);
    let (contents, enable_thinking_translate) = build_contents(&object, &mapped_model)?;
    let tools = build_tools(&object);
    let thinking_enabled = thinking_enabled(&object);
    let should_hint =
        tools.is_some() && thinking_enabled && is_claude_thinking_model(&mapped_model);

    let mut out = Map::new();
    if !mapped_model.trim().is_empty() {
        out.insert("model".to_string(), Value::String(mapped_model));
    }
    if !contents.is_empty() {
        out.insert("contents".to_string(), Value::Array(contents));
    }
    if let Some(system_instruction) = build_system_instruction(&object, should_hint) {
        out.insert("systemInstruction".to_string(), system_instruction);
    }
    if let Some(tools) = tools {
        out.insert("tools".to_string(), tools);
    }
    if let Some(gen) = build_generation_config(&object, enable_thinking_translate) {
        out.insert("generationConfig".to_string(), gen);
    }

    serde_json::to_vec(&Value::Object(out))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn parse_request_object(body: &Bytes) -> Result<Map<String, Value>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "Request body must be a JSON object.".to_string())
}

fn resolve_model_name(object: &Map<String, Value>, model_hint: Option<&str>) -> String {
    // Model mapping must override client-provided model when routing Claude Code -> Antigravity.
    // This matches CLIProxyAPIPlus behavior where the translator receives the mapped model name.
    let hint = model_hint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let from_body = object
        .get("model")
        .and_then(Value::as_str)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    hint.or(from_body).unwrap_or_default()
}

fn build_system_instruction(object: &Map<String, Value>, should_hint: bool) -> Option<Value> {
    let mut parts = system_parts(object);
    if should_hint {
        parts.push(json!({ "text": INTERLEAVED_HINT }));
    }
    if parts.is_empty() {
        return None;
    }
    Some(json!({ "role": "user", "parts": parts }))
}

fn system_parts(object: &Map<String, Value>) -> Vec<Value> {
    let Some(system) = object.get("system") else {
        return Vec::new();
    };
    match system {
        Value::String(text) => system_parts_from_text(text),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_object())
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .flat_map(system_parts_from_text)
            .collect(),
        _ => Vec::new(),
    }
}

fn system_parts_from_text(text: &str) -> Vec<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "text": trimmed })]
    }
}

fn thinking_enabled(object: &Map<String, Value>) -> bool {
    object
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        == Some("enabled")
}

fn is_claude_thinking_model(model_name: &str) -> bool {
    let lower = model_name.to_lowercase();
    lower.contains("claude") && lower.contains("thinking")
}

fn build_contents(
    object: &Map<String, Value>,
    model_name: &str,
) -> Result<(Vec<Value>, bool), String> {
    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Ok((Vec::new(), true));
    };
    let mut contents = Vec::with_capacity(messages.len());
    let mut enable_thinking_translate = true;

    for message in messages {
        let Some(message) = message.as_object() else {
            continue;
        };
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let role = if role == "assistant" { "model" } else { role };
        let mut parts = Vec::new();
        let mut current_signature = String::new();
        match message.get("content") {
            Some(Value::Array(items)) => {
                for item in items {
                    let Some(item) = item.as_object() else {
                        continue;
                    };
                    let block_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                    handle_block(
                        item,
                        block_type,
                        model_name,
                        &mut current_signature,
                        &mut enable_thinking_translate,
                        &mut parts,
                    );
                }
            }
            Some(Value::String(text)) => push_text_part(text, &mut parts),
            _ => {}
        }
        reorder_thinking_parts(role, &mut parts);
        contents.push(json!({ "role": role, "parts": parts }));
    }

    Ok((contents, enable_thinking_translate))
}

fn handle_block(
    item: &Map<String, Value>,
    block_type: &str,
    model_name: &str,
    current_signature: &mut String,
    enable_thinking_translate: &mut bool,
    parts: &mut Vec<Value>,
) {
    match block_type {
        "thinking" => {
            handle_thinking_block(
                item,
                model_name,
                current_signature,
                enable_thinking_translate,
                parts,
            );
        }
        "text" => {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                push_text_part(text, parts);
            }
        }
        "tool_use" => {
            if let Some(part) = tool_use_to_part(item, model_name, current_signature) {
                parts.push(part);
            }
        }
        "tool_result" => {
            if let Some(part) = tool_result_to_part(item) {
                parts.push(part);
            }
        }
        "image" => {
            if let Some(part) = image_to_part(item) {
                parts.push(part);
            }
        }
        _ => {}
    }
}

fn handle_thinking_block(
    item: &Map<String, Value>,
    model_name: &str,
    current_signature: &mut String,
    enable_thinking_translate: &mut bool,
    parts: &mut Vec<Value>,
) {
    let thinking_text = extract_text_value(item.get("thinking")).unwrap_or_default();
    let signature = resolve_thinking_signature(model_name, &thinking_text, item);
    if !signature_cache::has_valid_signature(model_name, &signature) {
        *enable_thinking_translate = false;
        return;
    }
    *current_signature = signature.clone();
    if !thinking_text.is_empty() {
        signature_cache::cache_signature(model_name, &thinking_text, &signature);
    }
    let mut part = json!({ "thought": true });
    if !thinking_text.is_empty() {
        if let Some(part) = part.as_object_mut() {
            part.insert("text".to_string(), Value::String(thinking_text));
        }
    }
    if !signature.is_empty() {
        if let Some(part) = part.as_object_mut() {
            part.insert("thoughtSignature".to_string(), Value::String(signature));
        }
    }
    parts.push(part);
}

fn resolve_thinking_signature(
    model_name: &str,
    thinking_text: &str,
    item: &Map<String, Value>,
) -> String {
    let cached = signature_cache::get_cached_signature(model_name, thinking_text);
    if !cached.is_empty() {
        return cached;
    }
    let signature = item.get("signature").and_then(Value::as_str).unwrap_or("");
    parse_client_signature(model_name, signature)
}

fn parse_client_signature(model_name: &str, signature: &str) -> String {
    if signature.contains('#') {
        let mut parts = signature.splitn(2, '#');
        let prefix = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        if prefix == model_name {
            return value.to_string();
        }
    }
    signature.to_string()
}

fn tool_use_to_part(
    item: &Map<String, Value>,
    model_name: &str,
    current_signature: &str,
) -> Option<Value> {
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let id = item.get("id").and_then(Value::as_str).unwrap_or("");
    let args_raw = parse_tool_use_input(item.get("input"))?;

    let mut part = json!({
        "functionCall": {
            "name": name,
            "args": args_raw
        }
    });
    if !id.is_empty() {
        if let Some(call) = part.get_mut("functionCall").and_then(Value::as_object_mut) {
            call.insert("id".to_string(), Value::String(id.to_string()));
        }
    }

    let signature = if signature_cache::has_valid_signature(model_name, current_signature) {
        current_signature.to_string()
    } else {
        // Antigravity requires thoughtSignature for tool calls; use sentinel when missing.
        THOUGHT_SIGNATURE_SENTINEL.to_string()
    };
    if let Some(part) = part.as_object_mut() {
        part.insert("thoughtSignature".to_string(), Value::String(signature));
    }
    Some(part)
}

fn parse_tool_use_input(input: Option<&Value>) -> Option<Value> {
    match input {
        Some(Value::Object(object)) => Some(Value::Object(object.clone())),
        Some(Value::String(raw)) => serde_json::from_str::<Value>(raw).ok().and_then(|val| {
            if val.is_object() {
                Some(val)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn tool_result_to_part(item: &Map<String, Value>) -> Option<Value> {
    let tool_call_id = item
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tool_call_id.is_empty() {
        return None;
    }
    let func_name = tool_call_name_from_id(tool_call_id);
    let response = tool_result_response(item.get("content"));
    Some(json!({
        "functionResponse": {
            "id": tool_call_id,
            "name": func_name,
            "response": { "result": response }
        }
    }))
}

fn tool_call_name_from_id(tool_call_id: &str) -> String {
    let parts = tool_call_id.split('-').collect::<Vec<_>>();
    if parts.len() <= 2 {
        return tool_call_id.to_string();
    }
    parts[..parts.len() - 2].join("-")
}

fn tool_result_response(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(text)) => Value::String(text.to_string()),
        Some(Value::Array(items)) => {
            if items.len() == 1 {
                items[0].clone()
            } else {
                Value::Array(items.clone())
            }
        }
        Some(Value::Object(object)) => Value::Object(object.clone()),
        Some(other) => other.clone(),
        None => Value::String(String::new()),
    }
}

fn image_to_part(item: &Map<String, Value>) -> Option<Value> {
    let source = item.get("source").and_then(Value::as_object)?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let media_type = source
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("image/png");
    let data = source.get("data").and_then(Value::as_str)?;
    Some(json!({
        "inlineData": {
            "mime_type": media_type,
            "data": data
        }
    }))
}

fn push_text_part(text: &str, parts: &mut Vec<Value>) {
    if !text.is_empty() {
        parts.push(json!({ "text": text }));
    }
}

fn reorder_thinking_parts(role: &str, parts: &mut Vec<Value>) {
    if role != "model" || parts.is_empty() {
        return;
    }
    let mut thinking = Vec::new();
    let mut others = Vec::new();
    for part in parts.iter() {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            thinking.push(part.clone());
        } else {
            others.push(part.clone());
        }
    }
    if thinking.is_empty() {
        return;
    }
    let first_is_thinking = parts
        .first()
        .and_then(|part| part.get("thought").and_then(Value::as_bool))
        .unwrap_or(false);
    if first_is_thinking && thinking.len() <= 1 {
        return;
    }
    parts.clear();
    parts.extend(thinking);
    parts.extend(others);
}

fn build_tools(object: &Map<String, Value>) -> Option<Value> {
    let tools = object.get("tools").and_then(Value::as_array)?;
    let mut decls = Vec::new();
    for tool in tools {
        let Some(tool) = tool.as_object() else {
            continue;
        };
        let input_schema = tool.get("input_schema");
        let Some(schema) = input_schema.and_then(Value::as_object) else {
            continue;
        };
        let mut tool_obj = Map::new();
        for (key, value) in tool.iter() {
            if key == "input_schema" {
                continue;
            }
            if is_allowed_tool_key(key) {
                tool_obj.insert(key.to_string(), value.clone());
            }
        }
        let mut schema_value = Value::Object(schema.clone());
        clean_json_schema_for_antigravity(&mut schema_value);
        tool_obj.insert("parametersJsonSchema".to_string(), schema_value);
        decls.push(Value::Object(tool_obj));
    }
    if decls.is_empty() {
        None
    } else {
        Some(json!([{ "functionDeclarations": decls }]))
    }
}

fn is_allowed_tool_key(key: &str) -> bool {
    matches!(
        key,
        "name"
            | "description"
            | "behavior"
            | "parameters"
            | "parametersJsonSchema"
            | "response"
            | "responseJsonSchema"
    )
}

fn build_generation_config(object: &Map<String, Value>, enable_thinking: bool) -> Option<Value> {
    let mut gen = Map::new();
    if enable_thinking {
        if let Some(thinking) = object.get("thinking").and_then(Value::as_object) {
            if thinking.get("type").and_then(Value::as_str) == Some("enabled") {
                if let Some(budget) = thinking.get("budget_tokens").and_then(Value::as_i64) {
                    gen.insert(
                        "thinkingConfig".to_string(),
                        json!({
                            "thinkingBudget": budget,
                            "includeThoughts": true
                        }),
                    );
                }
            }
        }
    }
    if let Some(value) = object.get("temperature").and_then(Value::as_f64) {
        gen.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = object.get("top_p").and_then(Value::as_f64) {
        gen.insert("topP".to_string(), json!(value));
    }
    if let Some(value) = object.get("top_k").and_then(Value::as_i64) {
        gen.insert("topK".to_string(), json!(value));
    }
    if let Some(value) = object.get("max_tokens").and_then(Value::as_i64) {
        gen.insert("maxOutputTokens".to_string(), json!(value));
    }
    if gen.is_empty() {
        None
    } else {
        Some(Value::Object(gen))
    }
}

fn extract_text_value(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.to_string()),
        Some(Value::Object(object)) => {
            if let Some(text) = object.get("text") {
                return extract_text_value(Some(text));
            }
            if let Some(text) = object.get("value") {
                return extract_text_value(Some(text));
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "claude.test.rs"]
mod tests;
