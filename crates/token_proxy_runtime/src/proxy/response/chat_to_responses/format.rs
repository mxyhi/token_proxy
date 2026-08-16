use serde_json::{json, Value};

use super::super::super::log::TokenUsage;

pub(super) enum OutputItemSnapshot {
    Reasoning {
        id: String,
        output_index: u64,
        text: String,
        encrypted_content: Option<String>,
        status: String,
    },
    Message {
        id: String,
        output_index: u64,
        text: String,
        audio: Option<Value>,
        status: String,
    },
    FunctionCall {
        id: String,
        output_index: u64,
        call_id: String,
        name: String,
        arguments: String,
        status: String,
    },
}

pub(super) fn usage_to_value(usage: TokenUsage) -> Value {
    let input_tokens = usage.input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let total_tokens = usage
        .total_tokens
        .or_else(|| input_tokens.checked_add(output_tokens))
        .unwrap_or(0);

    json!({
        "input_tokens": input_tokens,
        "input_tokens_details": { "cached_tokens": 0 },
        "output_tokens": output_tokens,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": total_tokens
    })
}

pub(super) fn snapshot_to_output_item(snapshot: &OutputItemSnapshot) -> Value {
    match snapshot {
        OutputItemSnapshot::Reasoning {
            id,
            text,
            encrypted_content,
            status,
            ..
        } => {
            let mut item = json!({
                "id": id,
                "type": "reasoning",
                "status": status,
                "summary": [
                    { "type": "summary_text", "text": text }
                ]
            });
            if let Some(item) = item.as_object_mut() {
                if let Some(encrypted_content) = encrypted_content {
                    item.insert(
                        "encrypted_content".to_string(),
                        Value::String(encrypted_content.clone()),
                    );
                }
            }
            item
        }
        OutputItemSnapshot::Message {
            id,
            text,
            audio,
            status,
            ..
        } => {
            let mut content = Vec::new();
            if !text.is_empty() {
                content.push(json!({
                    "type": "output_text",
                    "text": text,
                    "annotations": []
                }));
            }
            if let Some(audio) = audio {
                content.push(json!({
                    "type": "output_audio",
                    "audio": audio
                }));
            }
            json!({
                "id": id,
                "type": "message",
                "status": status,
                "role": "assistant",
                "content": content
            })
        }
        OutputItemSnapshot::FunctionCall {
            id,
            call_id,
            name,
            arguments,
            status,
            ..
        } => json!({
            "id": id,
            "type": "function_call",
            "status": status,
            "call_id": call_id,
            "name": name,
            "arguments": arguments
        }),
    }
}
