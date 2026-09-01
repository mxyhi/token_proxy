use serde_json::{json, Map, Value};

use super::super::super::log::TokenUsage;

pub(super) struct AnthropicCacheUsage {
    pub(super) read_tokens: u64,
    pub(super) creation_tokens: u64,
}

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
    WebSearch {
        id: String,
        output_index: u64,
        query: String,
        results: Option<Value>,
    },
}

pub(super) fn usage_to_value(usage: TokenUsage, cache_usage: Option<AnthropicCacheUsage>) -> Value {
    let input_tokens = usage.input_tokens.unwrap_or(0);
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let (cache_read_tokens, cache_creation_tokens) = cache_usage
        .map(|usage| (usage.read_tokens, usage.creation_tokens))
        .unwrap_or((0, 0));
    let total_input_tokens = input_tokens
        .saturating_add(cache_read_tokens)
        .saturating_add(cache_creation_tokens);
    let total_tokens = total_input_tokens.saturating_add(output_tokens);

    let mut value = json!({
        "input_tokens": total_input_tokens,
        "output_tokens": output_tokens,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": total_tokens
    });
    if cache_read_tokens > 0 || cache_creation_tokens > 0 {
        let mut details = Map::new();
        if cache_read_tokens > 0 {
            details.insert("cached_tokens".to_string(), json!(cache_read_tokens));
        }
        if cache_creation_tokens > 0 {
            details.insert(
                "cached_creation_tokens".to_string(),
                json!(cache_creation_tokens),
            );
            tracing::debug!(
                cache_creation_tokens,
                "mapped streaming cache creation usage"
            );
        }
        value
            .as_object_mut()
            .expect("usage value is object")
            .insert("input_tokens_details".to_string(), Value::Object(details));
    }
    value
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
        OutputItemSnapshot::WebSearch {
            id, query, results, ..
        } => {
            let mut item = json!({
                "id": id,
                "type": "web_search_call",
                "status": "completed",
                "action": { "type": "search", "query": query }
            });
            if let Some(results) = results {
                item.as_object_mut()
                    .expect("web search item is object")
                    .insert("results".to_string(), results.clone());
            }
            item
        }
    }
}
