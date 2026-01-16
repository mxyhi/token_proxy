use serde_json::{json, Map, Value};

pub(super) struct ChatToolCall {
    pub(super) item_id: String,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}

pub(super) struct ResponsesOutput {
    pub(super) content_parts: Vec<Value>,
    pub(super) tool_calls: Vec<Value>,
}

pub(super) fn extract_chat_choice_text(value: &Value) -> Option<String> {
    let choices = value.get("choices")?.as_array()?;
    let first = choices.first()?.as_object()?;
    let message = first.get("message")?.as_object()?;
    message.get("content")?.as_str().map(|text| text.to_string())
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

fn extract_chat_message_legacy_function_call(function_call: &Map<String, Value>) -> Option<ChatToolCall> {
    let name = function_call.get("name").and_then(Value::as_str).unwrap_or("");
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
            tool_calls: Vec::new(),
        };
    };

    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for item in output {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if item.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(content) = item.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for part in content {
                    if let Some(part_obj) = part.as_object() {
                        let _ = part_obj.get("type").and_then(Value::as_str);
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
        tool_calls,
    }
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
