use serde_json::{json, Map, Value};

pub(super) struct ChatToolCall {
    pub(super) item_id: String,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}

pub(super) struct ResponsesOutput {
    pub(super) content_parts: Vec<Value>,
    pub(super) reasoning_text: String,
    pub(super) tool_calls: Vec<Value>,
    pub(super) annotations: Vec<Value>,
    pub(super) audio: Option<Value>,
    pub(super) thinking_blocks: Vec<Value>,
}

pub(super) fn extract_chat_tool_calls(value: &Value) -> Vec<ChatToolCall> {
    let Some(message) = extract_chat_first_message(value) else {
        return Vec::new();
    };
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|tool_calls| extract_chat_message_tool_calls(tool_calls))
        .unwrap_or_default();
    if !tool_calls.is_empty() {
        return tool_calls;
    }

    message
        .get("function_call")
        .and_then(Value::as_object)
        .and_then(extract_chat_message_legacy_function_call)
        .into_iter()
        .collect()
}

fn extract_chat_first_message(value: &Value) -> Option<&Map<String, Value>> {
    let choices = value.get("choices")?.as_array()?;
    let first = choices.first()?.as_object()?;
    first.get("message")?.as_object()
}

fn extract_chat_message_tool_calls(tool_calls: &[Value]) -> Vec<ChatToolCall> {
    let mut output = Vec::new();
    for call in tool_calls {
        let Some(call) = call.as_object() else {
            continue;
        };
        let call_id = call.get("id").and_then(Value::as_str).unwrap_or("");
        if call_id.is_empty() {
            continue;
        }

        let function = call.get("function").and_then(Value::as_object);
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let arguments = function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or("");

        output.push(ChatToolCall {
            item_id: format!("fc_{call_id}"),
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
    }
    output
}

fn extract_chat_message_legacy_function_call(
    function_call: &Map<String, Value>,
) -> Option<ChatToolCall> {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let arguments = function_call
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("");
    if name.is_empty() && arguments.is_empty() {
        return None;
    }
    Some(ChatToolCall {
        item_id: "fc_call_proxy".to_string(),
        call_id: "call_proxy".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    })
}

pub(super) fn extract_responses_output(value: &Value) -> ResponsesOutput {
    let Some(output) = value.get("output").and_then(Value::as_array) else {
        return ResponsesOutput {
            content_parts: Vec::new(),
            reasoning_text: String::new(),
            tool_calls: Vec::new(),
            annotations: Vec::new(),
            audio: None,
            thinking_blocks: Vec::new(),
        };
    };

    let mut content_parts = Vec::new();
    let mut reasoning_text = String::new();
    let mut tool_calls = Vec::new();
    let mut annotations = Vec::new();
    let mut audio = None;
    let mut thinking_blocks = Vec::new();

    for item in output {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                let summary_text = extract_reasoning_summary_text(item);
                if !summary_text.is_empty() {
                    reasoning_text.push_str(&summary_text);
                    thinking_blocks.push(json!({
                        "type": "thinking",
                        "thinking": summary_text
                    }));
                }
                if let Some(encrypted_content) = item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    thinking_blocks.push(json!({
                        "type": "redacted_thinking",
                        "data": encrypted_content
                    }));
                }
            }
            Some("message") => {
                if item.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(content) = item.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for part in content {
                    if let Some(part_obj) = part.as_object() {
                        let part_type = part_obj.get("type").and_then(Value::as_str);
                        if part_type == Some("reasoning_text") {
                            if let Some(text) = part_obj.get("text").and_then(Value::as_str) {
                                reasoning_text.push_str(text);
                                thinking_blocks.push(json!({
                                    "type": "thinking",
                                    "thinking": text
                                }));
                            }
                        }
                        if part_type == Some("output_text") {
                            if let Some(part_annotations) =
                                part_obj.get("annotations").and_then(Value::as_array)
                            {
                                annotations.extend(part_annotations.iter().cloned());
                            }
                        }
                        if part_type == Some("output_audio") && audio.is_none() {
                            audio = part_obj.get("audio").cloned();
                        }
                    }
                    content_parts.push(part.clone());
                }
            }
            Some("function_call") => {
                if let Some(tool_call) = extract_responses_tool_call(item) {
                    tool_calls.push(tool_call);
                }
            }
            _ => {}
        }
    }

    ResponsesOutput {
        content_parts,
        reasoning_text,
        tool_calls,
        annotations,
        audio,
        thinking_blocks,
    }
}

fn extract_reasoning_summary_text(item: &Map<String, Value>) -> String {
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

fn extract_responses_tool_call(item: &Map<String, Value>) -> Option<Value> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    let id = if !call_id.is_empty() {
        call_id.to_string()
    } else if !item_id.is_empty() {
        item_id.to_string()
    } else {
        "call_proxy".to_string()
    };
    if name.is_empty() && arguments.is_empty() && id == "call_proxy" {
        return None;
    }
    Some(json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    }))
}
