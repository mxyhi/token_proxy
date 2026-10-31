//! Gemini 响应 → OpenAI Chat 响应转换

use axum::body::Bytes;
use serde_json::{json, Map, Value};

use super::tools::gemini_function_call_to_chat_tool_call;

/// 将 OpenAI Chat 响应转换为 Gemini 格式
pub(crate) fn chat_response_to_gemini(
    bytes: &Bytes,
    _model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let choices = object
        .get("choices")
        .and_then(Value::as_array)
        .map(|arr| arr.as_slice())
        .unwrap_or(&[]);

    let mut candidates = Vec::new();
    for (index, choice) in choices.iter().enumerate() {
        if let Some(candidate) = chat_choice_to_gemini_candidate(choice, index) {
            candidates.push(candidate);
        }
    }
    if candidates.is_empty() {
        candidates.push(json!({
            "index": 0,
            "content": { "role": "model", "parts": [] },
            "finishReason": "STOP"
        }));
    }

    let usage = object
        .get("usage")
        .and_then(Value::as_object)
        .and_then(map_chat_usage_to_gemini_usage);

    let mut output = json!({
        "candidates": candidates
    });
    if let Some(usage) = usage {
        if let Some(obj) = output.as_object_mut() {
            obj.insert("usageMetadata".to_string(), usage);
        }
    }

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize Gemini response: {err}"))
}

/// 将 Gemini 响应转换为 OpenAI Chat 格式
pub(crate) fn gemini_response_to_chat(
    bytes: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let mut value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Upstream response must be JSON.".to_string())?;
    if let Some(candidates) = value.get_mut("candidates").and_then(Value::as_array_mut) {
        for candidate in candidates {
            if let Some(parts) = candidate
                .pointer_mut("/content/parts")
                .and_then(Value::as_array_mut)
            {
                for part in parts {
                    if let Some(call) = part.get_mut("functionCall").and_then(Value::as_object_mut)
                    {
                        if super::partial_args::is_partial(call) {
                            let mut partial = super::partial_args::PartialCall::new(call, None)?;
                            if !partial.push(call)? {
                                return Err("Gemini returned incomplete function arguments.".into());
                            }
                            *call = partial.into_call();
                        }
                    }
                }
            }
        }
    }
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    // 检查是否有 error 字段（Gemini 错误响应）
    if object.get("error").is_some_and(|error| !error.is_null()) {
        return Err("Gemini upstream returned an error.".into());
    }

    let candidates = object
        .get("candidates")
        .and_then(Value::as_array)
        .map(|arr| arr.as_slice())
        .unwrap_or(&[]);

    if let Some(reason) = blocked_prompt_reason(&value) {
        return Err(format!("Gemini upstream blocked the prompt ({reason})."));
    }
    if candidates.iter().any(|candidate| {
        malformed_finish_reason(candidate.get("finishReason").and_then(Value::as_str))
    }) {
        return Err("Gemini upstream returned malformed output.".to_string());
    }

    let model = model_hint.unwrap_or("gemini");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let id = format!("chatcmpl_gemini_{now_ms}");
    let created = (now_ms / 1000) as i64;

    let mut choices = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if let Some(choice) = gemini_candidate_to_chat_choice(candidate, index) {
            choices.push(choice);
        }
    }

    // 如果没有候选结果，创建一个空的选择
    if choices.is_empty() {
        choices.push(json!({
            "index": 0,
            "message": {
                "role": "assistant",
                "content": ""
            },
            "finish_reason": "stop"
        }));
    }

    let usage = object
        .get("usageMetadata")
        .filter(|usage| usage.is_object())
        .map(token_proxy_protocol::gemini_usage::to_chat);

    let out = json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": choices,
        "usage": usage
    });

    serde_json::to_vec(&out)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize Chat response: {err}"))
}

pub(super) fn blocked_prompt_reason(value: &Value) -> Option<&str> {
    if value
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        return None;
    }
    value
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty() && *reason != "BLOCK_REASON_UNSPECIFIED")
}

pub(super) fn malformed_finish_reason(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("MALFORMED_FUNCTION_CALL" | "MALFORMED_RESPONSE" | "UNEXPECTED_TOOL_CALL")
    )
}

/// 将 Gemini candidate 转换为 Chat choice
fn gemini_candidate_to_chat_choice(candidate: &Value, index: usize) -> Option<Value> {
    let candidate = candidate.as_object()?;
    // Safety and token-limit candidates can terminate without a content object.
    let parts = candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_content = String::new();
    let mut thought_signatures = Vec::new();
    let mut audio = None;
    let mut tool_call_index = 0;

    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };

        if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
            thought_signatures.push(Value::String(signature.to_string()));
        }

        if part
            .get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                reasoning_content.push_str(text);
            }
            continue;
        }

        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if let Some(audio_value) = audio_response_from_text_part(text) {
                audio = Some(audio_value);
                continue;
            }
            content_parts.push(json!({ "type": "text", "text": text }));
            continue;
        }

        if let Some(inline_data) = part.get("inlineData").and_then(Value::as_object) {
            if let Some(content_part) = gemini_inline_data_to_chat_content_part(inline_data) {
                content_parts.push(content_part);
                continue;
            }
            if audio.is_none() {
                audio = gemini_inline_data_to_chat_audio(inline_data);
            }
            continue;
        }

        if let Some(file_data) = part.get("fileData").and_then(Value::as_object) {
            if let Some(content_part) = gemini_file_data_to_chat_content_part(file_data) {
                content_parts.push(content_part);
            }
            continue;
        }

        if let Some(function_call) = part.get("functionCall").and_then(Value::as_object) {
            let mut tool_call =
                gemini_function_call_to_chat_tool_call(function_call, tool_call_index);
            if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
                if let Some(tool_call) = tool_call.as_object_mut() {
                    tool_call.insert(
                        "provider_specific_fields".to_string(),
                        json!({
                            "provider": "gemini",
                            "thought_signature": signature
                        }),
                    );
                }
            }
            tool_calls.push(tool_call);
            tool_call_index += 1;
        }
    }

    let finish_reason = gemini_finish_reason_to_chat(
        candidate.get("finishReason").and_then(Value::as_str),
        !tool_calls.is_empty(),
    );
    let annotations = gemini_grounding_metadata_to_chat_annotations(candidate);
    let content = build_chat_message_content(&content_parts);

    let mut message = json!({
        "role": "assistant",
        "content": content
    });

    if !tool_calls.is_empty() {
        if let Some(msg) = message.as_object_mut() {
            msg.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }
    }
    if !reasoning_content.trim().is_empty() {
        if let Some(msg) = message.as_object_mut() {
            msg.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning_content),
            );
        }
    }
    if !annotations.is_empty() {
        if let Some(msg) = message.as_object_mut() {
            msg.insert("annotations".to_string(), Value::Array(annotations));
        }
    }
    if !thought_signatures.is_empty() {
        if let Some(msg) = message.as_object_mut() {
            msg.insert(
                "provider_specific_fields".to_string(),
                json!({ "thought_signatures": thought_signatures }),
            );
        }
    }
    if let Some(audio) = audio {
        if let Some(msg) = message.as_object_mut() {
            msg.insert("audio".to_string(), audio);
        }
    }

    Some(json!({
        "index": index,
        "message": message,
        "finish_reason": finish_reason
    }))
}

/// 将 Gemini finishReason 转换为 Chat finish_reason
pub(super) fn gemini_finish_reason_to_chat(
    reason: Option<&str>,
    has_tool_calls: bool,
) -> &'static str {
    match reason {
        Some("MAX_TOKENS") => "length",
        Some("SAFETY") => "content_filter",
        Some("RECITATION") => "content_filter",
        Some("OTHER") => "stop",
        Some("BLOCKLIST") => "content_filter",
        Some("PROHIBITED_CONTENT") => "content_filter",
        Some("SPII") => "content_filter",
        Some("IMAGE_SAFETY") => "content_filter",
        Some("IMAGE_PROHIBITED_CONTENT") => "content_filter",
        Some("TOO_MANY_TOOL_CALLS") => "length",
        _ if has_tool_calls => "tool_calls",
        _ => "stop",
    }
}

fn gemini_inline_data_to_chat_content_part(data: &Map<String, Value>) -> Option<Value> {
    let mime_type = data
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let payload = data.get("data").and_then(Value::as_str)?;
    if mime_type.starts_with("image/") {
        let url = format!("data:{mime_type};base64,{payload}");
        return Some(json!({ "type": "image_url", "image_url": { "url": url } }));
    }
    if mime_type.starts_with("audio/") {
        return None;
    }
    let url = format!("data:{mime_type};base64,{payload}");
    Some(json!({ "type": "input_file", "file_url": url }))
}

fn gemini_file_data_to_chat_content_part(data: &Map<String, Value>) -> Option<Value> {
    let uri = data.get("fileUri").and_then(Value::as_str)?;
    let mime_type = data.get("mimeType").and_then(Value::as_str).unwrap_or("");
    if mime_type.starts_with("image/") || looks_like_image_uri(uri) {
        return Some(json!({ "type": "image_url", "image_url": { "url": uri } }));
    }
    Some(json!({ "type": "input_file", "file_url": uri }))
}

fn gemini_inline_data_to_chat_audio(data: &Map<String, Value>) -> Option<Value> {
    let mime_type = data.get("mimeType").and_then(Value::as_str)?;
    if !mime_type.starts_with("audio/") {
        return None;
    }
    let payload = data.get("data").and_then(Value::as_str)?;
    Some(chat_audio_response(payload))
}

fn audio_response_from_text_part(text: &str) -> Option<Value> {
    let (mime_type, payload) = parse_data_uri(text)?;
    if !mime_type.starts_with("audio/") {
        return None;
    }
    Some(chat_audio_response(&payload))
}

fn chat_audio_response(payload: &str) -> Value {
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 24 * 60 * 60;
    json!({
        "data": payload,
        "expires_at": expires_at,
        "transcript": ""
    })
}

fn build_chat_message_content(parts: &[Value]) -> Value {
    if parts.is_empty() {
        return Value::Null;
    }
    let text_only = parts
        .iter()
        .all(|part| part.get("type").and_then(Value::as_str) == Some("text"));
    if text_only {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        return Value::String(text);
    }
    Value::Array(parts.to_vec())
}

fn gemini_grounding_metadata_to_chat_annotations(candidate: &Map<String, Value>) -> Vec<Value> {
    let grounding_metadata = match candidate.get("groundingMetadata") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_object)
            .cloned()
            .collect::<Vec<_>>(),
        Some(Value::Object(object)) => vec![object.clone()],
        _ => Vec::new(),
    };

    let mut annotations = Vec::new();
    for metadata in grounding_metadata {
        let grounding_supports = metadata
            .get("groundingSupports")
            .and_then(Value::as_array)
            .map(|value| value.as_slice())
            .unwrap_or(&[]);
        let grounding_chunks = metadata
            .get("groundingChunks")
            .and_then(Value::as_array)
            .map(|value| value.as_slice())
            .unwrap_or(&[]);

        let chunk_map = grounding_chunks
            .iter()
            .enumerate()
            .filter_map(|(index, chunk)| {
                let web = chunk.get("web")?.as_object()?;
                let uri = web.get("uri").and_then(Value::as_str)?;
                Some((
                    index,
                    (
                        uri.to_string(),
                        web.get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ),
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();

        for support in grounding_supports {
            let Some(support) = support.as_object() else {
                continue;
            };
            let segment = support.get("segment").and_then(Value::as_object);
            let start_index = segment
                .and_then(|segment| segment.get("startIndex"))
                .and_then(Value::as_i64);
            let end_index = segment
                .and_then(|segment| segment.get("endIndex"))
                .and_then(Value::as_i64);
            let Some(chunk_index) = support
                .get("groundingChunkIndices")
                .and_then(Value::as_array)
                .and_then(|indices| indices.first())
                .and_then(Value::as_u64)
            else {
                continue;
            };
            let Some((url, title)) = chunk_map.get(&(chunk_index as usize)) else {
                continue;
            };
            let (Some(start_index), Some(end_index)) = (start_index, end_index) else {
                continue;
            };
            annotations.push(json!({
                "type": "url_citation",
                "url": url,
                "title": title,
                "start_index": start_index,
                "end_index": end_index
            }));
        }
    }

    annotations
}

fn parse_data_uri(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (mime_type, data) = rest.split_once(";base64,")?;
    Some((mime_type.to_string(), data.to_string()))
}

fn looks_like_image_uri(uri: &str) -> bool {
    let extension = uri
        .split('?')
        .next()
        .and_then(|value| value.rsplit('.').next())
        .map(|value| value.to_ascii_lowercase());
    matches!(
        extension.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

fn chat_choice_to_gemini_candidate(choice: &Value, index: usize) -> Option<Value> {
    let choice = choice.as_object()?;
    let message = choice.get("message").and_then(Value::as_object)?;

    let content_parts = message.get("content_parts").and_then(Value::as_array);
    let content = if let Some(parts) = content_parts {
        map_chat_content_parts_to_gemini_parts(parts)
    } else {
        map_chat_content_to_gemini_parts(message.get("content"))
    };

    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| map_chat_tool_calls_to_gemini_parts(calls))
        .unwrap_or_default();

    let mut parts = Vec::new();
    parts.extend(content);
    parts.extend(tool_calls);

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(chat_finish_reason_to_gemini);

    let mut candidate = json!({
        "index": index,
        "content": { "role": "model", "parts": parts }
    });
    if let Some(reason) = finish_reason {
        if let Some(obj) = candidate.as_object_mut() {
            obj.insert(
                "finishReason".to_string(),
                Value::String(reason.to_string()),
            );
        }
    }
    Some(candidate)
}

fn map_chat_content_to_gemini_parts(content: Option<&Value>) -> Vec<Value> {
    let Some(content) = content else {
        return Vec::new();
    };
    match content {
        Value::String(text) => vec![json!({ "text": text })],
        Value::Array(parts) => map_chat_content_parts_to_gemini_parts(parts),
        _ => Vec::new(),
    }
}

fn map_chat_content_parts_to_gemini_parts(parts: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
        match part_type {
            "text" | "input_text" | "output_text" => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    output.push(json!({ "text": text }));
                }
            }
            "image_url" => {
                if let Some(url) = extract_image_url(part.get("image_url")) {
                    output.push(url);
                }
            }
            "input_image" | "output_image" => {
                if let Some(url) = extract_image_url(part.get("image_url")) {
                    output.push(url);
                }
            }
            _ => {}
        }
    }
    output
}

fn extract_image_url(value: Option<&Value>) -> Option<Value> {
    let url = match value {
        Some(Value::String(url)) => Some(url.as_str()),
        Some(Value::Object(obj)) => obj.get("url").and_then(Value::as_str),
        _ => None,
    }?;
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((mime_type, data)) = rest.split_once(";base64,") {
            return Some(json!({ "inlineData": { "mimeType": mime_type, "data": data } }));
        }
    }
    Some(json!({ "fileData": { "fileUri": url } }))
}

fn map_chat_tool_calls_to_gemini_parts(tool_calls: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    for call in tool_calls {
        let Some(call) = call.as_object() else {
            continue;
        };
        let function = call.get("function").and_then(Value::as_object);
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let arguments = function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or("{}");
        if name.is_empty() {
            continue;
        }
        let args: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
        output.push(json!({
            "functionCall": {
                "name": name,
                "args": args
            }
        }));
    }
    output
}

fn map_chat_usage_to_gemini_usage(usage: &Map<String, Value>) -> Option<Value> {
    let prompt_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
    let completion_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
    if prompt_tokens.is_none() && completion_tokens.is_none() && total_tokens.is_none() {
        return None;
    }
    Some(json!({
        "promptTokenCount": prompt_tokens.unwrap_or(0),
        "candidatesTokenCount": completion_tokens.unwrap_or(0),
        "totalTokenCount": total_tokens.unwrap_or_else(|| prompt_tokens.unwrap_or(0) + completion_tokens.unwrap_or(0))
    }))
}

fn chat_finish_reason_to_gemini(reason: &str) -> &'static str {
    match reason {
        "stop" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        _ => "STOP",
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "response.test.rs"]
mod tests;
