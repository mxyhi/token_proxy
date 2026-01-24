use std::collections::HashSet;

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

use super::super::constants::KIRO_AGENTIC_SYSTEM_PROMPT;
use super::super::types::{
    KiroAssistantResponseMessage, KiroConversationState, KiroHistoryMessage, KiroImage,
    KiroImageSource, KiroPayload, KiroTextContent, KiroToolResult, KiroToolUse,
    KiroUserInputMessage, KiroUserInputMessageContext,
};
use super::inference::build_inference_config;
use super::system::{
    extract_tool_choice_hint, has_thinking_tags, inject_hint, inject_timestamp, is_thinking_enabled,
};
use super::{BuildPayloadResult, THINKING_HINT};
use super::super::utils::random_uuid;

pub(crate) fn build_payload_from_claude(
    request: &Value,
    model_id: &str,
    profile_arn: Option<&str>,
    origin: &str,
    is_agentic: bool,
    is_chat_only: bool,
    headers: &HeaderMap,
) -> Result<BuildPayloadResult, String> {
    let object = request
        .as_object()
        .ok_or_else(|| "Request body must be a JSON object.".to_string())?;
    let messages = extract_messages(object)?;
    let merged_messages = merge_adjacent_messages(&messages);
    let system_prompt = build_system_prompt(object, headers, is_agentic);

    let (history, current_user, current_tool_results) =
        process_claude_messages(&merged_messages, model_id, origin);
    let current_message = super::build_current_message(
        &history,
        current_user,
        current_tool_results,
        model_id,
        origin,
        &system_prompt,
        object,
        is_chat_only,
    );

    let payload = KiroPayload {
        conversation_state: KiroConversationState {
            chat_trigger_type: "MANUAL".to_string(),
            conversation_id: random_uuid(),
            current_message,
            history,
        },
        profile_arn: profile_arn.map(|value| value.to_string()),
        inference_config: build_inference_config(object),
    };

    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|err| format!("Failed to serialize request payload: {err}"))?;

    Ok(BuildPayloadResult {
        payload: payload_bytes,
    })
}

fn extract_messages(object: &Map<String, Value>) -> Result<Vec<Value>, String> {
    object
        .get("messages")
        .and_then(Value::as_array)
        .map(|items| items.clone())
        .ok_or_else(|| "Request must include messages.".to_string())
}

fn build_system_prompt(object: &Map<String, Value>, headers: &HeaderMap, is_agentic: bool) -> String {
    let base_system_prompt = extract_claude_system(object);
    let thinking_enabled = is_thinking_enabled(object, headers, &base_system_prompt);

    let mut system_prompt = inject_timestamp(base_system_prompt);
    if is_agentic {
        system_prompt = inject_hint(system_prompt, KIRO_AGENTIC_SYSTEM_PROMPT.trim());
    }
    if let Some(tool_choice_hint) = extract_tool_choice_hint(object) {
        system_prompt = inject_hint(system_prompt, &tool_choice_hint);
    }

    if thinking_enabled && !has_thinking_tags(&system_prompt) {
        system_prompt = prepend_hint(system_prompt, THINKING_HINT);
    }

    system_prompt
}

fn prepend_hint(system_prompt: String, hint: &str) -> String {
    if hint.trim().is_empty() {
        return system_prompt;
    }
    if system_prompt.trim().is_empty() {
        return hint.trim().to_string();
    }
    format!("{}\n\n{}", hint.trim(), system_prompt)
}

fn extract_claude_system(object: &Map<String, Value>) -> String {
    let Some(system) = object.get("system") else {
        return String::new();
    };
    match system {
        Value::String(text) => text.to_string(),
        Value::Array(items) => {
            let mut output = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    output.push_str(text);
                } else if let Some(text) = item.as_str() {
                    output.push_str(text);
                }
            }
            output
        }
        _ => String::new(),
    }
}

fn merge_adjacent_messages(messages: &[Value]) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();
    for message in messages {
        let Some(message) = message.as_object() else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let content = message.get("content").unwrap_or(&Value::Null);
        let blocks = normalize_blocks(content);

        if let Some(last) = merged.last_mut().and_then(Value::as_object_mut) {
            if last.get("role").and_then(Value::as_str) == Some(role) {
                let merged_blocks = merge_blocks(last.get("content"), blocks);
                last.insert("content".to_string(), Value::Array(merged_blocks));
                continue;
            }
        }

        merged.push(json!({
            "role": role,
            "content": Value::Array(blocks),
        }));
    }
    merged
}

fn normalize_blocks(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => vec![json!({ "type": "text", "text": text })],
        Value::Array(items) => items.clone(),
        _ => Vec::new(),
    }
}

fn merge_blocks(existing: Option<&Value>, mut next: Vec<Value>) -> Vec<Value> {
    let mut merged = match existing {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => vec![json!({ "type": "text", "text": text })],
        _ => Vec::new(),
    };

    merge_text_blocks(&mut merged, &mut next);
    merged.extend(next);
    merged
}

fn merge_text_blocks(existing: &mut Vec<Value>, next: &mut Vec<Value>) {
    let Some(Value::Object(last)) = existing.last_mut() else {
        return;
    };
    if last.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    let Some(Value::Object(first)) = next.first_mut() else {
        return;
    };
    if first.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    let last_text = last.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let first_text = first.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    if last_text.is_empty() && first_text.is_empty() {
        return;
    }
    last.insert("text".to_string(), Value::String(format!("{last_text}\n{first_text}")));
    next.remove(0);
}

fn process_claude_messages(
    messages: &[Value],
    model_id: &str,
    origin: &str,
) -> (Vec<KiroHistoryMessage>, Option<KiroUserInputMessage>, Vec<KiroToolResult>) {
    let mut history = Vec::new();
    let mut current_user = None;
    let mut current_tool_results = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        let Some(message) = message.as_object() else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let is_last = index == messages.len().saturating_sub(1);

        match role {
            "user" => {
                let (mut user_msg, tool_results) = build_user_message(message, model_id, origin);
                if is_last {
                    current_user = Some(user_msg);
                    current_tool_results = tool_results;
                    continue;
                }
                if user_msg.content.trim().is_empty() {
                    user_msg.content = if tool_results.is_empty() {
                        "Continue".to_string()
                    } else {
                        "Tool results provided.".to_string()
                    };
                }
                if !tool_results.is_empty() {
                    user_msg.user_input_message_context = Some(KiroUserInputMessageContext {
                        tool_results,
                        tools: Vec::new(),
                    });
                }
                history.push(KiroHistoryMessage {
                    user_input_message: Some(user_msg),
                    assistant_response_message: None,
                });
            }
            "assistant" => {
                let assistant_msg = build_assistant_message(message);
                history.push(KiroHistoryMessage {
                    user_input_message: None,
                    assistant_response_message: Some(assistant_msg),
                });
                if is_last {
                    current_user = Some(KiroUserInputMessage {
                        content: "Continue".to_string(),
                        model_id: model_id.to_string(),
                        origin: origin.to_string(),
                        images: Vec::new(),
                        user_input_message_context: None,
                    });
                }
            }
            _ => {}
        }
    }

    (history, current_user, current_tool_results)
}

fn build_user_message(
    message: &Map<String, Value>,
    model_id: &str,
    origin: &str,
) -> (KiroUserInputMessage, Vec<KiroToolResult>) {
    let mut content = String::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    let mut seen_tool_use_ids = HashSet::new();

    if let Some(value) = message.get("content") {
        match value {
            Value::String(text) => {
                content.push_str(text);
            }
            Value::Array(parts) => {
                for part in parts {
                    let Some(part) = part.as_object() else {
                        continue;
                    };
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                    match part_type {
                        "text" => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                content.push_str(text);
                            }
                        }
                        "image" => {
                            if let Some(image) = parse_image_block(part) {
                                images.push(image);
                            }
                        }
                        "tool_result" => {
                            if let Some(result) =
                                parse_tool_result_block(part, &mut seen_tool_use_ids)
                            {
                                tool_results.push(result);
                            }
                        }
                        _ => {}
                    }
                    if part_type.is_empty()
                        && part.get("tool_use_id").is_some()
                        && part.get("content").is_some()
                    {
                        if let Some(result) =
                            parse_tool_result_block(part, &mut seen_tool_use_ids)
                        {
                            tool_results.push(result);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let user_msg = KiroUserInputMessage {
        content,
        model_id: model_id.to_string(),
        origin: origin.to_string(),
        images,
        user_input_message_context: None,
    };

    (user_msg, tool_results)
}

fn parse_image_block(block: &Map<String, Value>) -> Option<KiroImage> {
    let source = block.get("source").and_then(Value::as_object)?;
    let media_type = source.get("media_type").and_then(Value::as_str)?;
    let data = source.get("data").and_then(Value::as_str)?;
    if data.is_empty() {
        return None;
    }
    let format = media_type.split('/').last().unwrap_or("");
    if format.is_empty() {
        return None;
    }
    Some(KiroImage {
        format: format.to_string(),
        source: KiroImageSource {
            bytes: data.to_string(),
        },
    })
}

fn parse_tool_result_block(
    block: &Map<String, Value>,
    seen_tool_use_ids: &mut HashSet<String>,
) -> Option<KiroToolResult> {
    let tool_use_id = block
        .get("tool_use_id")
        .or_else(|| block.get("toolUseId"))
        .or_else(|| block.get("tool_call_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if tool_use_id.is_empty() {
        return None;
    }
    if seen_tool_use_ids.contains(tool_use_id) {
        return None;
    }
    seen_tool_use_ids.insert(tool_use_id.to_string());

    let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let content = block
        .get("content")
        .or_else(|| block.get("output"));
    let mut contents = parse_tool_result_contents(content);
    if contents.is_empty() {
        contents = vec![KiroTextContent {
            text: "Tool use was cancelled by the user".to_string(),
        }];
    }

    Some(KiroToolResult {
        content: contents,
        status: if is_error {
            "error".to_string()
        } else {
            "success".to_string()
        },
        tool_use_id: tool_use_id.to_string(),
    })
}

fn parse_tool_result_contents(content: Option<&Value>) -> Vec<KiroTextContent> {
    let Some(content) = content else {
        return Vec::new();
    };
    match content {
        Value::String(text) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![KiroTextContent {
                    text: text.to_string(),
                }]
            }
        }
        Value::Object(item) => {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        return vec![KiroTextContent {
                            text: text.to_string(),
                        }];
                    }
                }
            }
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    return vec![KiroTextContent {
                        text: text.to_string(),
                    }];
                }
            }
            Vec::new()
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    if !text.is_empty() {
                        return Some(KiroTextContent {
                            text: text.to_string(),
                        });
                    }
                }
                let Some(item) = item.as_object() else {
                    return None;
                };
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    let text = item.get("text").and_then(Value::as_str)?;
                    if !text.is_empty() {
                        return Some(KiroTextContent {
                            text: text.to_string(),
                        });
                    }
                }
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        return Some(KiroTextContent {
                            text: text.to_string(),
                        });
                    }
                }
                None
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn build_assistant_message(message: &Map<String, Value>) -> KiroAssistantResponseMessage {
    let mut content = String::new();
    let mut tool_uses = Vec::new();

    if let Some(value) = message.get("content") {
        match value {
            Value::String(text) => content.push_str(text),
            Value::Array(parts) => {
                for part in parts {
                    let Some(part) = part.as_object() else {
                        continue;
                    };
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                    match part_type {
                        "text" => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                content.push_str(text);
                            }
                        }
                        "tool_use" => {
                            if let Some(tool_use) = parse_tool_use_block(part) {
                                tool_uses.push(tool_use);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    KiroAssistantResponseMessage { content, tool_uses }
}

fn parse_tool_use_block(block: &Map<String, Value>) -> Option<KiroToolUse> {
    let tool_use_id = block.get("id").and_then(Value::as_str).unwrap_or("");
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    if tool_use_id.is_empty() || name.is_empty() {
        return None;
    }
    let input_value = block.get("input");
    let input = input_value
        .and_then(Value::as_object)
        .map(|object| object.clone())
        .unwrap_or_default();

    Some(KiroToolUse {
        tool_use_id: tool_use_id.to_string(),
        name: name.to_string(),
        input,
    })
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "claude.test.rs"]
mod tests;
