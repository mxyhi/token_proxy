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
pub(crate) fn gemini_response_to_chat(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    // 检查是否有 error 字段（Gemini 错误响应）
    if let Some(error) = object.get("error") {
        return handle_gemini_error(error, model_hint);
    }

    let candidates = object
        .get("candidates")
        .and_then(Value::as_array)
        .map(|arr| arr.as_slice())
        .unwrap_or(&[]);

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
        .and_then(Value::as_object)
        .map(gemini_usage_to_chat_usage);

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

/// 处理 Gemini 错误响应
fn handle_gemini_error(error: &Value, model_hint: Option<&str>) -> Result<Bytes, String> {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Unknown error from Gemini");
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or(500);

    let model = model_hint.unwrap_or("gemini");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let out = json!({
        "id": format!("chatcmpl_gemini_{now_ms}"),
        "object": "chat.completion",
        "created": (now_ms / 1000) as i64,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!("Error from Gemini (code {}): {}", code, message)
            },
            "finish_reason": "stop"
        }],
        "usage": null
    });

    serde_json::to_vec(&out)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize error response: {err}"))
}

/// 将 Gemini candidate 转换为 Chat choice
fn gemini_candidate_to_chat_choice(candidate: &Value, index: usize) -> Option<Value> {
    let candidate = candidate.as_object()?;
    let content = candidate.get("content")?.as_object()?;
    let parts = content.get("parts").and_then(Value::as_array)?;

    let mut text_content = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_call_index = 0;

    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };

        // 文本内容
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            text_content.push_str(text);
        }

        // 函数调用
        if let Some(function_call) = part.get("functionCall").and_then(Value::as_object) {
            let tool_call = gemini_function_call_to_chat_tool_call(function_call, tool_call_index);
            tool_calls.push(tool_call);
            tool_call_index += 1;
        }
    }

    let finish_reason = gemini_finish_reason_to_chat(
        candidate.get("finishReason").and_then(Value::as_str),
        !tool_calls.is_empty(),
    );

    let mut message = json!({
        "role": "assistant",
        "content": if text_content.is_empty() { Value::Null } else { Value::String(text_content) }
    });

    if !tool_calls.is_empty() {
        if let Some(msg) = message.as_object_mut() {
            msg.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }
    }

    Some(json!({
        "index": index,
        "message": message,
        "finish_reason": finish_reason
    }))
}

/// 将 Gemini finishReason 转换为 Chat finish_reason
fn gemini_finish_reason_to_chat(reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        return "tool_calls";
    }
    match reason {
        Some("STOP") => "stop",
        Some("MAX_TOKENS") => "length",
        Some("SAFETY") => "content_filter",
        Some("RECITATION") => "content_filter",
        Some("OTHER") => "stop",
        Some("BLOCKLIST") => "content_filter",
        Some("PROHIBITED_CONTENT") => "content_filter",
        Some("SPII") => "content_filter",
        _ => "stop",
    }
}

/// 将 Gemini usageMetadata 转换为 Chat usage
fn gemini_usage_to_chat_usage(usage: &Map<String, Value>) -> Value {
    let prompt_tokens = usage
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("totalTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + completion_tokens);
    let cached_tokens = usage
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64);

    let mut result = json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": total_tokens
    });

    if let Some(cached) = cached_tokens {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("cached_tokens".to_string(), json!(cached));
        }
    }

    result
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
            obj.insert("finishReason".to_string(), Value::String(reason.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_response_to_gemini_maps_tool_calls_and_text() {
        let input = json!({
            "id": "chatcmpl_x",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "getFoo", "arguments": "{\"a\":1}" }
                    }]
                },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 }
        });

        let output = chat_response_to_gemini(&Bytes::from(serde_json::to_vec(&input).unwrap()), None)
            .expect("convert");
        let value: Value = serde_json::from_slice(&output).expect("json");
        assert_eq!(value["candidates"][0]["content"]["parts"][0]["text"], json!("hello"));
        assert_eq!(
            value["candidates"][0]["content"]["parts"][1]["functionCall"]["name"],
            json!("getFoo")
        );
        assert_eq!(value["usageMetadata"]["totalTokenCount"], json!(3));
    }
}
