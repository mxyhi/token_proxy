use axum::http::HeaderMap;
use serde_json::{Map, Value};
use time::OffsetDateTime;

pub(super) fn extract_system_prompt(object: &Map<String, Value>, messages: &[Value]) -> String {
    let mut parts = Vec::new();
    if let Some(Value::String(instructions)) = object.get("instructions") {
        if !instructions.trim().is_empty() {
            parts.push(instructions.trim().to_string());
        }
    }

    for message in messages {
        let Some(message) = message.as_object() else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str);
        if role != Some("system") {
            continue;
        }
        if let Some(content) = message.get("content") {
            match content {
                Value::String(text) => {
                    if !text.trim().is_empty() {
                        parts.push(text.trim().to_string());
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                parts.push(text.trim().to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    parts.join("\n")
}

pub(super) fn is_thinking_enabled(
    object: &Map<String, Value>,
    headers: &HeaderMap,
    system_prompt: &str,
) -> bool {
    if thinking_enabled_from_header(headers) {
        return true;
    }
    if thinking_enabled_from_claude(object) {
        return true;
    }
    if thinking_enabled_from_reasoning_effort(object) {
        return true;
    }
    if thinking_enabled_from_system_prompt(system_prompt) {
        return true;
    }
    if thinking_enabled_from_model_hint(object) {
        return true;
    }
    false
}

pub(super) fn inject_hint(mut system_prompt: String, hint: &str) -> String {
    if hint.trim().is_empty() {
        return system_prompt;
    }
    if system_prompt.trim().is_empty() {
        return hint.trim().to_string();
    }
    system_prompt.push('\n');
    system_prompt.push_str(hint.trim());
    system_prompt
}

pub(super) fn inject_timestamp(system_prompt: String) -> String {
    let timestamp = format_timestamp();
    let context = format!("[Context: Current time is {timestamp}]");
    if system_prompt.trim().is_empty() {
        return context;
    }
    format!("{context}\n\n{system_prompt}")
}

pub(super) fn extract_tool_choice_hint(object: &Map<String, Value>) -> Option<String> {
    let tool_choice = object.get("tool_choice")?;
    if let Some(choice) = tool_choice.as_str() {
        return match choice {
            "none" => Some(
                "[INSTRUCTION: Do NOT use any tools. Respond with text only.]".to_string(),
            ),
            "required" | "any" => Some("[INSTRUCTION: You MUST use at least one of the available tools to respond. Do not respond with text only - always make a tool call.]".to_string()),
            "auto" => None,
            _ => None,
        };
    }
    if let Some(choice) = tool_choice.as_object() {
        let choice_type = choice.get("type").and_then(Value::as_str).unwrap_or("");
        let name = match choice_type {
            "function" => choice
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .or_else(|| choice.get("name").and_then(Value::as_str))
                .unwrap_or(""),
            "tool" => choice.get("name").and_then(Value::as_str).unwrap_or(""),
            "any" => "",
            _ => "",
        };
        if choice_type == "any" {
            return Some("[INSTRUCTION: You MUST use at least one of the available tools to respond. Do not respond with text only - always make a tool call.]".to_string());
        }
        if !name.trim().is_empty() {
            return Some(format!("[INSTRUCTION: You MUST use the tool named '{name}' to respond. Do not use any other tool or respond with text only.]"));
        }
    }
    None
}

pub(super) fn extract_response_format_hint(object: &Map<String, Value>) -> Option<String> {
    let mut format_value = object.get("response_format");
    if format_value.is_none() {
        format_value = object
            .get("text")
            .and_then(Value::as_object)
            .and_then(|text| text.get("format"));
    }
    let format_value = format_value?;
    let format_type = format_value.get("type").and_then(Value::as_str);
    match format_type {
        Some("json_object") => Some("[INSTRUCTION: You MUST respond with valid JSON only. Do not include any text before or after the JSON. Do not wrap the JSON in markdown code blocks. Output raw JSON directly.]".to_string()),
        Some("json_schema") => {
            let schema = format_value
                .get("json_schema")
                .and_then(Value::as_object)
                .and_then(|schema| schema.get("schema"));
            if let Some(schema) = schema {
                let mut schema_str = schema.to_string();
                if schema_str.len() > 500 {
                    schema_str.truncate(500);
                    schema_str.push_str("...");
                }
                return Some(format!("[INSTRUCTION: You MUST respond with valid JSON that matches this schema: {schema_str}. Do not include any text before or after the JSON. Do not wrap the JSON in markdown code blocks. Output raw JSON directly.]"));
            }
            Some("[INSTRUCTION: You MUST respond with valid JSON only. Do not include any text before or after the JSON. Do not wrap the JSON in markdown code blocks. Output raw JSON directly.]".to_string())
        }
        Some("text") | _ => None,
    }
}

fn format_timestamp() -> String {
    let format = time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second] UTC");
    if let Ok(format) = format {
        if let Ok(value) = OffsetDateTime::now_utc().format(&format) {
            return value;
        }
    }
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn thinking_enabled_from_header(headers: &HeaderMap) -> bool {
    let beta = headers.get("anthropic-beta").or_else(|| headers.get("Anthropic-Beta"));
    let Some(beta) = beta else {
        return false;
    };
    beta.to_str()
        .ok()
        .is_some_and(|value| value.contains("interleaved-thinking"))
}

fn thinking_enabled_from_claude(object: &Map<String, Value>) -> bool {
    let Some(thinking) = object.get("thinking").and_then(Value::as_object) else {
        return false;
    };
    if thinking.get("type").and_then(Value::as_str) != Some("enabled") {
        return false;
    }
    if let Some(budget) = thinking.get("budget_tokens").and_then(Value::as_i64) {
        return budget > 0;
    }
    true
}

fn thinking_enabled_from_reasoning_effort(object: &Map<String, Value>) -> bool {
    let Some(reasoning) = object.get("reasoning_effort").and_then(Value::as_str) else {
        return false;
    };
    !reasoning.trim().is_empty() && reasoning != "none"
}

fn thinking_enabled_from_system_prompt(system_prompt: &str) -> bool {
    extract_thinking_mode(system_prompt)
        .is_some_and(|value| matches!(value.trim(), "interleaved" | "enabled"))
}

fn thinking_enabled_from_model_hint(object: &Map<String, Value>) -> bool {
    if object.get("max_completion_tokens").is_none() {
        return false;
    }
    let Some(model) = object.get("model").and_then(Value::as_str) else {
        return false;
    };
    let lower = model.to_ascii_lowercase();
    lower.contains("thinking") || lower.contains("reason")
}

pub(super) fn has_thinking_tags(system_prompt: &str) -> bool {
    system_prompt.contains("<thinking_mode>") || system_prompt.contains("<max_thinking_length>")
}

fn extract_thinking_mode(system_prompt: &str) -> Option<String> {
    let start = system_prompt.find("<thinking_mode>")?;
    let end = system_prompt.find("</thinking_mode>")?;
    if end <= start {
        return None;
    }
    let value_start = start + "<thinking_mode>".len();
    Some(system_prompt[value_start..end].to_string())
}
