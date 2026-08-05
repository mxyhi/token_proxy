use axum::body::Bytes;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

use super::{
    namespace::{collect_tool_identities, flatten_responses_namespaces},
    tool_names::ToolNameMap,
    RestoredToolName,
};
use crate::proxy::codex_tool_types::is_codex_tool_call_output_item_type;

const CODEX_DEFAULT_INSTRUCTIONS: &str = "You are Codex, based on GPT-5. You are running as a coding agent in the Codex CLI on a user's computer.";
const GPT5_1_DEFAULT_INSTRUCTIONS: &str =
    "You are GPT-5.1 running in the Codex CLI, a terminal-based coding assistant.";
const GPT5_2_DEFAULT_INSTRUCTIONS: &str =
    "You are GPT-5.2 running in the Codex CLI, a terminal-based coding assistant.";
const CODEX_CALL_ID_MAX_BYTES: usize = 64;
const CODEX_CALL_ID_PREFIX: &str = "fc_";
const CODEX_INPUT_ITEM_ID_MAX_CHARS: usize = 64;

fn normalize_codex_call_id(id: &str) -> String {
    let candidate = match id {
        "" => return String::new(),
        value if value.starts_with("fc") => Cow::Borrowed(value),
        value if value.starts_with("call_") => {
            Cow::Owned(format!("{CODEX_CALL_ID_PREFIX}{}", &value["call_".len()..]))
        }
        value => Cow::Owned(format!("{CODEX_CALL_ID_PREFIX}{value}")),
    };
    if candidate.len() <= CODEX_CALL_ID_MAX_BYTES {
        return id.to_string();
    }

    // 同一 canonical ID 生成相同摘要，使 call 与 output 在压缩后仍能配对。
    // sha2 0.11 的 Output 不再实现 LowerHex，手动格式化固定宽度 hex。
    let mut hasher = Sha256::new();
    hasher.update(b"token-proxy:codex-call-id:v1:");
    hasher.update(candidate.as_bytes());
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let suffix_bytes = CODEX_CALL_ID_MAX_BYTES - CODEX_CALL_ID_PREFIX.len();
    let normalized = format!("{CODEX_CALL_ID_PREFIX}{}", &digest[..suffix_bytes]);
    tracing::debug!(
        original_bytes = id.len(),
        normalized_bytes = normalized.len(),
        "compacted overlong Codex call_id"
    );
    normalized
}

fn normalize_codex_call_id_field(object: &mut Map<String, Value>) {
    let Some(call_id) = object.get("call_id").and_then(Value::as_str) else {
        return;
    };
    let normalized = normalize_codex_call_id(call_id);
    if normalized != call_id {
        object.insert("call_id".to_string(), Value::String(normalized));
    }
}

pub(crate) fn extract_tool_name_map(body: &Bytes) -> Option<HashMap<String, RestoredToolName>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    let tools = object.get("tools")?;
    let identities = collect_tool_identities(tools);
    if identities.is_empty() {
        return None;
    }
    let names = identities
        .iter()
        .map(|(flat_name, _)| flat_name.clone())
        .collect::<Vec<_>>();
    let map = ToolNameMap::from_names(&names);
    Some(
        identities
            .into_iter()
            .map(|(flat_name, identity)| (map.shorten(&flat_name), identity))
            .collect(),
    )
}

#[cfg(test)]
pub(crate) fn chat_request_to_codex(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    chat_request_to_codex_with_prompt_cache_key(body, model_hint, None)
}

pub(crate) fn chat_request_to_codex_with_prompt_cache_key(
    body: &Bytes,
    model_hint: Option<&str>,
    prompt_cache_key: Option<&str>,
) -> Result<Bytes, String> {
    let object = parse_object(body)?;
    if is_responses_shaped_chat_request(&object) {
        return transform_responses_request_to_codex(body, model_hint, false, prompt_cache_key);
    }

    let model = resolve_model(&object, model_hint);
    let effort = resolve_reasoning_effort(&object, Some(&model));
    let tool_map = build_tool_name_map(&object);
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Chat request must include messages.".to_string())?;

    let mut output = Map::new();
    output.insert("stream".to_string(), Value::Bool(true));
    output.insert("model".to_string(), Value::String(model.clone()));
    output.insert(
        "instructions".to_string(),
        Value::String(codex_base_instructions_for_model(&model).to_string()),
    );
    output.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    output.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
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
    apply_text_format(
        object.get("response_format"),
        object.get("text"),
        &mut output,
    );

    ensure_prompt_cache_key(&mut output, prompt_cache_key);
    output.insert("store".to_string(), Value::Bool(false));
    reject_codex_spark_non_text_features(&model, &output)?;

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn is_responses_shaped_chat_request(object: &Map<String, Value>) -> bool {
    !object.contains_key("messages") && object.contains_key("input")
}

#[cfg(test)]
pub(crate) fn responses_request_to_codex(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    responses_request_to_codex_with_prompt_cache_key(body, model_hint, None)
}

pub(crate) fn responses_request_to_codex_with_prompt_cache_key(
    body: &Bytes,
    model_hint: Option<&str>,
    prompt_cache_key: Option<&str>,
) -> Result<Bytes, String> {
    transform_responses_request_to_codex(body, model_hint, false, prompt_cache_key)
}

#[cfg(test)]
pub(crate) fn responses_compact_request_to_codex(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    responses_compact_request_to_codex_with_prompt_cache_key(body, model_hint, None)
}

pub(crate) fn responses_compact_request_to_codex_with_prompt_cache_key(
    body: &Bytes,
    model_hint: Option<&str>,
    prompt_cache_key: Option<&str>,
) -> Result<Bytes, String> {
    transform_responses_request_to_codex(body, model_hint, true, prompt_cache_key)
}

fn transform_responses_request_to_codex(
    body: &Bytes,
    model_hint: Option<&str>,
    strip_reasoning_include: bool,
    prompt_cache_key: Option<&str>,
) -> Result<Bytes, String> {
    let mut object = parse_object(body)?;
    normalize_responses_payload(
        &mut object,
        model_hint,
        strip_reasoning_include,
        prompt_cache_key,
    );
    flatten_responses_namespaces(&mut object)?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    reject_codex_spark_non_text_features(model, &object)?;
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
    normalize_tool_choice_for_codex(&mut object);
    if let Some(input) = object.get_mut("input") {
        normalize_input_message_text(input);
        add_missing_tool_call_names(input);
        rewrite_input_function_names(input, &tool_map);
    }

    serde_json::to_vec(&Value::Object(object))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn parse_object(body: &Bytes) -> Result<Map<String, Value>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "Request body must be a JSON object.".to_string())
}

fn resolve_model(object: &Map<String, Value>, model_hint: Option<&str>) -> String {
    if let Some(model) = object.get("model").and_then(Value::as_str) {
        return normalize_codex_model(model_hint.unwrap_or(model));
    }
    normalize_codex_model(model_hint.unwrap_or_default())
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
    if let Some((base, effort)) = model.rsplit_once("-reasoning-") {
        if base.trim().is_empty() {
            return None;
        }
        let effort = effort.trim().to_ascii_lowercase();
        return (!effort.is_empty()).then_some(effort);
    }

    parse_gpt_5_6_effort_suffix(model)
}

fn parse_gpt_5_6_effort_suffix(model: &str) -> Option<String> {
    let model = model.trim().rsplit('/').next()?.trim().to_ascii_lowercase();
    for prefix in [
        "gpt-5.6-sol-",
        "gpt-5.6-terra-",
        "gpt-5.6-luna-",
        "gpt-5.6-",
    ] {
        let Some(effort) = model.strip_prefix(prefix) else {
            continue;
        };
        if matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        ) {
            return Some(effort.to_string());
        }
    }
    None
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
        let name = tool
            .get("function")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .or_else(|| tool.get("name").and_then(Value::as_str));
        if let Some(name) = name {
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names
}

fn map_chat_messages_to_input(messages: &[Value], tool_map: &ToolNameMap) -> Vec<Value> {
    let mut input = Vec::new();
    let mut pending_calls = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let Some(role) = message.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role == "tool" {
            if let Some(item) = map_tool_message(message, &mut pending_calls) {
                input.push(item);
            }
            continue;
        }
        pending_calls.clear();
        if let Some(item) = map_regular_message(message, role) {
            input.push(item);
        }
        if role == "assistant" {
            pending_calls = map_tool_calls(message, message_index, tool_map, &mut input);
        }
    }
    input
}

struct PendingToolCall {
    call_id: String,
    source_call_id: String,
    output_type: &'static str,
    consumed: bool,
}

fn map_tool_message(message: &Value, pending_calls: &mut [PendingToolCall]) -> Option<Value> {
    let source_call_id = message
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pending = pending_calls.iter_mut().find(|pending| {
        !pending.consumed
            && (source_call_id.is_empty()
                || pending.source_call_id == source_call_id
                || pending.call_id == source_call_id)
    })?;
    pending.consumed = true;
    let empty = Value::String(String::new());
    let content = message.get("content").unwrap_or(&empty);
    Some(json!({
        "type": pending.output_type,
        "call_id": pending.call_id,
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
                if let Some(text) = item.get("text") {
                    push_text_part(&mut parts, role, &value_to_string(text));
                    continue;
                }
                if item.get("type").and_then(Value::as_str) == Some("image_url") && role == "user" {
                    if let Some(url) = item
                        .get("image_url")
                        .and_then(|value| value.get("url"))
                        .and_then(Value::as_str)
                    {
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
    let part_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    parts.push(json!({ "type": part_type, "text": text }));
}

fn map_tool_calls(
    message: &Value,
    message_index: usize,
    tool_map: &ToolNameMap,
    input: &mut Vec<Value>,
) -> Vec<PendingToolCall> {
    let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut source_id_counts = HashMap::new();
    let mut used_call_ids = HashSet::new();
    for call in tool_calls {
        if !matches!(
            call.get("type").and_then(Value::as_str),
            Some("function" | "custom")
        ) {
            continue;
        }
        let source_id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        if !source_id.is_empty() {
            *source_id_counts
                .entry(source_id.to_string())
                .or_insert(0usize) += 1;
            used_call_ids.insert(normalize_codex_call_id(source_id));
        }
    }
    let ambiguous_ids = source_id_counts
        .into_iter()
        .filter_map(|(id, count)| (count > 1).then_some(id))
        .collect::<HashSet<_>>();
    if !ambiguous_ids.is_empty() {
        tracing::warn!(
            duplicate_id_count = ambiguous_ids.len(),
            "dropped ambiguous Codex tool calls with duplicate ids"
        );
    }

    let mut pending = Vec::new();
    for (call_index, call) in tool_calls.iter().enumerate() {
        let Some(call_type @ ("function" | "custom")) = call.get("type").and_then(Value::as_str)
        else {
            continue;
        };
        let source_call_id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        if !source_call_id.is_empty() && ambiguous_ids.contains(source_call_id) {
            continue;
        }
        let call_id = if source_call_id.is_empty() {
            let base = format!("call_missing_{message_index}_{call_index}");
            let mut candidate = base.clone();
            let mut suffix = 1usize;
            while used_call_ids.contains(&candidate) {
                candidate = format!("{base}_{suffix}");
                suffix += 1;
            }
            used_call_ids.insert(candidate.clone());
            tracing::debug!(
                message_index,
                call_index,
                "synthesized missing Codex tool call id"
            );
            candidate
        } else {
            normalize_codex_call_id(source_call_id)
        };

        let (item, output_type) = if call_type == "custom" {
            let custom = call.get("custom").unwrap_or(&Value::Null);
            let name = custom
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let custom_input = custom.get("input").map(value_to_string).unwrap_or_default();
            (
                json!({
                    "type": "custom_tool_call",
                    "call_id": call_id,
                    "name": tool_map.shorten(name),
                    "input": custom_input,
                }),
                "custom_tool_call_output",
            )
        } else {
            let function = call.get("function").unwrap_or(&Value::Null);
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (
                json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": tool_map.shorten(name),
                    "arguments": arguments,
                }),
                "function_call_output",
            )
        };
        input.push(item);
        pending.push(PendingToolCall {
            call_id,
            source_call_id: source_call_id.to_string(),
            output_type,
            consumed: false,
        });
    }
    pending
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
        let mut item = Map::new();
        item.insert("type".to_string(), Value::String("function".to_string()));
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| tool.get("name").and_then(Value::as_str));
        if let Some(name) = name {
            if !name.is_empty() {
                item.insert("name".to_string(), Value::String(tool_map.shorten(name)));
            }
        }
        if let Some(desc) = function
            .get("description")
            .or_else(|| tool.get("description"))
        {
            item.insert("description".to_string(), desc.clone());
        }
        if let Some(params) = function
            .get("parameters")
            .or_else(|| tool.get("parameters"))
        {
            item.insert("parameters".to_string(), params.clone());
        }
        if let Some(strict) = function.get("strict").or_else(|| tool.get("strict")) {
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
        .or_else(|| object.get("name").and_then(Value::as_str));
    let mut output = Map::new();
    output.insert("type".to_string(), Value::String("function".to_string()));
    if let Some(name) = name {
        if !name.is_empty() {
            output.insert("name".to_string(), Value::String(tool_map.shorten(name)));
        }
    }
    Value::Object(output)
}

fn normalize_tool_choice_for_codex(object: &mut Map<String, Value>) {
    let Some(choice_type) = object
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|choice| choice.get("type"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let choice_type = choice_type.trim();
    if choice_type.is_empty() || codex_tools_contain_type(object.get("tools"), choice_type) {
        return;
    }
    object.insert("tool_choice".to_string(), Value::String("auto".to_string()));
}

fn codex_tools_contain_type(tools: Option<&Value>, tool_type: &str) -> bool {
    let Some(items) = tools.and_then(Value::as_array) else {
        return false;
    };
    items
        .iter()
        .filter_map(Value::as_object)
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some(tool_type))
}

fn apply_text_format(
    response_format: Option<&Value>,
    text: Option<&Value>,
    output: &mut Map<String, Value>,
) {
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
            let entry = output
                .entry("text".to_string())
                .or_insert_with(|| json!({}));
            if let Value::Object(obj) = entry {
                obj.insert("verbosity".to_string(), verbosity.clone());
            }
        }
    }
}

fn normalize_responses_payload(
    object: &mut Map<String, Value>,
    model_hint: Option<&str>,
    strip_reasoning_include: bool,
    prompt_cache_key: Option<&str>,
) {
    let requested_model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|_| model_hint.is_none())
        .or(model_hint)
        .unwrap_or_default();
    let inferred_effort = parse_gpt_5_6_effort_suffix(requested_model);
    let model = normalize_codex_model(requested_model);
    object.insert("model".to_string(), Value::String(model.clone()));
    normalize_gpt_5_6_reasoning_effort(
        object,
        &model,
        inferred_effort.as_deref(),
        strip_reasoning_include,
    );
    if !object.contains_key("parallel_tool_calls") {
        object.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    }
    object.insert("stream".to_string(), Value::Bool(true));
    object.insert("store".to_string(), Value::Bool(false));
    if strip_reasoning_include {
        object.remove("include");
    } else {
        object.insert(
            "include".to_string(),
            json!(["reasoning.encrypted_content"]),
        );
    }
    for key in [
        "max_output_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "service_tier",
        "previous_response_id",
        "prompt_cache_retention",
        "safety_identifier",
        "metadata",
        "stream_options",
    ] {
        object.remove(key);
    }

    let input = match object.get("input") {
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [json!({"type":"input_text","text": text})]
        })],
        Some(Value::Array(items)) => sanitize_responses_input_for_codex(items),
        _ => Vec::new(),
    };
    let (input, extracted_instructions) = extract_system_messages_from_input(input);
    merge_extracted_instructions(object, extracted_instructions);
    ensure_default_instructions(object, &model);
    ensure_prompt_cache_key(object, prompt_cache_key);
    object.insert("input".to_string(), Value::Array(input));
}

fn normalize_gpt_5_6_reasoning_effort(
    object: &mut Map<String, Value>,
    model: &str,
    inferred_effort: Option<&str>,
    is_compact: bool,
) {
    if !matches!(model, "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna") {
        return;
    }

    let explicit_effort = object
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(effort) = explicit_effort
        .as_deref()
        .or(inferred_effort)
        .map(str::to_string)
    else {
        return;
    };
    let normalized_effort = if is_compact && effort.trim().eq_ignore_ascii_case("max") {
        "xhigh"
    } else {
        effort.as_str()
    };
    if explicit_effort.as_deref() == Some(normalized_effort) {
        return;
    }

    let reasoning = object
        .entry("reasoning".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(reasoning) = reasoning.as_object_mut() else {
        return;
    };
    reasoning.insert(
        "effort".to_string(),
        Value::String(normalized_effort.to_string()),
    );
    tracing::debug!(
        model,
        requested_effort = effort.as_str(),
        normalized_effort,
        is_compact,
        "normalized GPT-5.6 reasoning effort"
    );
}

fn ensure_prompt_cache_key(object: &mut Map<String, Value>, prompt_cache_key: Option<&str>) {
    let existing = object
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if existing.is_some() {
        return;
    }
    if let Some(prompt_cache_key) = prompt_cache_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        object.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.to_string()),
        );
        return;
    }
    // Codex upstream expects this field even when a non-Codex client did not send a thread id.
    object.insert(
        "prompt_cache_key".to_string(),
        Value::String(crate::proxy::kiro::utils::random_uuid()),
    );
}

fn normalize_codex_model(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        return "gpt-5.4".to_string();
    }
    let model_id = model.rsplit('/').next().unwrap_or(model).trim();
    let normalized = model_id.to_ascii_lowercase();
    let compact = normalized.replace(' ', "-");

    for (alias, target) in CODEX_MODEL_ALIASES {
        if compact == *alias {
            return (*target).to_string();
        }
    }

    if compact.contains("gpt-5.6-sol") {
        return "gpt-5.6-sol".to_string();
    }
    if compact.contains("gpt-5.6-terra") {
        return "gpt-5.6-terra".to_string();
    }
    if compact.contains("gpt-5.6-luna") {
        return "gpt-5.6-luna".to_string();
    }
    if compact.contains("gpt-5.5-pro") || compact.contains("gpt5.5-pro") {
        return "gpt-5.5-pro".to_string();
    }
    if compact.contains("gpt-5.5") {
        return "gpt-5.5".to_string();
    }
    if compact.contains("gpt-5.4-mini") {
        return "gpt-5.4-mini".to_string();
    }
    if compact.contains("gpt-5.4") {
        return "gpt-5.4".to_string();
    }
    if compact.contains("gpt-5.2") {
        return "gpt-5.2".to_string();
    }
    if compact.contains("gpt-5.3-codex-spark") {
        return "gpt-5.3-codex-spark".to_string();
    }
    if compact.contains("gpt-5.3-codex") || compact.contains("gpt-5.3") {
        return "gpt-5.3-codex".to_string();
    }

    model_id.to_string()
}

fn reject_codex_spark_non_text_features(
    model: &str,
    object: &Map<String, Value>,
) -> Result<(), String> {
    if model != "gpt-5.3-codex-spark" {
        return Ok(());
    }
    if value_contains_image_input(object.get("input")) {
        return Err(
            "gpt-5.3-codex-spark is text-only and does not support image inputs.".to_string(),
        );
    }
    if value_contains_image_generation_tool(object.get("tools"))
        || input_contains_additional_image_generation_tool(object.get("input"))
        || tool_choice_selects_image_generation(object.get("tool_choice"))
    {
        return Err(
            "gpt-5.3-codex-spark is text-only and does not support image generation tools."
                .to_string(),
        );
    }
    Ok(())
}

fn value_contains_image_input(value: Option<&Value>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value {
        Value::Array(items) => items
            .iter()
            .any(|item| value_contains_image_input(Some(item))),
        Value::Object(object) => {
            let item_type = object.get("type").and_then(Value::as_str);
            matches!(item_type, Some("input_image" | "image_url"))
                || object
                    .values()
                    .any(|item| value_contains_image_input(Some(item)))
        }
        _ => false,
    }
}

fn value_contains_image_generation_tool(value: Option<&Value>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value {
        Value::Array(items) => items
            .iter()
            .any(|item| value_contains_image_generation_tool(Some(item))),
        Value::Object(object) => {
            let item_type = object.get("type").and_then(Value::as_str);
            matches!(
                item_type,
                Some("image_generation" | "image_generation_call")
            ) || object
                .values()
                .any(|item| value_contains_image_generation_tool(Some(item)))
        }
        _ => false,
    }
}

fn tool_choice_selects_image_generation(choice: Option<&Value>) -> bool {
    let Some(choice) = choice else {
        return false;
    };
    if choice
        .as_str()
        .is_some_and(|value| matches!(value, "image_generation" | "image_generation_call"))
    {
        return true;
    }
    let Some(object) = choice.as_object() else {
        return false;
    };
    let choice_type = object.get("type").and_then(Value::as_str);
    if matches!(
        choice_type,
        Some("image_generation" | "image_generation_call")
    ) {
        return true;
    }
    if choice_type == Some("namespace")
        && matches!(
            object
                .get("name")
                .or_else(|| object.get("namespace"))
                .and_then(Value::as_str),
            Some("image_gen")
        )
    {
        return true;
    }
    object
        .get("tool")
        .is_some_and(|tool| tool_choice_selects_image_generation(Some(tool)))
        || object
            .get("function")
            .is_some_and(|function| tool_choice_selects_image_generation(Some(function)))
        || (object.get("namespace").and_then(Value::as_str) == Some("image_gen")
            && matches!(
                object.get("name").and_then(Value::as_str),
                Some("imagegen" | "image_gen.imagegen" | "image_gen__imagegen")
            ))
}

fn input_contains_additional_image_generation_tool(input: Option<&Value>) -> bool {
    let Some(items) = input.and_then(Value::as_array) else {
        return false;
    };
    items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("additional_tools")
            && value_contains_image_generation_tool(item.get("tools"))
    })
}

const CODEX_MODEL_ALIASES: &[(&str, &str)] = &[
    ("gpt-5.6", "gpt-5.6-sol"),
    ("gpt-5.6-none", "gpt-5.6-sol"),
    ("gpt-5.6-minimal", "gpt-5.6-sol"),
    ("gpt-5.6-low", "gpt-5.6-sol"),
    ("gpt-5.6-medium", "gpt-5.6-sol"),
    ("gpt-5.6-high", "gpt-5.6-sol"),
    ("gpt-5.6-xhigh", "gpt-5.6-sol"),
    ("gpt-5.6-max", "gpt-5.6-sol"),
    ("gpt-5.6-sol", "gpt-5.6-sol"),
    ("gpt-5.6-sol-none", "gpt-5.6-sol"),
    ("gpt-5.6-sol-low", "gpt-5.6-sol"),
    ("gpt-5.6-sol-medium", "gpt-5.6-sol"),
    ("gpt-5.6-sol-high", "gpt-5.6-sol"),
    ("gpt-5.6-sol-xhigh", "gpt-5.6-sol"),
    ("gpt-5.6-terra", "gpt-5.6-terra"),
    ("gpt-5.6-terra-none", "gpt-5.6-terra"),
    ("gpt-5.6-terra-low", "gpt-5.6-terra"),
    ("gpt-5.6-terra-medium", "gpt-5.6-terra"),
    ("gpt-5.6-terra-high", "gpt-5.6-terra"),
    ("gpt-5.6-terra-xhigh", "gpt-5.6-terra"),
    ("gpt-5.6-luna", "gpt-5.6-luna"),
    ("gpt-5.6-luna-none", "gpt-5.6-luna"),
    ("gpt-5.6-luna-low", "gpt-5.6-luna"),
    ("gpt-5.6-luna-medium", "gpt-5.6-luna"),
    ("gpt-5.6-luna-high", "gpt-5.6-luna"),
    ("gpt-5.6-luna-xhigh", "gpt-5.6-luna"),
    ("gpt-5.5-pro", "gpt-5.5-pro"),
    ("gpt-5.5-pro-none", "gpt-5.5-pro"),
    ("gpt-5.5-pro-low", "gpt-5.5-pro"),
    ("gpt-5.5-pro-medium", "gpt-5.5-pro"),
    ("gpt-5.5-pro-high", "gpt-5.5-pro"),
    ("gpt-5.5-pro-xhigh", "gpt-5.5-pro"),
    ("gpt-5.5", "gpt-5.5"),
    ("gpt-5.5-none", "gpt-5.5"),
    ("gpt-5.5-low", "gpt-5.5"),
    ("gpt-5.5-medium", "gpt-5.5"),
    ("gpt-5.5-high", "gpt-5.5"),
    ("gpt-5.5-xhigh", "gpt-5.5"),
    ("gpt-5-codex", "gpt-5-codex"),
    ("gpt-5.4", "gpt-5.4"),
    ("gpt-5.4-none", "gpt-5.4"),
    ("gpt-5.4-low", "gpt-5.4"),
    ("gpt-5.4-medium", "gpt-5.4"),
    ("gpt-5.4-high", "gpt-5.4"),
    ("gpt-5.4-xhigh", "gpt-5.4"),
    ("gpt-5.4-chat-latest", "gpt-5.4"),
    ("gpt-5.4-mini", "gpt-5.4-mini"),
    ("gpt-5.3", "gpt-5.3-codex"),
    ("gpt-5.3-none", "gpt-5.3-codex"),
    ("gpt-5.3-low", "gpt-5.3-codex"),
    ("gpt-5.3-medium", "gpt-5.3-codex"),
    ("gpt-5.3-high", "gpt-5.3-codex"),
    ("gpt-5.3-xhigh", "gpt-5.3-codex"),
    ("gpt-5.3-codex", "gpt-5.3-codex"),
    ("gpt-5.3-codex-low", "gpt-5.3-codex"),
    ("gpt-5.3-codex-medium", "gpt-5.3-codex"),
    ("gpt-5.3-codex-high", "gpt-5.3-codex"),
    ("gpt-5.3-codex-xhigh", "gpt-5.3-codex"),
    ("gpt-5.3-codex-spark", "gpt-5.3-codex-spark"),
    ("gpt-5.3-codex-spark-low", "gpt-5.3-codex-spark"),
    ("gpt-5.3-codex-spark-medium", "gpt-5.3-codex-spark"),
    ("gpt-5.3-codex-spark-high", "gpt-5.3-codex-spark"),
    ("gpt-5.3-codex-spark-xhigh", "gpt-5.3-codex-spark"),
    ("gpt-5.2", "gpt-5.2"),
    ("gpt-5.2-none", "gpt-5.2"),
    ("gpt-5.2-low", "gpt-5.2"),
    ("gpt-5.2-medium", "gpt-5.2"),
    ("gpt-5.2-high", "gpt-5.2"),
    ("gpt-5.2-xhigh", "gpt-5.2"),
];

pub(crate) fn supported_codex_model_ids() -> Vec<String> {
    let mut models = Vec::new();
    for (alias, target) in CODEX_MODEL_ALIASES {
        push_unique_codex_model_id(&mut models, alias);
        push_unique_codex_model_id(&mut models, target);
    }
    models
}

fn push_unique_codex_model_id(models: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && models.iter().all(|model| model != value) {
        models.push(value.to_string());
    }
}

fn extract_system_messages_from_input(items: Vec<Value>) -> (Vec<Value>, Vec<String>) {
    let mut output = Vec::with_capacity(items.len());
    let mut instructions = Vec::new();

    for item in items {
        let mut item = item;
        let Some(object) = item.as_object_mut() else {
            output.push(item);
            continue;
        };
        let role = object.get("role").and_then(Value::as_str);
        if !matches!(role, Some("system" | "developer")) {
            output.push(item);
            continue;
        }
        if role == Some("system") {
            object.insert("role".to_string(), Value::String("developer".to_string()));
        }
        if let Some(text) = extract_text_from_content(object.get("content")) {
            instructions.push(text);
        }
        output.push(item);
    }

    (output, instructions)
}

fn merge_extracted_instructions(object: &mut Map<String, Value>, extracted: Vec<String>) {
    if extracted.is_empty() {
        return;
    }
    let extracted = extracted.join("\n\n");
    let existing = object
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let instructions = match existing {
        Some(existing) => format!("{extracted}\n\n{existing}"),
        None => extracted,
    };
    object.insert("instructions".to_string(), Value::String(instructions));
}

fn ensure_default_instructions(object: &mut Map<String, Value>, model: &str) {
    let has_instructions = object
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if !has_instructions {
        object.insert(
            "instructions".to_string(),
            Value::String(codex_base_instructions_for_model(model).to_string()),
        );
    }
}

fn codex_base_instructions_for_model(model: &str) -> &'static str {
    let model = model.trim().to_ascii_lowercase();
    if model.contains("codex") {
        return CODEX_DEFAULT_INSTRUCTIONS;
    }
    if model.starts_with("gpt-5.2") {
        return GPT5_2_DEFAULT_INSTRUCTIONS;
    }
    if model.starts_with("gpt-5.1") || model.starts_with("gpt-5") {
        return GPT5_1_DEFAULT_INSTRUCTIONS;
    }
    CODEX_DEFAULT_INSTRUCTIONS
}

fn extract_text_from_content(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => non_empty_text(text),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| item.as_str())
                })
                .filter_map(non_empty_text)
                .collect::<Vec<_>>()
                .join("\n\n");
            non_empty_text(&text)
        }
        _ => None,
    }
}

fn non_empty_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn sanitize_responses_input_for_codex(items: &[Value]) -> Vec<Value> {
    let mut sanitized = items
        .iter()
        .map(sanitize_responses_input_item_for_codex)
        .collect::<Vec<_>>();
    normalize_codex_input_item_ids(&mut sanitized);
    sanitized
}

// Codex caps every retained input item id, not only function call ids. Keep a
// request-wide map so repeated source ids stay paired and cannot collide.
fn normalize_codex_input_item_ids(items: &mut [Value]) {
    let mut occupied = items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            let normalized =
                normalize_codex_input_item_id(item.get("type").and_then(Value::as_str), id);
            (normalized.chars().count() <= CODEX_INPUT_ITEM_ID_MAX_CHARS).then_some(normalized)
        })
        .collect::<HashSet<_>>();
    let mut mapped = HashMap::new();
    let mut changed = 0usize;

    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let Some(source_id) = object.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let id =
            normalize_codex_input_item_id(object.get("type").and_then(Value::as_str), &source_id);

        let normalized_id = if id.chars().count() > CODEX_INPUT_ITEM_ID_MAX_CHARS {
            mapped
                .entry(id.clone())
                .or_insert_with(|| {
                    let mut attempt = 0usize;
                    loop {
                        let candidate = shorten_codex_input_item_id(&id, attempt);
                        if occupied.insert(candidate.clone()) {
                            break candidate;
                        }
                        attempt += 1;
                    }
                })
                .clone()
        } else {
            id
        };
        if object.get("id").and_then(Value::as_str) != Some(normalized_id.as_str()) {
            object.insert("id".to_string(), Value::String(normalized_id));
            changed += 1;
        }
    }

    if changed > 0 {
        tracing::debug!(changed, "normalized Codex input item ids");
    }
}

fn normalize_codex_input_item_id(item_type: Option<&str>, id: &str) -> String {
    if item_type == Some("message") && !id.is_empty() && !id.starts_with("msg") {
        return format!("msg_{id}");
    }
    id.to_string()
}

fn shorten_codex_input_item_id(id: &str, attempt: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"token-proxy:codex-input-item-id:v1:");
    hasher.update(id.as_bytes());
    hasher.update(attempt.to_be_bytes());
    let digest: String = hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let suffix = format!("_{digest}");
    let prefix_chars = CODEX_INPUT_ITEM_ID_MAX_CHARS - suffix.chars().count();
    format!(
        "{}{}",
        id.chars().take(prefix_chars).collect::<String>(),
        suffix
    )
}

fn sanitize_responses_input_item_for_codex(item: &Value) -> Value {
    let Some(object) = item.as_object() else {
        return item.clone();
    };
    if object.get("role").and_then(Value::as_str) == Some("tool") {
        return map_responses_tool_role_message(object);
    }
    if object.get("type").is_none()
        && object.get("role").is_some()
        && object.get("content").is_some()
    {
        let role = object.get("role").and_then(Value::as_str).unwrap_or("user");
        return map_regular_message(item, role).unwrap_or_else(|| item.clone());
    }
    let item_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    if item_type == "reasoning" {
        let mut sanitized = object.clone();
        // store=false 时 rs_* 只能作为回放内容，携带旧 id 会触发上游对象查询并返回 404。
        if sanitized
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("rs_"))
        {
            sanitized.remove("id");
        }
        if matches!(sanitized.get("summary"), None | Some(Value::Null)) {
            sanitized.insert("summary".to_string(), Value::Array(Vec::new()));
        }
        return Value::Object(sanitized);
    }
    if is_codex_call_input_item_type(item_type) {
        let mut sanitized = object.clone();
        // Codex 只接受 fc* call-input id；call_id 负责和 output 配对，必须保留。
        if sanitized
            .get("id")
            .is_some_and(|id| !id.as_str().is_some_and(|id| id.starts_with("fc")))
        {
            sanitized.remove("id");
        }
        normalize_codex_call_id_field(&mut sanitized);
        return Value::Object(sanitized);
    }
    if !is_codex_tool_call_output_item_type(item_type) {
        return item.clone();
    }
    let mut sanitized = object.clone();
    normalize_codex_call_id_field(&mut sanitized);
    // Claude -> Responses may carry structured tool output in `output_parts`.
    // Codex only needs the flattened `output` string here; forwarding the extra field
    // breaks composition without adding value.
    sanitized.remove("output_parts");
    Value::Object(sanitized)
}

fn map_responses_tool_role_message(object: &Map<String, Value>) -> Value {
    let call_id = object
        .get("call_id")
        .and_then(Value::as_str)
        .or_else(|| object.get("tool_call_id").and_then(Value::as_str))
        .or_else(|| object.get("id").and_then(Value::as_str))
        .unwrap_or_default()
        .trim();
    if call_id.is_empty() {
        let mut fallback = object.clone();
        fallback.insert("role".to_string(), Value::String("user".to_string()));
        fallback.remove("tool_call_id");
        return Value::Object(fallback);
    }
    let call_id = normalize_codex_call_id(call_id);

    let output = extract_text_from_content(object.get("content"))
        .or_else(|| object.get("output").map(value_to_string))
        .unwrap_or_default();
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output,
    })
}

fn normalize_input_message_text(input: &mut Value) {
    let Some(items) = input.as_array_mut() else {
        return;
    };
    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        if object.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(parts) = object.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for part in parts {
            let Some(part_object) = part.as_object_mut() else {
                continue;
            };
            let Some(text) = part_object.get("text") else {
                continue;
            };
            if text.is_string() {
                continue;
            }
            part_object.insert("text".to_string(), Value::String(value_to_string(text)));
        }
    }
}

fn add_missing_tool_call_names(input: &mut Value) {
    let Some(items) = input.as_array_mut() else {
        return;
    };
    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let Some(item_type) = object.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !codex_input_item_requires_name(item_type)
            || object
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        {
            continue;
        }
        let fallback_name = object
            .get("tool_name")
            .and_then(Value::as_str)
            .or_else(|| {
                object
                    .get("function")
                    .and_then(Value::as_object)
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
            })
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("tool")
            .to_string();
        object.insert("name".to_string(), Value::String(fallback_name));
    }
}

fn codex_input_item_requires_name(item_type: &str) -> bool {
    matches!(
        item_type.trim(),
        "function_call" | "custom_tool_call" | "mcp_tool_call"
    )
}

fn is_codex_call_input_item_type(item_type: &str) -> bool {
    matches!(
        item_type.trim(),
        "function_call"
            | "tool_call"
            | "local_shell_call"
            | "tool_search_call"
            | "custom_tool_call"
            | "mcp_tool_call"
    )
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
