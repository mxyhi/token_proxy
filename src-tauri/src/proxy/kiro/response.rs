use std::collections::HashSet;

use serde_json::{Map, Value};

use super::event_stream::EventStreamDecoder;
use super::types::KiroToolUse;

#[derive(Clone, Debug, Default)]
pub(crate) struct KiroUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct KiroParsedResponse {
    pub(crate) content: String,
    pub(crate) tool_uses: Vec<KiroToolUse>,
    pub(crate) usage: KiroUsage,
    pub(crate) stop_reason: Option<String>,
}

pub(crate) fn parse_event_stream(bytes: &[u8]) -> Result<KiroParsedResponse, String> {
    let mut decoder = EventStreamDecoder::new();
    let messages = decoder
        .push(bytes)
        .map_err(|err| format!("EventStream parse error: {}", err.message))?;

    let mut content = String::new();
    let mut tool_uses: Vec<KiroToolUse> = Vec::new();
    let mut usage = KiroUsage::default();
    let mut stop_reason: Option<String> = None;
    let mut processed_tool_ids: HashSet<String> = HashSet::new();
    let mut tool_state: Option<ToolUseState> = None;

    for message in messages {
        if message.payload.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Value>(&message.payload) else {
            continue;
        };
        let Some(event_obj) = event.as_object() else {
            continue;
        };

        if let Some(error) = extract_error(event_obj) {
            return Err(error);
        }

        update_stop_reason(event_obj, &mut stop_reason);
        update_usage(event_obj, &mut usage);

        let event_type = if !message.event_type.is_empty() {
            message.event_type.as_str()
        } else {
            detect_event_type(event_obj)
        };

        match event_type {
            "assistantResponseEvent" => {
                if let Some(Value::Object(assistant)) = event_obj.get("assistantResponseEvent") {
                    if let Some(text) = assistant.get("content").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                    if let Some(tool_items) = assistant.get("toolUses").and_then(Value::as_array) {
                        extract_tool_uses(tool_items, &mut tool_uses, &mut processed_tool_ids);
                    }
                    update_stop_reason(assistant, &mut stop_reason);
                }
                if let Some(text) = event_obj.get("content").and_then(Value::as_str) {
                    content.push_str(text);
                }
                if let Some(tool_items) = event_obj.get("toolUses").and_then(Value::as_array) {
                    extract_tool_uses(tool_items, &mut tool_uses, &mut processed_tool_ids);
                }
            }
            "toolUseEvent" => {
                let (completed, next_state) =
                    process_tool_use_event(event_obj, tool_state.take(), &mut processed_tool_ids);
                tool_uses.extend(completed);
                tool_state = next_state;
            }
            "reasoningContentEvent" => {
                if let Some(Value::Object(reasoning)) = event_obj.get("reasoningContentEvent") {
                    if let Some(text) = reasoning.get("thinkingText").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                }
            }
            _ => {}
        }
    }

    if stop_reason.is_none() {
        if !tool_uses.is_empty() {
            stop_reason = Some("tool_use".to_string());
        } else {
            stop_reason = Some("end_turn".to_string());
        }
    }

    Ok(KiroParsedResponse {
        content,
        tool_uses,
        usage,
        stop_reason,
    })
}

fn detect_event_type(event: &Map<String, Value>) -> &str {
    for key in [
        "assistantResponseEvent",
        "toolUseEvent",
        "reasoningContentEvent",
        "messageStopEvent",
        "messageMetadataEvent",
        "metadataEvent",
        "usageEvent",
        "usage",
        "supplementaryWebLinksEvent",
    ] {
        if event.contains_key(key) {
            return key;
        }
    }
    ""
}

fn extract_error(event: &Map<String, Value>) -> Option<String> {
    if let Some(Value::String(err_type)) = event.get("_type") {
        let message = event
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        return Some(format!("Kiro error: {err_type} {message}"));
    }
    if let Some(Value::String(kind)) = event.get("type") {
        if matches!(kind.as_str(), "error" | "exception" | "internalServerException") {
            let message = event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            if message.is_empty() {
                if let Some(Value::Object(err_obj)) = event.get("error") {
                    if let Some(text) = err_obj.get("message").and_then(Value::as_str) {
                        return Some(format!("Kiro error: {text}"));
                    }
                }
            }
            return Some(format!("Kiro error: {message}"));
        }
    }
    None
}

fn update_stop_reason(event: &Map<String, Value>, stop_reason: &mut Option<String>) {
    if let Some(reason) = event.get("stop_reason").and_then(Value::as_str) {
        *stop_reason = Some(reason.to_string());
    }
    if let Some(reason) = event.get("stopReason").and_then(Value::as_str) {
        *stop_reason = Some(reason.to_string());
    }
}

fn update_usage(event: &Map<String, Value>, usage: &mut KiroUsage) {
    if let Some(tokens) = event.get("inputTokens").and_then(Value::as_u64) {
        usage.input_tokens = Some(tokens);
    }
    if let Some(tokens) = event.get("outputTokens").and_then(Value::as_u64) {
        usage.output_tokens = Some(tokens);
    }
    if let Some(tokens) = event.get("totalTokens").and_then(Value::as_u64) {
        usage.total_tokens = Some(tokens);
    }

    if let Some(metadata) = event.get("messageMetadataEvent").and_then(Value::as_object) {
        update_usage_from_metadata(metadata, usage);
    } else if let Some(metadata) = event.get("metadataEvent").and_then(Value::as_object) {
        update_usage_from_metadata(metadata, usage);
    }

    if let Some(usage_obj) = event.get("usage").and_then(Value::as_object) {
        update_usage_from_usage_obj(usage_obj, usage);
    }
    if let Some(usage_obj) = event.get("usageEvent").and_then(Value::as_object) {
        update_usage_from_usage_obj(usage_obj, usage);
    }

    if let Some(links) = event.get("supplementaryWebLinksEvent").and_then(Value::as_object) {
        if let Some(tokens) = links.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = links.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }
}

fn update_usage_from_metadata(metadata: &Map<String, Value>, usage: &mut KiroUsage) {
    if let Some(token_usage) = metadata.get("tokenUsage").and_then(Value::as_object) {
        if let Some(tokens) = token_usage.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("uncachedInputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("cacheReadInputTokens").and_then(Value::as_u64) {
            let current = usage.input_tokens.unwrap_or(0);
            usage.input_tokens = Some(current + tokens);
        }
    }

    if usage.input_tokens.is_none() {
        if let Some(tokens) = metadata.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
    }
    if usage.output_tokens.is_none() {
        if let Some(tokens) = metadata.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }
    if usage.total_tokens.is_none() {
        if let Some(tokens) = metadata.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
    }
}

fn update_usage_from_usage_obj(usage_obj: &Map<String, Value>, usage: &mut KiroUsage) {
    let input_tokens = usage_obj
        .get("input_tokens")
        .or_else(|| usage_obj.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage_obj
        .get("output_tokens")
        .or_else(|| usage_obj.get("completion_tokens"))
        .and_then(Value::as_u64);
    let total_tokens = usage_obj.get("total_tokens").and_then(Value::as_u64);

    if input_tokens.is_some() {
        usage.input_tokens = input_tokens;
    }
    if output_tokens.is_some() {
        usage.output_tokens = output_tokens;
    }
    if total_tokens.is_some() {
        usage.total_tokens = total_tokens;
    }
}

fn extract_tool_uses(
    tool_items: &[Value],
    output: &mut Vec<KiroToolUse>,
    processed: &mut HashSet<String>,
) {
    for item in tool_items {
        let Some(tool) = item.as_object() else {
            continue;
        };
        let tool_use_id = tool
            .get("toolUseId")
            .or_else(|| tool.get("tool_use_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if tool_use_id.is_empty() || processed.contains(tool_use_id) {
            continue;
        }
        let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
        let input = tool
            .get("input")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        processed.insert(tool_use_id.to_string());
        output.push(KiroToolUse {
            tool_use_id: tool_use_id.to_string(),
            name: name.to_string(),
            input,
        });
    }
}

struct ToolUseState {
    id: String,
    name: String,
    input_buffer: String,
}

fn process_tool_use_event(
    event: &Map<String, Value>,
    current: Option<ToolUseState>,
    processed: &mut HashSet<String>,
) -> (Vec<KiroToolUse>, Option<ToolUseState>) {
    let mut tool_uses = Vec::new();
    let mut state = current;

    let source = event
        .get("toolUseEvent")
        .and_then(Value::as_object)
        .unwrap_or(event);

    let tool_use_id = source
        .get("toolUseId")
        .or_else(|| source.get("tool_use_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = source.get("name").and_then(Value::as_str).unwrap_or("");
    let stop = source.get("stop").and_then(Value::as_bool).unwrap_or(false);

    if !tool_use_id.is_empty() && !name.is_empty() {
        if let Some(current_state) = &state {
            if current_state.id != tool_use_id {
                state = None;
            }
        }

        if state.is_none() && !processed.contains(tool_use_id) {
            state = Some(ToolUseState {
                id: tool_use_id.to_string(),
                name: name.to_string(),
                input_buffer: String::new(),
            });
        }
    }

    if let Some(current_state) = &mut state {
        if let Some(Value::String(fragment)) = source.get("input") {
            current_state.input_buffer.push_str(fragment);
        } else if let Some(Value::Object(input)) = source.get("input") {
            let serialized = serde_json::to_string(input).unwrap_or_default();
            current_state.input_buffer = serialized;
        }
    }

    if stop {
        if let Some(current_state) = state.take() {
            let input = parse_tool_input(&current_state.input_buffer);
            processed.insert(current_state.id.clone());
            tool_uses.push(KiroToolUse {
                tool_use_id: current_state.id,
                name: current_state.name,
                input,
            });
        }
    }

    (tool_uses, state)
}

fn parse_tool_input(raw: &str) -> Map<String, Value> {
    if raw.trim().is_empty() {
        return Map::new();
    }
    let repaired = repair_json(raw);
    serde_json::from_str::<Map<String, Value>>(&repaired).unwrap_or_default()
}

fn repair_json(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ',' {
            if let Some(next) = chars.peek() {
                if *next == '}' || *next == ']' {
                    continue;
                }
            }
        }
        output.push(ch);
    }
    output
}
