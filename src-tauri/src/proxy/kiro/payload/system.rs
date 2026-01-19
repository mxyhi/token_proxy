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
    if let Some(beta) = headers.get("anthropic-beta").or_else(|| headers.get("Anthropic-Beta")) {
        if let Ok(value) = beta.to_str() {
            if value.contains("interleaved-thinking") {
                return true;
            }
        }
    }

    if let Some(reasoning) = object.get("reasoning_effort").and_then(Value::as_str) {
        if !reasoning.trim().is_empty() && reasoning != "none" {
            return true;
        }
    }

    if system_prompt.contains("<thinking_mode>") && system_prompt.contains("</thinking_mode>") {
        return true;
    }

    if let Some(model) = object.get("model").and_then(Value::as_str) {
        let lower = model.to_ascii_lowercase();
        if lower.contains("thinking") || lower.contains("reason") {
            return true;
        }
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
            "required" => Some("[INSTRUCTION: You MUST use at least one of the available tools to respond. Do not respond with text only - always make a tool call.]".to_string()),
            "auto" => None,
            _ => None,
        };
    }
    if let Some(choice) = tool_choice.as_object() {
        if choice.get("type").and_then(Value::as_str) == Some("function") {
            if let Some(function) = choice.get("function").and_then(Value::as_object) {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    if !name.trim().is_empty() {
                        return Some(format!("[INSTRUCTION: You MUST use the tool named '{name}' to respond. Do not use any other tool or respond with text only.]"));
                    }
                }
            }
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
