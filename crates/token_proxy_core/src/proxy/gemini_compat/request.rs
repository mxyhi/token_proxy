//! OpenAI Chat 请求 → Gemini 请求转换

use axum::body::Bytes;
use serde_json::{json, Map, Value};

use super::tools::{
    gemini_function_call_to_chat_tool_call, map_chat_tool_choice_to_gemini,
    map_chat_tools_to_gemini, map_gemini_tool_config_to_chat, map_gemini_tools_to_chat,
};

/// 将 OpenAI Chat 请求转换为 Gemini 格式
pub(crate) fn chat_request_to_gemini(body: &Bytes) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Err("Chat request must include messages.".to_string());
    };

    let (contents, system_instruction) = chat_messages_to_gemini_contents(messages)?;

    let mut out = Map::new();
    out.insert("contents".to_string(), Value::Array(contents));

    // 系统指令
    if let Some(system) = system_instruction {
        out.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": system }] }),
        );
    }

    // 生成参数
    let mut gen_config = Map::new();
    if let Some(temperature) = object.get("temperature").and_then(Value::as_f64) {
        gen_config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = object.get("top_p").and_then(Value::as_f64) {
        gen_config.insert("topP".to_string(), json!(top_p));
    }
    if let Some(max_tokens) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
        .and_then(Value::as_i64)
    {
        gen_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
    }
    if let Some(stop) = object.get("stop") {
        let stop_sequences = match stop {
            Value::String(s) => vec![s.clone()],
            Value::Array(arr) => arr
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect(),
            _ => vec![],
        };
        if !stop_sequences.is_empty() {
            gen_config.insert(
                "stopSequences".to_string(),
                Value::Array(stop_sequences.into_iter().map(Value::String).collect()),
            );
        }
    }
    if let Some(seed) = object.get("seed").and_then(Value::as_i64) {
        gen_config.insert("seed".to_string(), json!(seed));
    }
    // 响应格式
    if let Some(response_format) = object.get("response_format").and_then(Value::as_object) {
        if let Some(format_type) = response_format.get("type").and_then(Value::as_str) {
            if format_type == "json_object" || format_type == "json_schema" {
                gen_config.insert("responseMimeType".to_string(), json!("application/json"));
                // 如有 json_schema，复制 schema
                if format_type == "json_schema" {
                    if let Some(schema) = response_format
                        .get("json_schema")
                        .and_then(Value::as_object)
                        .and_then(|js| js.get("schema"))
                    {
                        gen_config.insert("responseSchema".to_string(), schema.clone());
                    }
                }
            }
        }
    }
    if !gen_config.is_empty() {
        out.insert("generationConfig".to_string(), Value::Object(gen_config));
    }

    // 工具
    if let Some(tools) = object.get("tools") {
        let gemini_tools = map_chat_tools_to_gemini(tools);
        if let Some(arr) = gemini_tools.as_array() {
            if !arr.is_empty() {
                out.insert("tools".to_string(), gemini_tools);
            }
        }
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        if let Some(tool_config) = map_chat_tool_choice_to_gemini(tool_choice) {
            out.insert("toolConfig".to_string(), tool_config);
        }
    }

    // 默认安全设置（参考 new-api：禁用内容过滤以保证完整回复）
    out.insert(
        "safetySettings".to_string(),
        json!([
            { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "BLOCK_NONE" },
            { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE" }
        ]),
    );

    serde_json::to_vec(&Value::Object(out))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize Gemini request: {err}"))
}

/// 将 Gemini 请求转换为 OpenAI Chat 格式
pub(crate) fn gemini_request_to_chat(
    body: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    let Some(contents) = object.get("contents").and_then(Value::as_array) else {
        return Err("Gemini request must include contents.".to_string());
    };

    let mut messages = gemini_contents_to_chat_messages(contents)?;
    if let Some(system) = extract_system_instruction(object.get("systemInstruction")) {
        messages.insert(0, json!({ "role": "system", "content": system }));
    }

    let mut out = Map::new();
    if let Some(model) = object.get("model").and_then(Value::as_str).or(model_hint) {
        out.insert("model".to_string(), Value::String(model.to_string()));
    }
    out.insert("messages".to_string(), Value::Array(messages));

    if let Some(gen_config) = object.get("generationConfig").and_then(Value::as_object) {
        map_generation_config_to_chat(gen_config, &mut out);
    }

    if let Some(tools) = object.get("tools") {
        let tools = map_gemini_tools_to_chat(tools);
        if tools.as_array().is_some_and(|arr| !arr.is_empty()) {
            out.insert("tools".to_string(), tools);
        }
    }
    if let Some(tool_config) = object.get("toolConfig") {
        if let Some(tool_choice) = map_gemini_tool_config_to_chat(tool_config) {
            out.insert("tool_choice".to_string(), tool_choice);
        }
    }

    serde_json::to_vec(&Value::Object(out))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize Chat request: {err}"))
}

/// 将 Chat messages 转换为 Gemini contents，并提取系统指令
fn chat_messages_to_gemini_contents(
    messages: &[Value],
) -> Result<(Vec<Value>, Option<String>), String> {
    let mut system_texts = Vec::new();
    let mut contents = Vec::new();

    for message in messages {
        let Some(message) = message.as_object() else {
            continue;
        };

        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        match role {
            "system" | "developer" => {
                if let Some(text) = extract_text_from_content(message.get("content")) {
                    system_texts.push(text);
                }
            }
            "user" => {
                let parts = chat_content_to_gemini_parts(message.get("content"))?;
                if !parts.is_empty() {
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
            "assistant" => {
                let mut parts = chat_content_to_gemini_parts(message.get("content"))?;
                // 处理 tool_calls
                if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        if let Some(fc) = chat_tool_call_to_gemini_function_call(tool_call) {
                            parts.push(fc);
                        }
                    }
                }
                // 处理旧版 function_call
                if let Some(function_call) = message.get("function_call").and_then(Value::as_object)
                {
                    if let Some(fc) = legacy_function_call_to_gemini(function_call) {
                        parts.push(fc);
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
            }
            "tool" | "function" => {
                // 工具结果 → functionResponse
                let name = message
                    .get("name")
                    .or_else(|| message.get("tool_call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("function");
                let response_content = message.get("content");
                let response = parse_tool_response_content(response_content);
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": response
                        }
                    }]
                }));
            }
            _ => {}
        }
    }

    let system_instruction = if system_texts.is_empty() {
        None
    } else {
        Some(
            system_texts
                .into_iter()
                .filter(|t| !t.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };

    Ok((contents, system_instruction))
}

fn gemini_contents_to_chat_messages(contents: &[Value]) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    for content in contents {
        let Some(content) = content.as_object() else {
            continue;
        };
        let mut converted = gemini_content_to_chat_messages(content)?;
        messages.append(&mut converted);
    }
    Ok(messages)
}

fn gemini_content_to_chat_messages(
    content: &serde_json::Map<String, Value>,
) -> Result<Vec<Value>, String> {
    let role = content
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let role = if role == "model" { "assistant" } else { role };
    let parts = content
        .get("parts")
        .and_then(Value::as_array)
        .map(|value| value.as_slice())
        .unwrap_or(&[]);

    let mut messages = Vec::new();
    let mut content_parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        if let Some(tool_message) = function_response_to_chat_message(part) {
            if !content_parts.is_empty() || !tool_calls.is_empty() {
                messages.push(build_chat_message(role, &content_parts, &tool_calls));
                content_parts.clear();
                tool_calls.clear();
            }
            messages.push(tool_message);
            continue;
        }
        if let Some(function_call) = part.get("functionCall").and_then(Value::as_object) {
            let tool_call = gemini_function_call_to_chat_tool_call(function_call, tool_calls.len());
            tool_calls.push(tool_call);
            continue;
        }
        if let Some(content_part) = gemini_part_to_chat_content_part(part) {
            content_parts.push(content_part);
        }
    }

    if !content_parts.is_empty() || !tool_calls.is_empty() {
        messages.push(build_chat_message(role, &content_parts, &tool_calls));
    }

    Ok(messages)
}

fn build_chat_message(role: &str, content_parts: &[Value], tool_calls: &[Value]) -> Value {
    let content = build_chat_content(content_parts);
    let mut message = json!({ "role": role, "content": content });
    if !tool_calls.is_empty() {
        if let Some(message) = message.as_object_mut() {
            message.insert("tool_calls".to_string(), Value::Array(tool_calls.to_vec()));
        }
    }
    message
}

fn build_chat_content(parts: &[Value]) -> Value {
    if parts.is_empty() {
        return Value::String(String::new());
    }
    let mut combined = String::new();
    let mut text_only = true;
    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("text") {
            text_only = false;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            combined.push_str(text);
        }
    }
    if text_only {
        Value::String(combined)
    } else {
        Value::Array(parts.to_vec())
    }
}

fn gemini_part_to_chat_content_part(part: &serde_json::Map<String, Value>) -> Option<Value> {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return Some(json!({ "type": "text", "text": text }));
    }
    if let Some(inline) = part.get("inlineData").and_then(Value::as_object) {
        return gemini_inline_data_to_image_url(inline);
    }
    if let Some(file_data) = part.get("fileData").and_then(Value::as_object) {
        return gemini_file_data_to_image_url(file_data);
    }
    None
}

fn gemini_inline_data_to_image_url(data: &serde_json::Map<String, Value>) -> Option<Value> {
    let mime = data
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let payload = data.get("data").and_then(Value::as_str)?;
    let url = format!("data:{mime};base64,{payload}");
    Some(json!({ "type": "image_url", "image_url": { "url": url } }))
}

fn gemini_file_data_to_image_url(data: &serde_json::Map<String, Value>) -> Option<Value> {
    let uri = data.get("fileUri").and_then(Value::as_str)?;
    Some(json!({ "type": "image_url", "image_url": { "url": uri } }))
}

fn function_response_to_chat_message(part: &serde_json::Map<String, Value>) -> Option<Value> {
    let response = part.get("functionResponse")?.as_object()?;
    let name = response.get("name").and_then(Value::as_str).unwrap_or("");
    let payload = response
        .get("response")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let content = match payload {
        Value::String(text) => text,
        other => serde_json::to_string(&other).unwrap_or_else(|_| "{}".to_string()),
    };
    let mut message = json!({ "role": "tool", "content": content });
    if !name.is_empty() {
        if let Some(message) = message.as_object_mut() {
            message.insert("name".to_string(), Value::String(name.to_string()));
        }
    }
    Some(message)
}

fn extract_system_instruction(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    let parts = value.get("parts").and_then(Value::as_array)?;
    let mut texts = Vec::new();
    for part in parts {
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            continue;
        };
        if !text.trim().is_empty() {
            texts.push(text.to_string());
        }
    }
    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

fn map_generation_config_to_chat(
    gen_config: &serde_json::Map<String, Value>,
    out: &mut Map<String, Value>,
) {
    if let Some(temperature) = gen_config.get("temperature").and_then(Value::as_f64) {
        out.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = gen_config.get("topP").and_then(Value::as_f64) {
        out.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(max_tokens) = gen_config.get("maxOutputTokens").and_then(Value::as_i64) {
        out.insert(
            "max_completion_tokens".to_string(),
            Value::Number(max_tokens.into()),
        );
    }
    if let Some(stop) = map_stop_sequences(gen_config.get("stopSequences")) {
        out.insert("stop".to_string(), stop);
    }
    if let Some(seed) = gen_config.get("seed").and_then(Value::as_i64) {
        out.insert("seed".to_string(), json!(seed));
    }
    if let Some(response_format) = map_gemini_response_format(gen_config) {
        out.insert("response_format".to_string(), response_format);
    }
}

fn map_stop_sequences(value: Option<&Value>) -> Option<Value> {
    let Some(sequences) = value.and_then(Value::as_array) else {
        return None;
    };
    let items = sequences
        .iter()
        .filter_map(Value::as_str)
        .map(|item| Value::String(item.to_string()))
        .collect::<Vec<_>>();
    if items.is_empty() {
        None
    } else if items.len() == 1 {
        items.first().cloned()
    } else {
        Some(Value::Array(items))
    }
}

fn map_gemini_response_format(gen_config: &serde_json::Map<String, Value>) -> Option<Value> {
    if let Some(schema) = gen_config.get("responseSchema") {
        return Some(json!({
            "type": "json_schema",
            "json_schema": { "schema": schema.clone() }
        }));
    }
    let mime = gen_config
        .get("responseMimeType")
        .and_then(Value::as_str)
        .unwrap_or("");
    if mime.contains("json") {
        return Some(json!({ "type": "json_object" }));
    }
    None
}

/// 从 content 中提取纯文本
fn extract_text_from_content(content: Option<&Value>) -> Option<String> {
    let Some(content) = content else {
        return None;
    };
    match content {
        Value::String(text) => Some(text.to_string()),
        Value::Array(parts) => {
            let mut combined = String::new();
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    combined.push_str(text);
                }
            }
            if combined.is_empty() {
                None
            } else {
                Some(combined)
            }
        }
        _ => None,
    }
}

/// 将 Chat content 转换为 Gemini parts
fn chat_content_to_gemini_parts(content: Option<&Value>) -> Result<Vec<Value>, String> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    match content {
        Value::String(text) => Ok(vec![json!({ "text": text })]),
        Value::Null => Ok(vec![]),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                match part_type {
                    "text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push(json!({ "text": text }));
                        }
                    }
                    "image_url" => {
                        if let Some(inline_data) = image_url_to_gemini_inline_data(part) {
                            out.push(json!({ "inlineData": inline_data }));
                        }
                    }
                    _ => {}
                }
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

/// 将 image_url 转换为 Gemini inlineData
fn image_url_to_gemini_inline_data(part: &serde_json::Map<String, Value>) -> Option<Value> {
    let image_url = part.get("image_url")?;
    let url = match image_url {
        Value::String(s) => s.as_str(),
        Value::Object(obj) => obj.get("url").and_then(Value::as_str)?,
        _ => return None,
    };

    // 处理 data URI（base64）
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((mime_type, data)) = rest.split_once(";base64,") {
            return Some(json!({
                "mimeType": mime_type,
                "data": data
            }));
        }
    }

    // HTTP(S) URL → Gemini 支持 fileData 格式引用外部文件
    // 但简单起见，这里仅支持 base64 inline，外部 URL 需要先下载
    // 对于外部 URL，返回 None（或可扩展为使用 fileData）
    None
}

/// 将 OpenAI tool_call 转换为 Gemini functionCall
fn chat_tool_call_to_gemini_function_call(tool_call: &Value) -> Option<Value> {
    let tool_call = tool_call.as_object()?;
    let function = tool_call.get("function")?.as_object()?;
    let name = function.get("name").and_then(Value::as_str)?;
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let args: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    Some(json!({
        "functionCall": {
            "name": name,
            "args": args
        }
    }))
}

/// 将旧版 function_call 转换为 Gemini functionCall
fn legacy_function_call_to_gemini(function_call: &serde_json::Map<String, Value>) -> Option<Value> {
    let name = function_call.get("name").and_then(Value::as_str)?;
    let arguments = function_call
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let args: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    Some(json!({
        "functionCall": {
            "name": name,
            "args": args
        }
    }))
}

/// 解析工具响应内容
fn parse_tool_response_content(content: Option<&Value>) -> Value {
    let Some(content) = content else {
        return json!({});
    };
    match content {
        Value::String(s) => {
            // 尝试解析为 JSON
            serde_json::from_str(s).unwrap_or_else(|_| json!({ "result": s }))
        }
        other => other.clone(),
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "request.test.rs"]
mod tests;
