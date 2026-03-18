use serde_json::{json, Value};

use super::super::super::log::TokenUsage;

pub(super) enum OutputItemSnapshot {
    Reasoning {
        id: String,
        output_index: u64,
        text: String,
        encrypted_content: Option<String>,
    },
    Message {
        id: String,
        output_index: u64,
        text: String,
    },
    FunctionCall {
        id: String,
        output_index: u64,
        call_id: String,
        name: String,
        arguments: String,
    },
}

pub(super) fn usage_to_value(usage: TokenUsage, cached_tokens: Option<u64>) -> Value {
    let input_tokens = usage.input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let total_tokens = usage
        .total_tokens
        .or_else(|| input_tokens.checked_add(output_tokens))
        .unwrap_or(0);
    let cached_tokens = cached_tokens.unwrap_or(0);

    json!({
        "input_tokens": input_tokens,
        "input_tokens_details": { "cached_tokens": cached_tokens },
        "output_tokens": output_tokens,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": total_tokens
    })
}

pub(super) fn snapshot_to_output_item(
    snapshot: &OutputItemSnapshot,
    response_status: &str,
) -> Value {
    match snapshot {
        OutputItemSnapshot::Reasoning {
            id,
            text,
            encrypted_content,
            ..
        } => {
            let mut item = json!({
                "id": id,
                "type": "reasoning",
                "status": response_status,
                "summary": []
            });
            if let Some(item) = item.as_object_mut() {
                if !text.is_empty() {
                    item.insert(
                        "summary".to_string(),
                        json!([{ "type": "summary_text", "text": text }]),
                    );
                }
                if let Some(encrypted_content) = encrypted_content {
                    item.insert(
                        "encrypted_content".to_string(),
                        Value::String(encrypted_content.clone()),
                    );
                }
            }
            item
        }
        OutputItemSnapshot::Message { id, text, .. } => json!({
            "id": id,
            "type": "message",
            "status": response_status,
            "role": "assistant",
            "content": [
                { "type": "output_text", "text": text, "annotations": [] }
            ]
        }),
        OutputItemSnapshot::FunctionCall {
            id,
            call_id,
            name,
            arguments,
            ..
        } => json!({
            "id": id,
            "type": "function_call",
            "status": "completed",
            "call_id": call_id,
            "name": name,
            "arguments": arguments
        }),
    }
}
