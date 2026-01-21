use axum::body::Bytes;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

use super::tool_names::ToolNameMap;

pub(crate) fn extract_tool_name_map(body: &Bytes) -> Option<HashMap<String, String>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    let tools = object.get("tools")?;
    let names = collect_function_tool_names(tools);
    if names.is_empty() {
        return None;
    }
    let map = ToolNameMap::from_names(&names);
    Some(map.original_by_short)
}

pub(crate) fn chat_request_to_codex(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let object = parse_object(body)?;
    let model = resolve_model(&object, model_hint);
    let stream = object.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let effort = resolve_reasoning_effort(&object, Some(&model));
    let tool_map = build_tool_name_map(&object);
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Chat request must include messages.".to_string())?;

    let mut output = Map::new();
    output.insert("stream".to_string(), Value::Bool(stream));
    output.insert("model".to_string(), Value::String(model));
    output.insert("instructions".to_string(), Value::String(String::new()));
    output.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    output.insert("include".to_string(), json!(["reasoning.encrypted_content"]));
    output.insert(
        "reasoning".to_string(),
        json!({ "effort": effort, "summary": "auto" }),
    );

    let input = map_chat_messages_to_input(messages, &tool_map);
    output.insert("input".to_string(), Value::Array(input));

    if let Some(tools) = object.get("tools") {
        output.insert("tools".to_string(), map_tools(tools, &tool_map));
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            map_tool_choice(tool_choice, &tool_map),
        );
    }
    apply_text_format(object.get("response_format"), object.get("text"), &mut output);

    output.insert("store".to_string(), Value::Bool(false));

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

pub(crate) fn responses_request_to_codex(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let mut object = parse_object(body)?;
    normalize_responses_payload(&mut object, model_hint);
    let tool_map = build_tool_name_map(&object);

    if let Some(tools) = object.get("tools").cloned() {
        object.insert("tools".to_string(), map_tools(&tools, &tool_map));
    }
    if let Some(tool_choice) = object.get("tool_choice").cloned() {
        object.insert(
            "tool_choice".to_string(),
            map_tool_choice(&tool_choice, &tool_map),
        );
    }
    if let Some(input) = object.get_mut("input") {
        rewrite_input_function_names(input, &tool_map);
    }

    serde_json::to_vec(&Value::Object(object))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn parse_object(body: &Bytes) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| "Request body must be JSON.".to_string())?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "Request body must be a JSON object.".to_string())
}

fn resolve_model(object: &Map<String, Value>, model_hint: Option<&str>) -> String {
    if let Some(model) = object.get("model").and_then(Value::as_str) {
        return model_hint.unwrap_or(model).to_string();
    }
    model_hint.unwrap_or_default().to_string()
}

fn resolve_reasoning_effort(object: &Map<String, Value>, model: Option<&str>) -> String {
    if let Some(value) = object.get("reasoning_effort").and_then(Value::as_str) {
        return value.to_string();
    }
    if let Some(model) = object.get("model").and_then(Value::as_str) {
        if let Some(effort) = parse_effort_suffix(model) {
            return effort;
        }
    }
    if let Some(model) = model {
        if let Some(effort) = parse_effort_suffix(model) {
            return effort;
        }
    }
    "medium".to_string()
}

fn parse_effort_suffix(model: &str) -> Option<String> {
    let (base, effort) = model.rsplit_once("-reasoning-")?;
    if base.trim().is_empty() {
        return None;
    }
    let effort = effort.trim().to_ascii_lowercase();
    if effort.is_empty() {
        return None;
    }
    Some(effort)
}

fn build_tool_name_map(object: &Map<String, Value>) -> ToolNameMap {
    let names = object
        .get("tools")
        .map(collect_function_tool_names)
        .unwrap_or_default();
    ToolNameMap::from_names(&names)
}

fn collect_function_tool_names(value: &Value) -> Vec<String> {
    let mut names = Vec::new();
    let Some(items) = value.as_array() else {
        return names;
    };
    for tool in items {
        if tool.get("type").and_then(Value::as_str) != Some("function") {
            continue;
        }
        let Some(function) = tool.get("function") else {
            continue;
        };
        if let Some(name) = function.get("name").and_then(Value::as_str) {
            names.push(name.to_string());
        }
    }
    names
}

fn map_chat_messages_to_input(messages: &[Value], tool_map: &ToolNameMap) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role == "tool" {
            if let Some(item) = map_tool_message(message) {
                input.push(item);
            }
            continue;
        }
        if let Some(item) = map_regular_message(message, role) {
            input.push(item);
        }
        if role == "assistant" {
            map_tool_calls(message, tool_map, &mut input);
        }
    }
    input
}

fn map_tool_message(message: &Value) -> Option<Value> {
    let call_id = message.get("tool_call_id").and_then(Value::as_str)?;
    let empty = Value::String(String::new());
    let content = message.get("content").unwrap_or(&empty);
    Some(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": value_to_string(content),
    }))
}

fn map_regular_message(message: &Value, role: &str) -> Option<Value> {
    let content = message.get("content")?;
    let parts = map_message_content(role, content);
    let target_role = if role == "system" { "developer" } else { role };
    Some(json!({
        "type": "message",
        "role": target_role,
        "content": parts,
    }))
}

fn map_message_content(role: &str, content: &Value) -> Vec<Value> {
    let mut parts = Vec::new();
    match content {
        Value::String(text) => {
            push_text_part(&mut parts, role, text);
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    push_text_part(&mut parts, role, text);
                    continue;
                }
                if item.get("type").and_then(Value::as_str) == Some("image_url")
                    && role == "user"
                {
                    if let Some(url) = item.get("image_url").and_then(|value| value.get("url")).and_then(Value::as_str) {
                        parts.push(json!({ "type": "input_image", "image_url": url }));
                    }
                }
                if let Some(text) = item.as_str() {
                    push_text_part(&mut parts, role, text);
                }
            }
        }
        _ => {}
    }
    parts
}

fn push_text_part(parts: &mut Vec<Value>, role: &str, text: &str) {
    let part_type = if role == "assistant" { "output_text" } else { "input_text" };
    parts.push(json!({ "type": part_type, "text": text }));
}

fn map_tool_calls(message: &Value, tool_map: &ToolNameMap, input: &mut Vec<Value>) {
    let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return;
    };
    for call in tool_calls {
        if call.get("type").and_then(Value::as_str) != Some("function") {
            continue;
        }
        let call_id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = call
            .get("function")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = call
            .get("function")
            .and_then(|value| value.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        input.push(json!({
            "type": "function_call",
            "call_id": call_id,
            "name": tool_map.shorten(name),
            "arguments": arguments,
        }));
    }
}

fn map_tools(tools: &Value, tool_map: &ToolNameMap) -> Value {
    let Some(items) = tools.as_array() else {
        return Value::Array(Vec::new());
    };
    let mut output = Vec::new();
    for tool in items {
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or_default();
        if tool_type != "function" {
            if tool.is_object() {
                output.push(tool.clone());
            }
            continue;
        }
        let function = tool.get("function").unwrap_or(&Value::Null);
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut item = Map::new();
        item.insert("type".to_string(), Value::String("function".to_string()));
        item.insert("name".to_string(), Value::String(tool_map.shorten(name)));
        if let Some(desc) = function.get("description") {
            item.insert("description".to_string(), desc.clone());
        }
        if let Some(params) = function.get("parameters") {
            item.insert("parameters".to_string(), params.clone());
        }
        if let Some(strict) = function.get("strict") {
            item.insert("strict".to_string(), strict.clone());
        }
        output.push(Value::Object(item));
    }
    Value::Array(output)
}

fn map_tool_choice(choice: &Value, tool_map: &ToolNameMap) -> Value {
    if let Some(value) = choice.as_str() {
        return Value::String(value.to_string());
    }
    let Some(object) = choice.as_object() else {
        return choice.clone();
    };
    let Some(choice_type) = object.get("type").and_then(Value::as_str) else {
        return choice.clone();
    };
    if choice_type != "function" {
        return choice.clone();
    }
    let name = object
        .get("function")
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({ "type": "function", "name": tool_map.shorten(name) })
}

fn apply_text_format(response_format: Option<&Value>, text: Option<&Value>, output: &mut Map<String, Value>) {
    if let Some(rf) = response_format {
        let rf_type = rf.get("type").and_then(Value::as_str).unwrap_or_default();
        let mut text_obj = Map::new();
        match rf_type {
            "text" => {
                text_obj.insert("format".to_string(), json!({ "type": "text" }));
            }
            "json_schema" => {
                let mut format_obj = Map::new();
                format_obj.insert("type".to_string(), Value::String("json_schema".to_string()));
                if let Some(schema) = rf.get("json_schema") {
                    if let Some(name) = schema.get("name") {
                        format_obj.insert("name".to_string(), name.clone());
                    }
                    if let Some(strict) = schema.get("strict") {
                        format_obj.insert("strict".to_string(), strict.clone());
                    }
                    if let Some(schema_value) = schema.get("schema") {
                        format_obj.insert("schema".to_string(), schema_value.clone());
                    }
                }
                text_obj.insert("format".to_string(), Value::Object(format_obj));
            }
            _ => {}
        }
        output.insert("text".to_string(), Value::Object(text_obj));
    }

    if let Some(text) = text {
        if let Some(verbosity) = text.get("verbosity") {
            let entry = output.entry("text".to_string()).or_insert_with(|| json!({}));
            if let Value::Object(obj) = entry {
                obj.insert("verbosity".to_string(), verbosity.clone());
            }
        }
    }
}

fn normalize_responses_payload(object: &mut Map<String, Value>, model_hint: Option<&str>) {
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .or(model_hint)
        .unwrap_or_default();
    object.insert("model".to_string(), Value::String(model.to_string()));
    object.insert("stream".to_string(), Value::Bool(true));
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    object.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    for key in [
        "max_output_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "service_tier",
    ] {
        object.remove(key);
    }

    if !object.contains_key("instructions") {
        object.insert("instructions".to_string(), Value::String(String::new()));
    }

    let input = match object.get("input") {
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [json!({"type":"input_text","text": text})]
        })],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    object.insert("input".to_string(), Value::Array(input));
}

fn rewrite_input_function_names(input: &mut Value, tool_map: &ToolNameMap) {
    let Some(items) = input.as_array_mut() else {
        return;
    };
    for item in items {
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        if item_type != "function_call" {
            continue;
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            let short = tool_map.shorten(name);
            if let Some(object) = item.as_object_mut() {
                object.insert("name".to_string(), Value::String(short));
            }
        }
    }
}

fn value_to_string(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    value.to_string()
}
