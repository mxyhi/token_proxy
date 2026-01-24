use axum::body::Bytes;
use serde_json::Value;

use super::sse::SseEventParser;
use super::log::{TokenUsage, UsageSnapshot};

pub(crate) struct SseUsageCollector {
    parser: SseEventParser,
    snapshot: UsageSnapshot,
}

impl SseUsageCollector {
    pub(crate) fn new() -> Self {
        Self {
            parser: SseEventParser::new(),
            snapshot: UsageSnapshot {
                usage: None,
                cached_tokens: None,
                usage_json: None,
            },
        }
    }

    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) {
        let snapshot = &mut self.snapshot;
        self.parser
            .push_chunk(chunk, |data| update_usage(snapshot, &data));
    }

    pub(crate) fn finish(&mut self) -> UsageSnapshot {
        let snapshot = &mut self.snapshot;
        self.parser.finish(|data| update_usage(snapshot, &data));
        self.snapshot.clone()
    }
}

pub(crate) fn extract_usage_from_response(bytes: &Bytes) -> UsageSnapshot {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return UsageSnapshot {
            usage: None,
            cached_tokens: None,
            usage_json: None,
        };
    };

    if let Some(usage) = value.get("usage") {
        return snapshot_from_usage_value(usage);
    }

    value
        .get("usageMetadata")
        .map(snapshot_from_usage_metadata_value)
        .unwrap_or(UsageSnapshot {
            usage: None,
            cached_tokens: None,
            usage_json: None,
        })
}

fn extract_usage_from_event(value: &Value) -> Option<UsageSnapshot> {
    if let Some(usage) = value.get("usage") {
        return Some(snapshot_from_usage_value(usage));
    }

    if let Some(usage) = value.get("message").and_then(|message| message.get("usage")) {
        return Some(snapshot_from_usage_value(usage));
    }

    if let Some(usage) = value.get("response").and_then(|response| response.get("usage")) {
        return Some(snapshot_from_usage_value(usage));
    }

    if let Some(metadata) = value.get("usageMetadata") {
        return Some(snapshot_from_usage_metadata_value(metadata));
    }

    value
        .get("response")
        .and_then(|response| response.get("usageMetadata"))
        .map(snapshot_from_usage_metadata_value)
}

fn usage_from_value(value: &Value) -> Option<TokenUsage> {
    // Normalize both OpenAI Responses usage (`input_tokens`/`output_tokens`) and
    // Chat Completions usage (`prompt_tokens`/`completion_tokens`) into a single shape.
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("prompt_tokens").and_then(Value::as_u64));
    let output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("completion_tokens").and_then(Value::as_u64));
    let total_tokens = value.get("total_tokens").and_then(Value::as_u64);
    if input_tokens.is_some() || output_tokens.is_some() || total_tokens.is_some() {
        return Some(TokenUsage {
            input_tokens,
            output_tokens,
            total_tokens,
        });
    }
    None
}

fn gemini_usage_from_value(value: &Value) -> Option<TokenUsage> {
    // Gemini API 返回 `usageMetadata`：prompt/candidates/total token 计数。
    let input_tokens = value.get("promptTokenCount").and_then(Value::as_u64);
    let output_tokens = value
        .get("candidatesTokenCount")
        .and_then(Value::as_u64);
    let total_tokens = value.get("totalTokenCount").and_then(Value::as_u64);

    if input_tokens.is_some() || output_tokens.is_some() || total_tokens.is_some() {
        return Some(TokenUsage {
            input_tokens,
            output_tokens,
            total_tokens,
        });
    }
    None
}

fn snapshot_from_usage_value(value: &Value) -> UsageSnapshot {
    UsageSnapshot {
        usage: usage_from_value(value),
        cached_tokens: cached_tokens_from_usage_value(value),
        usage_json: Some(value.clone()),
    }
}

fn snapshot_from_usage_metadata_value(value: &Value) -> UsageSnapshot {
    UsageSnapshot {
        usage: gemini_usage_from_value(value),
        cached_tokens: None,
        usage_json: Some(value.clone()),
    }
}

fn cached_tokens_from_usage_value(value: &Value) -> Option<u64> {
    let cache_read = value.get("cache_read_input_tokens").and_then(Value::as_u64);
    let cache_creation = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    if cache_read.is_some() || cache_creation.is_some() {
        return match (cache_read, cache_creation) {
            (Some(left), Some(right)) => left.checked_add(right),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
    }

    value
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .or_else(|| value.get("cached_tokens").and_then(Value::as_u64))
}

fn update_usage(snapshot: &mut UsageSnapshot, data: &str) {
    if data == "[DONE]" {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    let Some(updated) = extract_usage_from_event(&value) else {
        return;
    };
    if updated.usage_json.is_some() {
        snapshot.usage_json = updated.usage_json;
        snapshot.usage = updated.usage;
        snapshot.cached_tokens = updated.cached_tokens;
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "usage.test.rs"]
mod tests;
