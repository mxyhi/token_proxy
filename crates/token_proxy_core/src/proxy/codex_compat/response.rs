use axum::body::Bytes;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::extract_tool_name_map_from_request_body;

pub(crate) fn codex_response_to_chat(
    bytes: &Bytes,
    request_body: Option<&str>,
) -> Result<Bytes, String> {
    let response = parse_codex_response(bytes)?;
    let tool_name_map = extract_tool_name_map_from_request_body(request_body);
    let output = build_chat_completion_value(&response, &tool_name_map);

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

pub(crate) fn codex_response_to_responses(
    bytes: &Bytes,
    request_body: Option<&str>,
) -> Result<Bytes, String> {
    let mut response = parse_codex_response(bytes)?;
    let tool_name_map = extract_tool_name_map_from_request_body(request_body);
    restore_tool_names_in_response(&mut response, &tool_name_map);

    serde_json::to_vec(&Value::Object(response))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn parse_codex_response(bytes: &Bytes) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        format!(
            "Codex upstream returned non-JSON success payload: {}",
            response_text(bytes)
        )
    })?;
    if let Some(message) = extract_error_message(&value) {
        return Err(message);
    }
    extract_response_object(&value)
        .ok_or_else(|| "Codex success response missing response object.".to_string())
}

fn extract_response_object(value: &Value) -> Option<Map<String, Value>> {
    if value.get("type").and_then(Value::as_str) == Some("response.completed") {
        return value
            .get("response")
            .and_then(Value::as_object)
            .filter(|response| is_success_response_object(response))
            .cloned();
    }
    if let Some(response) = value
        .get("response")
        .and_then(Value::as_object)
        .filter(|response| is_success_response_object(response))
    {
        return Some(response.clone());
    }
    value
        .as_object()
        .filter(|response| is_success_response_object(response))
        .cloned()
}

fn is_success_response_object(response: &Map<String, Value>) -> bool {
    response.get("object").and_then(Value::as_str) == Some("response")
        || (response.get("id").and_then(Value::as_str).is_some() && response.contains_key("output"))
}

fn extract_error_message(value: &Value) -> Option<String> {
    let root = value.as_object()?;
    if let Some(error) = root.get("error") {
        return Some(format!(
            "Codex upstream returned error payload: {}",
            error_message(error)
        ));
    }
    if root.get("type").and_then(Value::as_str) == Some("error") {
        if let Some(error) = root.get("error") {
            return Some(format!(
                "Codex upstream returned error payload: {}",
                error_message(error)
            ));
        }
        if let Some(message) = root.get("message") {
            return Some(format!(
                "Codex upstream returned error payload: {}",
                error_message(message)
            ));
        }
    }
    None
}

fn error_message(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| value.to_string())
}

fn response_text(bytes: &Bytes) -> String {
    const LIMIT: usize = 4 * 1024;
    let slice = bytes.as_ref();
    if slice.len() <= LIMIT {
        return String::from_utf8_lossy(slice).trim().to_string();
    }
    let truncated = &slice[..LIMIT];
    format!(
        "{}... (truncated)",
        String::from_utf8_lossy(truncated).trim()
    )
}

fn build_chat_completion_value(
    response: &Map<String, Value>,
    tool_name_map: &HashMap<String, String>,
) -> Value {
    let (content_text, reasoning_text, tool_calls) =
        extract_response_output(response, tool_name_map);
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl_proxy")
        .to_string();
    let created = response
        .get("created_at")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_unix_seconds);
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let finish_reason = resolve_finish_reason(response, !tool_calls.is_empty());
    let message = build_chat_message(&content_text, &reasoning_text, tool_calls);

    let mut output = Map::new();
    output.insert("id".to_string(), Value::String(id));
    output.insert(
        "object".to_string(),
        Value::String("chat.completion".to_string()),
    );
    output.insert("created".to_string(), Value::Number(created.into()));
    output.insert("model".to_string(), Value::String(model));
    output.insert(
        "choices".to_string(),
        Value::Array(vec![json!({
            "index": 0,
            "message": message,
            "finish_reason": finish_reason
        })]),
    );
    if let Some(usage) = map_usage(response) {
        output.insert("usage".to_string(), usage);
    }
    Value::Object(output)
}

fn build_chat_message(
    content: &str,
    reasoning: &str,
    tool_calls: Vec<Value>,
) -> Map<String, Value> {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), optional_text_value(content));
    message.insert(
        "reasoning_content".to_string(),
        optional_text_value(reasoning),
    );
    if tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Null);
    } else {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    message
}

fn extract_response_output(
    response: &Map<String, Value>,
    tool_name_map: &HashMap<String, String>,
) -> (String, String, Vec<Value>) {
    let mut content_text = String::new();
    let mut reasoning_text = String::new();
    let mut tool_calls = Vec::new();

    let Some(output) = response.get("output").and_then(Value::as_array) else {
        return (content_text, reasoning_text, tool_calls);
    };

    for item in output {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                if reasoning_text.is_empty() {
                    reasoning_text = extract_reasoning_summary(item);
                }
            }
            Some("message") => {
                if content_text.is_empty() {
                    content_text = extract_output_text(item);
                }
            }
            Some("function_call") => {
                if let Some(tool_call) = build_tool_call(item, tool_name_map) {
                    tool_calls.push(tool_call);
                }
            }
            _ => {}
        }
    }

    (content_text, reasoning_text, tool_calls)
}

fn extract_reasoning_summary(item: &Map<String, Value>) -> String {
    let Some(summary) = item.get("summary").and_then(Value::as_array) else {
        return String::new();
    };
    for part in summary {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("summary_text") {
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

fn extract_output_text(item: &Map<String, Value>) -> String {
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    for part in content {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("output_text") {
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

fn build_tool_call(
    item: &Map<String, Value>,
    tool_name_map: &HashMap<String, String>,
) -> Option<Value> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    let restored_name = tool_name_map.get(name).map(String::as_str).unwrap_or(name);

    Some(json!({
        "id": call_id,
        "type": "function",
        "function": {
            "name": restored_name,
            "arguments": arguments
        }
    }))
}

fn restore_tool_names_in_response(
    response: &mut Map<String, Value>,
    tool_name_map: &HashMap<String, String>,
) {
    if tool_name_map.is_empty() {
        return;
    }
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return;
    };
    for item in output {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(restored) = tool_name_map.get(name) else {
            continue;
        };
        item.insert("name".to_string(), Value::String(restored.clone()));
    }
}

fn resolve_finish_reason(response: &Map<String, Value>, has_tool_calls: bool) -> Value {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    if status == "completed" {
        Value::String(if has_tool_calls { "tool_calls" } else { "stop" }.to_string())
    } else {
        Value::Null
    }
}

fn map_usage(response: &Map<String, Value>) -> Option<Value> {
    let usage = response.get("usage")?;
    let input_tokens = usage.get("input_tokens").and_then(Value::as_i64);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_i64);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_i64);
    if input_tokens.is_none() && output_tokens.is_none() && total_tokens.is_none() {
        return None;
    }
    let mut mapped = Map::new();
    if let Some(value) = input_tokens {
        mapped.insert("prompt_tokens".to_string(), Value::Number(value.into()));
    }
    if let Some(value) = output_tokens {
        mapped.insert("completion_tokens".to_string(), Value::Number(value.into()));
    }
    if let Some(value) = total_tokens {
        mapped.insert("total_tokens".to_string(), Value::Number(value.into()));
    }
    if let Some(reasoning) = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_i64)
    {
        let mut details = Map::new();
        details.insert(
            "reasoning_tokens".to_string(),
            Value::Number(reasoning.into()),
        );
        mapped.insert(
            "completion_tokens_details".to_string(),
            Value::Object(details),
        );
    }
    Some(Value::Object(mapped))
}

fn optional_text_value(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
