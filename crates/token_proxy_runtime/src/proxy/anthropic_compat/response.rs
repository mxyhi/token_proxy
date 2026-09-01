use axum::body::Bytes;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use super::web_search;
use crate::proxy::{claude_reasoning, compat_reason};

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
        .filter(|_| model_hint.is_none())
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
    let mut content = Vec::new();
    let mut tool_use_count = 0;

    for item in output {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                if let Some(encrypted_content) = item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    if let Some(data) = claude_reasoning::redacted_thinking_data(encrypted_content)
                    {
                        content.push(json!({
                            "type": "redacted_thinking",
                            "data": data
                        }));
                    } else {
                        content.push(json!({
                            "type": "thinking",
                            "thinking": extract_reasoning_text_from_item(item),
                            "signature": encrypted_content
                        }));
                    }
                } else {
                    tracing::debug!("dropping Responses reasoning item without Claude carrier");
                }
            }
            Some("message") => {
                if item.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    for part in parts {
                        let Some(part) = part.as_object() else {
                            continue;
                        };
                        match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    combined_text.push_str(text);
                                    let mut block = json!({ "type": "text", "text": text });
                                    let citations = web_search::annotations_to_citations(
                                        part.get("annotations"),
                                    );
                                    if !citations.as_array().is_some_and(Vec::is_empty) {
                                        block
                                            .as_object_mut()
                                            .expect("text block object")
                                            .insert("citations".to_string(), citations);
                                    }
                                    content.push(block);
                                }
                            }
                            Some("reasoning_text") => {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    let mut block = json!({ "type": "thinking", "thinking": text });
                                    if let (Some(signature), Some(block)) =
                                        (thinking_signature(text), block.as_object_mut())
                                    {
                                        block.insert(
                                            "signature".to_string(),
                                            Value::String(signature),
                                        );
                                    }
                                    content.push(block);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("function_call") => {
                if let Some(tool_use) = responses_function_call_to_tool_use(item) {
                    content.push(tool_use);
                    tool_use_count += 1;
                }
            }
            Some("web_search_call") => {
                if let Some((use_block, result_block)) =
                    web_search::responses_search_call_to_claude(item)
                {
                    content.push(use_block);
                    content.push(result_block);
                }
            }
            _ => {}
        }
    }

    if content.is_empty() {
        content.push(json!({ "type": "text", "text": combined_text }));
    }

    let finish_reason =
        compat_reason::chat_finish_reason_from_response_object(object, tool_use_count > 0);
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
    let stop_reason = object.get("stop_reason").and_then(Value::as_str);
    let (status, incomplete_reason) =
        compat_reason::responses_status_from_anthropic_stop_reason(stop_reason);
    let status = status.unwrap_or("completed");
    let incomplete_details = incomplete_reason
        .map(|reason| json!({ "reason": reason }))
        .unwrap_or(Value::Null);

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
    let mut reasoning_count = 0;
    let mut message_count = 0;
    let mut tool_call_count = 0;
    // One Responses item represents the Claude server-tool use and its later result.
    let mut search_calls: HashMap<String, (usize, Value)> = HashMap::new();
    for block in content {
        let Some(block) = block.as_object() else {
            continue;
        };
        match block.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                let text = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                let signature = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let mut item = json!({
                    "type": "reasoning",
                    "id": format!("rs_proxy_{reasoning_count}"),
                    "status": status,
                    "summary": []
                });
                if let Some(item) = item.as_object_mut() {
                    if !text.trim().is_empty() {
                        item.insert(
                            "summary".to_string(),
                            json!([{ "type": "summary_text", "text": text }]),
                        );
                    }
                    if let Some(signature) = signature {
                        item.insert(
                            "encrypted_content".to_string(),
                            Value::String(signature.to_string()),
                        );
                    }
                }
                reasoning_count += 1;
                output.push(item);
            }
            Some("redacted_thinking") => {
                if let Some(data) = block
                    .get("data")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    if let Some(carrier) = claude_reasoning::redacted_thinking_carrier(data) {
                        output.push(json!({
                            "type": "reasoning",
                            "id": format!("rs_proxy_{reasoning_count}"),
                            "status": status,
                            "summary": [],
                            "encrypted_content": carrier
                        }));
                        reasoning_count += 1;
                    }
                }
            }
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    output.push(json!({
                        "type": "message",
                        "id": format!("msg_proxy_{message_count}"),
                        "status": status,
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": text,
                            "annotations": web_search::citations_to_annotations(block.get("citations"))
                        }]
                    }));
                    message_count += 1;
                }
            }
            Some("server_tool_use") => {
                if block.get("name").and_then(Value::as_str)
                    == Some(web_search::CLAUDE_WEB_SEARCH_NAME)
                {
                    let tool_use_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                    if tool_use_id.is_empty() {
                        tracing::debug!("dropping Claude web search block without id");
                        continue;
                    }
                    let output_index = output.len();
                    output.push(web_search::claude_search_blocks_to_responses(block, None));
                    search_calls.insert(
                        tool_use_id.to_string(),
                        (output_index, Value::Object(block.clone())),
                    );
                } else {
                    tracing::debug!("dropping unknown Anthropic server tool block");
                }
            }
            Some("web_search_tool_result") => {
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some((output_index, use_block)) = search_calls.remove(tool_use_id) {
                    let Some(use_block) = use_block.as_object() else {
                        continue;
                    };
                    output[output_index] =
                        web_search::claude_search_blocks_to_responses(use_block, Some(block));
                } else {
                    tracing::debug!(tool_use_id, "dropping unmatched Claude web search result");
                }
            }
            Some("tool_use") => {
                if let Some(call) = tool_use_to_responses_function_call(block) {
                    output.push(call);
                    tool_call_count += 1;
                }
            }
            _ => {}
        }
    }

    let parallel_tool_calls = tool_call_count > 1;

    if output.is_empty() {
        output.push(json!({
            "type": "message",
            "id": format!("msg_proxy_{message_count}"),
            "status": status,
            "role": "assistant",
            "content": [
                { "type": "output_text", "text": "", "annotations": [] }
            ]
        }));
    }

    let out = json!({
        "id": id,
        "object": "response",
        "created_at": created_at,
        "status": status,
        "error": null,
        "incomplete_details": incomplete_details,
        "model": model,
        "parallel_tool_calls": parallel_tool_calls,
        "output": output,
        "usage": usage
    });

    serde_json::to_vec(&out)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn extract_reasoning_summary(item: &Map<String, Value>) -> String {
    let Some(summary) = item.get("summary").and_then(Value::as_array) else {
        return String::new();
    };
    let mut combined = String::new();
    for part in summary {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("summary_text") {
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            combined.push_str(text);
        }
    }
    combined
}

fn extract_reasoning_text_from_item(item: &Map<String, Value>) -> String {
    let summary = extract_reasoning_summary(item);
    if !summary.is_empty() {
        return summary;
    }
    item.get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_object)
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("reasoning_text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
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
    let cache_read_tokens = cache_read.unwrap_or(0);
    let cache_creation_tokens = cache_creation.unwrap_or(0);
    let total_input_tokens = input_tokens
        .saturating_add(cache_read_tokens)
        .saturating_add(cache_creation_tokens);
    if cache_creation_tokens > 0 {
        tracing::debug!(
            cache_creation_tokens,
            "mapped Anthropic cache creation usage"
        );
    }
    json!({
        "input_tokens": total_input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_input_tokens + output_tokens,
        "input_tokens_details": {
            "cached_tokens": cache_read_tokens,
            "cached_creation_tokens": cache_creation_tokens
        }
    })
}
