use axum::http::HeaderMap;
use rand::RngCore;
use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::constants::{KIRO_AGENTIC_SYSTEM_PROMPT, KIRO_MAX_OUTPUT_TOKENS};
use super::tools::convert_openai_tools;
use super::types::{
    KiroAssistantResponseMessage, KiroConversationState, KiroCurrentMessage, KiroImage,
    KiroImageSource, KiroInferenceConfig, KiroPayload, KiroTextContent, KiroToolResult,
    KiroToolUse, KiroUserInputMessage, KiroUserInputMessageContext,
};

const THINKING_HINT: &str = "<thinking_mode>enabled</thinking_mode>\n<max_thinking_length>200000</max_thinking_length>";

pub(crate) struct BuildPayloadResult {
    pub(crate) payload: Vec<u8>,
    pub(crate) thinking_enabled: bool,
}

pub(crate) fn build_payload_from_responses(
    request: &Value,
    model_id: &str,
    profile_arn: Option<&str>,
    origin: &str,
    is_agentic: bool,
    is_chat_only: bool,
    headers: &HeaderMap,
) -> Result<BuildPayloadResult, String> {
    let Some(object) = request.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    let messages = extract_input_messages(object)?;
    let mut system_prompt = extract_system_prompt(object, &messages);

    let mut thinking_enabled = is_thinking_enabled(object, headers, &system_prompt);
    if thinking_enabled {
        system_prompt = inject_hint(system_prompt, THINKING_HINT);
    }

    system_prompt = inject_timestamp(system_prompt);
    if is_agentic {
        system_prompt = inject_hint(system_prompt, KIRO_AGENTIC_SYSTEM_PROMPT.trim());
    }

    if let Some(tool_choice_hint) = extract_tool_choice_hint(object) {
        system_prompt = inject_hint(system_prompt, &tool_choice_hint);
    }
    if let Some(response_format_hint) = extract_response_format_hint(object) {
        system_prompt = inject_hint(system_prompt, &response_format_hint);
    }

    let (history, mut current_user, mut current_tool_results) =
        process_messages(&messages, model_id, origin);

    if let Some(user) = current_user.as_mut() {
        let effective_system_prompt = if history.is_empty() {
            system_prompt.clone()
        } else {
            String::new()
        };
        user.content = build_final_content(&user.content, &effective_system_prompt, &current_tool_results);

        current_tool_results = deduplicate_tool_results(current_tool_results);
        let tools = convert_openai_tools(object.get("tools"), is_chat_only);
        if !tools.is_empty() || !current_tool_results.is_empty() {
            user.user_input_message_context = Some(KiroUserInputMessageContext {
                tool_results: current_tool_results,
                tools,
            });
        }
    }

    let current_message = if let Some(user) = current_user {
        KiroCurrentMessage {
            user_input_message: user,
        }
    } else {
        let fallback = if system_prompt.trim().is_empty() {
            "Continue".to_string()
        } else {
            format!("--- SYSTEM PROMPT ---\n{system_prompt}\n--- END SYSTEM PROMPT ---\n")
        };
        KiroCurrentMessage {
            user_input_message: KiroUserInputMessage {
                content: fallback,
                model_id: model_id.to_string(),
                origin: origin.to_string(),
                images: Vec::new(),
                user_input_message_context: None,
            },
        }
    };

    let inference_config = build_inference_config(object);

    let payload = KiroPayload {
        conversation_state: KiroConversationState {
            chat_trigger_type: "MANUAL".to_string(),
            conversation_id: random_uuid(),
            current_message,
            history,
        },
        profile_arn: profile_arn.map(|value| value.to_string()),
        inference_config,
    };

    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|err| format!("Failed to serialize request payload: {err}"))?;

    Ok(BuildPayloadResult {
        payload: payload_bytes,
        thinking_enabled,
    })
}

fn extract_input_messages(object: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let input = object.get("input");
    match input {
        Some(Value::String(text)) => Ok(vec![serde_json::json!({ "role": "user", "content": text })]),
        Some(Value::Array(items)) => responses_input_to_chat_messages(items),
        Some(Value::Null) | None => Ok(Vec::new()),
        _ => Err("Responses input must be a string or array.".to_string()),
    }
}

fn responses_input_to_chat_messages(items: &[Value]) -> Result<Vec<Value>, String> {
    let mut messages = Vec::with_capacity(items.len());
    for item in items {
        messages.push(responses_input_item_to_chat_message(item)?);
    }
    Ok(messages)
}

fn responses_input_item_to_chat_message(item: &Value) -> Result<Value, String> {
    let Some(item) = item.as_object() else {
        return Err("Responses input item must be an object.".to_string());
    };

    if item.get("role").and_then(Value::as_str).is_some() {
        let mut output = item.clone();
        if let Some(content) = responses_message_content_to_chat_content(item.get("content")) {
            output.insert("content".to_string(), content);
        }
        return Ok(Value::Object(output));
    }

    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
        return Err("Responses input item must include role or type.".to_string());
    };

    match item_type {
        "message" => responses_message_item_to_chat_message(item),
        "function_call_output" => responses_function_call_output_item_to_chat_message(item),
        "function_call" => responses_function_call_item_to_chat_message(item),
        other => Err(format!("Unsupported Responses input item type: {other}")),
    }
}

fn responses_message_item_to_chat_message(item: &Map<String, Value>) -> Result<Value, String> {
    let role = item
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| "Responses message item must include role.".to_string())?;
    let content = item
        .get("content")
        .and_then(responses_message_content_to_chat_content)
        .unwrap_or_else(|| Value::String(String::new()));
    Ok(serde_json::json!({ "role": role, "content": content }))
}

fn responses_function_call_output_item_to_chat_message(
    item: &Map<String, Value>,
) -> Result<Value, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "function_call_output must include call_id.".to_string())?;
    let output = item.get("output").and_then(Value::as_str).unwrap_or("");
    Ok(serde_json::json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": output
    }))
}

fn responses_function_call_item_to_chat_message(item: &Map<String, Value>) -> Result<Value, String> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "function_call must include call_id.".to_string())?;
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    Ok(serde_json::json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [
            {
                "id": call_id,
                "type": "function",
                "function": { "name": name, "arguments": arguments }
            }
        ]
    }))
}

fn responses_message_content_to_chat_content(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(text)) => Some(Value::String(text.to_string())),
        Some(Value::Array(parts)) => {
            let mut output_parts = Vec::new();
            let mut combined = String::new();
            let mut text_only = true;
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                let part_type = part.get("type").and_then(Value::as_str);
                match part_type {
                    Some("input_text") | Some("text") | Some("output_text") => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            combined.push_str(text);
                            output_parts.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                    Some("refusal") => {
                        let text = part
                            .get("refusal")
                            .or_else(|| part.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if !text.is_empty() {
                            combined.push_str(text);
                            output_parts.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                    Some("input_image") | Some("output_image") => {
                        if let Some(image_url) = part.get("image_url") {
                            text_only = false;
                            output_parts.push(serde_json::json!({ "type": "image_url", "image_url": image_url }));
                        }
                    }
                    _ => {
                        text_only = false;
                    }
                }
            }
            if text_only {
                Some(Value::String(combined))
            } else {
                Some(Value::Array(output_parts))
            }
        }
        Some(_) => Some(Value::String(String::new())),
        None => None,
    }
}

fn extract_system_prompt(object: &Map<String, Value>, messages: &[Value]) -> String {
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

fn is_thinking_enabled(object: &Map<String, Value>, headers: &HeaderMap, system_prompt: &str) -> bool {
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

fn inject_hint(mut system_prompt: String, hint: &str) -> String {
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

fn inject_timestamp(system_prompt: String) -> String {
    let timestamp = format_timestamp();
    let context = format!("[Context: Current time is {timestamp}]");
    if system_prompt.trim().is_empty() {
        return context;
    }
    format!("{context}\n\n{system_prompt}")
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

fn extract_tool_choice_hint(object: &Map<String, Value>) -> Option<String> {
    let tool_choice = object.get("tool_choice")?;
    if let Some(choice) = tool_choice.as_str() {
        return match choice {
            "none" => Some("[INSTRUCTION: Do NOT use any tools. Respond with text only.]".to_string()),
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

fn extract_response_format_hint(object: &Map<String, Value>) -> Option<String> {
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

fn process_messages(
    messages: &[Value],
    model_id: &str,
    origin: &str,
) -> (Vec<KiroHistoryMessage>, Option<KiroUserInputMessage>, Vec<KiroToolResult>) {
    let mut history = Vec::new();
    let mut current_user = None;
    let mut current_tool_results = Vec::new();
    let mut pending_tool_results = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        let Some(message) = message.as_object() else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let is_last = index == messages.len().saturating_sub(1);

        match role {
            "system" => continue,
            "user" => {
                let (mut user_msg, tool_results) = build_user_message(message, model_id, origin);
                let mut tool_results = pending_tool_results
                    .drain(..)
                    .chain(tool_results)
                    .collect::<Vec<_>>();

                if is_last {
                    current_user = Some(user_msg);
                    current_tool_results = tool_results;
                } else {
                    if user_msg.content.trim().is_empty() {
                        user_msg.content = if tool_results.is_empty() {
                            "Continue".to_string()
                        } else {
                            "Tool results provided.".to_string()
                        };
                    }
                    if !tool_results.is_empty() {
                        user_msg.user_input_message_context = Some(KiroUserInputMessageContext {
                            tool_results: tool_results.drain(..).collect(),
                            tools: Vec::new(),
                        });
                    }
                    history.push(super::types::KiroHistoryMessage {
                        user_input_message: Some(user_msg),
                        assistant_response_message: None,
                    });
                }
            }
            "assistant" => {
                let assistant_msg = build_assistant_message(message);

                if !pending_tool_results.is_empty() {
                    let synthetic = KiroUserInputMessage {
                        content: "Tool results provided.".to_string(),
                        model_id: model_id.to_string(),
                        origin: origin.to_string(),
                        images: Vec::new(),
                        user_input_message_context: Some(KiroUserInputMessageContext {
                            tool_results: pending_tool_results.drain(..).collect(),
                            tools: Vec::new(),
                        }),
                    };
                    history.push(super::types::KiroHistoryMessage {
                        user_input_message: Some(synthetic),
                        assistant_response_message: None,
                    });
                }

                history.push(super::types::KiroHistoryMessage {
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
            "tool" => {
                if let Some(tool_result) = build_tool_result(message) {
                    pending_tool_results.push(tool_result);
                }
            }
            _ => {}
        }
    }

    if !pending_tool_results.is_empty() {
        current_tool_results.extend(pending_tool_results.drain(..));
        if current_user.is_none() {
            current_user = Some(KiroUserInputMessage {
                content: "Tool results provided.".to_string(),
                model_id: model_id.to_string(),
                origin: origin.to_string(),
                images: Vec::new(),
                user_input_message_context: None,
            });
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
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("text");
                    match part_type {
                        "text" | "input_text" | "output_text" => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                content.push_str(text);
                            }
                        }
                        "image_url" | "input_image" => {
                            if let Some(image) = parse_image_url(part.get("image_url")) {
                                images.push(image);
                            }
                        }
                        _ => {}
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

    (user_msg, Vec::new())
}

fn build_assistant_message(message: &Map<String, Value>) -> KiroAssistantResponseMessage {
    let mut content = String::new();
    if let Some(value) = message.get("content") {
        match value {
            Value::String(text) => content.push_str(text),
            Value::Array(parts) => {
                for part in parts {
                    if part.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            content.push_str(text);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut tool_uses = Vec::new();
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let Some(tool_call) = tool_call.as_object() else {
                continue;
            };
            if tool_call.get("type").and_then(Value::as_str) != Some("function") {
                continue;
            }
            let tool_use_id = tool_call.get("id").and_then(Value::as_str).unwrap_or("");
            let name = tool_call
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let arguments = tool_call
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let input = serde_json::from_str::<Map<String, Value>>(arguments).unwrap_or_default();
            if !tool_use_id.is_empty() && !name.is_empty() {
                tool_uses.push(KiroToolUse {
                    tool_use_id: tool_use_id.to_string(),
                    name: name.to_string(),
                    input,
                });
            }
        }
    }

    KiroAssistantResponseMessage { content, tool_uses }
}

fn build_tool_result(message: &Map<String, Value>) -> Option<KiroToolResult> {
    let tool_use_id = message
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tool_use_id.is_empty() {
        return None;
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    Some(KiroToolResult {
        content: vec![KiroTextContent {
            text: content.to_string(),
        }],
        status: "success".to_string(),
        tool_use_id: tool_use_id.to_string(),
    })
}

fn parse_image_url(value: Option<&Value>) -> Option<KiroImage> {
    let url = match value {
        Some(Value::String(url)) => url.as_str(),
        Some(Value::Object(obj)) => obj.get("url").and_then(Value::as_str)?,
        _ => return None,
    };
    if !url.starts_with("data:") {
        return None;
    }
    let parts = url.splitn(2, ";base64,").collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }
    let media_type = parts[0].trim_start_matches("data:");
    let data = parts[1].trim();
    if data.is_empty() {
        return None;
    }
    let format = media_type
        .split('/')
        .last()
        .unwrap_or("")
        .to_string();
    if format.is_empty() {
        return None;
    }
    Some(KiroImage {
        format,
        source: KiroImageSource {
            bytes: data.to_string(),
        },
    })
}

fn build_final_content(content: &str, system_prompt: &str, tool_results: &[KiroToolResult]) -> String {
    let mut output = String::new();
    if !system_prompt.trim().is_empty() {
        output.push_str("--- SYSTEM PROMPT ---\n");
        output.push_str(system_prompt.trim());
        output.push_str("\n--- END SYSTEM PROMPT ---\n\n");
    }
    output.push_str(content);

    if output.trim().is_empty() {
        if tool_results.is_empty() {
            return "Continue".to_string();
        }
        return "Tool results provided.".to_string();
    }

    output
}

fn deduplicate_tool_results(results: Vec<KiroToolResult>) -> Vec<KiroToolResult> {
    let mut seen = std::collections::HashSet::new();
    let mut output = Vec::new();
    for result in results {
        if !seen.insert(result.tool_use_id.clone()) {
            continue;
        }
        output.push(result);
    }
    output
}

fn build_inference_config(object: &Map<String, Value>) -> Option<KiroInferenceConfig> {
    let mut max_tokens = object
        .get("max_output_tokens")
        .or_else(|| object.get("max_tokens"))
        .and_then(Value::as_i64);
    if let Some(value) = max_tokens {
        if value == -1 {
            max_tokens = Some(KIRO_MAX_OUTPUT_TOKENS);
        }
    }
    let temperature = object.get("temperature").and_then(Value::as_f64);
    let top_p = object.get("top_p").and_then(Value::as_f64);

    if max_tokens.is_none() && temperature.is_none() && top_p.is_none() {
        return None;
    }

    Some(KiroInferenceConfig {
        max_tokens,
        temperature,
        top_p,
    })
}

fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}
