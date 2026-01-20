use serde_json::{Map, Value};

use super::super::types::{
    KiroAssistantResponseMessage, KiroHistoryMessage, KiroImage, KiroImageSource, KiroTextContent,
    KiroToolResult, KiroToolUse, KiroUserInputMessage, KiroUserInputMessageContext,
};

pub(super) fn process_messages(
    messages: &[Value],
    model_id: &str,
    origin: &str,
) -> (Vec<KiroHistoryMessage>, Option<KiroUserInputMessage>, Vec<KiroToolResult>) {
    let mut state = MessageState::new();
    for (index, message) in messages.iter().enumerate() {
        let Some(message) = message.as_object() else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let is_last = index == messages.len().saturating_sub(1);
        match role {
            "system" => continue,
            "user" => handle_user_message(message, model_id, origin, is_last, &mut state),
            "assistant" => handle_assistant_message(message, model_id, origin, is_last, &mut state),
            "tool" => handle_tool_message(message, &mut state),
            _ => {}
        }
    }
    finalize_pending_tools(model_id, origin, &mut state);
    (state.history, state.current_user, state.current_tool_results)
}

struct MessageState {
    history: Vec<KiroHistoryMessage>,
    current_user: Option<KiroUserInputMessage>,
    current_tool_results: Vec<KiroToolResult>,
    pending_tool_results: Vec<KiroToolResult>,
}

impl MessageState {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            current_user: None,
            current_tool_results: Vec::new(),
            pending_tool_results: Vec::new(),
        }
    }
}

fn handle_user_message(
    message: &Map<String, Value>,
    model_id: &str,
    origin: &str,
    is_last: bool,
    state: &mut MessageState,
) {
    let (mut user_msg, tool_results) = build_user_message(message, model_id, origin);
    let mut tool_results = state
        .pending_tool_results
        .drain(..)
        .chain(tool_results)
        .collect::<Vec<_>>();

    if is_last {
        state.current_user = Some(user_msg);
        state.current_tool_results = tool_results;
        return;
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
            tool_results: tool_results.drain(..).collect(),
            tools: Vec::new(),
        });
    }
    state.history.push(KiroHistoryMessage {
        user_input_message: Some(user_msg),
        assistant_response_message: None,
    });
}

fn handle_assistant_message(
    message: &Map<String, Value>,
    model_id: &str,
    origin: &str,
    is_last: bool,
    state: &mut MessageState,
) {
    let assistant_msg = build_assistant_message(message);
    if !state.pending_tool_results.is_empty() {
        let synthetic = KiroUserInputMessage {
            content: "Tool results provided.".to_string(),
            model_id: model_id.to_string(),
            origin: origin.to_string(),
            images: Vec::new(),
            user_input_message_context: Some(KiroUserInputMessageContext {
                tool_results: state.pending_tool_results.drain(..).collect(),
                tools: Vec::new(),
            }),
        };
        state.history.push(KiroHistoryMessage {
            user_input_message: Some(synthetic),
            assistant_response_message: None,
        });
    }

    state.history.push(KiroHistoryMessage {
        user_input_message: None,
        assistant_response_message: Some(assistant_msg),
    });

    if is_last {
        state.current_user = Some(KiroUserInputMessage {
            content: "Continue".to_string(),
            model_id: model_id.to_string(),
            origin: origin.to_string(),
            images: Vec::new(),
            user_input_message_context: None,
        });
    }
}

fn handle_tool_message(message: &Map<String, Value>, state: &mut MessageState) {
    if let Some(tool_result) = build_tool_result(message) {
        state.pending_tool_results.push(tool_result);
    }
}

fn finalize_pending_tools(model_id: &str, origin: &str, state: &mut MessageState) {
    if state.pending_tool_results.is_empty() {
        return;
    }
    state
        .current_tool_results
        .extend(state.pending_tool_results.drain(..));
    if state.current_user.is_some() {
        return;
    }
    state.current_user = Some(KiroUserInputMessage {
        content: "Tool results provided.".to_string(),
        model_id: model_id.to_string(),
        origin: origin.to_string(),
        images: Vec::new(),
        user_input_message_context: None,
    });
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
    let is_error = message
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut contents = extract_tool_result_contents(message);
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

fn extract_tool_result_contents(message: &Map<String, Value>) -> Vec<KiroTextContent> {
    if let Some(parts) = message.get("content_parts").and_then(Value::as_array) {
        let mut out = Vec::new();
        for part in parts {
            match part {
                Value::String(text) => {
                    if !text.is_empty() {
                        out.push(KiroTextContent { text: text.clone() });
                    }
                }
                Value::Object(obj) => {
                    if obj.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = obj.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                out.push(KiroTextContent {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    if content.is_empty() {
        return Vec::new();
    }
    vec![KiroTextContent {
        text: content.to_string(),
    }]
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
    let format = media_type.split('/').last().unwrap_or("").to_string();
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

pub(super) fn build_final_content(
    content: &str,
    system_prompt: &str,
    tool_results: &[KiroToolResult],
) -> String {
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

pub(super) fn deduplicate_tool_results(results: Vec<KiroToolResult>) -> Vec<KiroToolResult> {
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
