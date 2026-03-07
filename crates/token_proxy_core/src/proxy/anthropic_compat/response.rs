use axum::body::Bytes;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proxy::compat_reason;

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(super) fn responses_response_to_anthropic(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("msg_proxy");
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .or(model_hint)
        .unwrap_or("unknown");

    let usage = object
        .get("usage")
        .and_then(Value::as_object)
        .map(map_openai_usage_to_anthropic_usage);

    let output = object
        .get("output")
        .and_then(Value::as_array)
        .map(|items| items.as_slice())
        .unwrap_or(&[]);
    let mut combined_text = String::new();
    let mut thinking_text = String::new();
    let mut tool_uses = Vec::new();

    for item in output {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if item.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        let Some(part) = part.as_object() else {
                            continue;
                        };
                        match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    combined_text.push_str(text);
                                }
                            }
                            Some("reasoning_text") => {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    thinking_text.push_str(text);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("function_call") => {
                if let Some(tool_use) = responses_function_call_to_tool_use(item) {
                    tool_uses.push(tool_use);
                }
            }
            _ => {}
        }
    }

    let mut content = Vec::new();
    if !thinking_text.trim().is_empty() {
        let signature = thinking_signature(&thinking_text);
        let mut block = json!({ "type": "thinking", "thinking": thinking_text });
        if let (Some(signature), Some(block)) = (signature, block.as_object_mut()) {
            block.insert("signature".to_string(), Value::String(signature));
        }
        content.push(block);
    }
    if !combined_text.trim().is_empty() || tool_uses.is_empty() {
        content.push(json!({ "type": "text", "text": combined_text }));
    }
    let has_tool_uses = !tool_uses.is_empty();
    content.extend(tool_uses);

    let finish_reason =
        compat_reason::chat_finish_reason_from_response_object(object, has_tool_uses);
    let stop_reason = compat_reason::anthropic_stop_reason_from_chat_finish_reason(finish_reason);

    let out = json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage.unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }))
    });

    serde_json::to_vec(&out)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

pub(super) fn anthropic_response_to_responses(body: &Bytes) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_proxy");
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let created_at = now_s();

    let usage = object
        .get("usage")
        .and_then(Value::as_object)
        .map(map_anthropic_usage_to_openai_usage);

    let content = object
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.as_slice())
        .unwrap_or(&[]);
    let mut output = Vec::new();

    let mut combined_text = String::new();
    let mut tool_calls = Vec::new();
    for block in content {
        let Some(block) = block.as_object() else {
            continue;
        };
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    combined_text.push_str(text);
                }
            }
            Some("tool_use") => {
                if let Some(call) = tool_use_to_responses_function_call(block) {
                    tool_calls.push(call);
                }
            }
            _ => {}
        }
    }

    let parallel_tool_calls = tool_calls.len() > 1;

    if !combined_text.trim().is_empty() || tool_calls.is_empty() {
        output.push(json!({
            "type": "message",
            "id": "msg_proxy",
            "status": "completed",
            "role": "assistant",
            "content": [
                { "type": "output_text", "text": combined_text, "annotations": [] }
            ]
        }));
    }
    output.extend(tool_calls);

    let out = json!({
        "id": id,
        "object": "response",
        "created_at": created_at,
        "status": "completed",
        "error": null,
        "model": model,
        "parallel_tool_calls": parallel_tool_calls,
        "output": output,
        "usage": usage
    });

    serde_json::to_vec(&out)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn responses_function_call_to_tool_use(item: &Map<String, Value>) -> Option<Value> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
    let id = if !call_id.is_empty() {
        call_id
    } else {
        item_id
    };
    if id.is_empty() {
        return None;
    }
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    let input = serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|v| v.as_object().cloned().map(Value::Object))
        .unwrap_or_else(|| json!({ "_raw": arguments }));
    Some(json!({
        "type": "tool_use",
        "id": id,
        "name": name,
        "input": input
    }))
}

fn tool_use_to_responses_function_call(block: &Map<String, Value>) -> Option<Value> {
    let call_id = block
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("call_proxy");
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
    let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    Some(json!({
        "id": format!("fc_{call_id}"),
        "type": "function_call",
        "status": "completed",
        "arguments": arguments,
        "call_id": call_id,
        "name": name
    }))
}

fn thinking_signature(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    Some(STANDARD.encode(hasher.finalize()))
}

fn map_openai_usage_to_anthropic_usage(usage: &Map<String, Value>) -> Value {
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens
    })
}

fn map_anthropic_usage_to_openai_usage(usage: &Map<String, Value>) -> Value {
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "cache_read_input_tokens": cache_read,
        "cache_creation_input_tokens": cache_creation
    })
}
