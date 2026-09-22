use bytes::Bytes;
use serde_json::Value;

use super::log::{TokenUsage, UsageSnapshot};
use super::pricing::BillableUsage;
use token_proxy_protocol::sse::SseEventParser;

pub struct SseUsageCollector {
    parser: SseEventParser,
    snapshot: UsageSnapshot,
}

impl SseUsageCollector {
    pub fn new() -> Self {
        Self {
            parser: SseEventParser::new(),
            snapshot: UsageSnapshot::default(),
        }
    }

    pub fn push_chunk(&mut self, chunk: &[u8]) {
        let snapshot = &mut self.snapshot;
        self.parser
            .push_chunk(chunk, |data| update_usage(snapshot, &data));
    }

    pub fn finish(&mut self) -> UsageSnapshot {
        let snapshot = &mut self.snapshot;
        self.parser.finish(|data| update_usage(snapshot, &data));
        self.snapshot.clone()
    }
}

impl Default for SseUsageCollector {
    fn default() -> Self {
        Self::new()
    }
}

pub fn extract_usage_from_response(bytes: &Bytes) -> UsageSnapshot {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return UsageSnapshot::default();
    };
    let mut snapshot = snapshot_from_envelope(&value).unwrap_or_default();
    // 没有 usage 的错误体也要留下响应模型，后面才能标「模型不一致」。
    snapshot.response_model = response_model_from_envelope(&value);
    snapshot
}

pub fn extract_usage_from_stored_json(raw: &str) -> Option<UsageSnapshot> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    snapshot_from_envelope(&value).or_else(|| {
        if contains_usage_fields(&value) {
            Some(snapshot_from_usage_value(&value))
        } else if contains_usage_metadata_fields(&value) {
            Some(snapshot_from_usage_metadata_value(&value))
        } else {
            None
        }
    })
}

fn contains_usage_fields(value: &Value) -> bool {
    [
        "input_tokens",
        "prompt_tokens",
        "output_tokens",
        "completion_tokens",
        "total_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "cache_write_tokens",
        "cached_creation_tokens",
    ]
    .iter()
    .any(|field| value.get(*field).is_some())
}

fn contains_usage_metadata_fields(value: &Value) -> bool {
    [
        "promptTokenCount",
        "candidatesTokenCount",
        "thoughtsTokenCount",
        "totalTokenCount",
        "cachedContentTokenCount",
    ]
    .iter()
    .any(|field| value.get(*field).is_some())
}

fn extract_usage_from_event(value: &Value) -> Option<UsageSnapshot> {
    snapshot_from_envelope(value)
}

fn snapshot_from_envelope(value: &Value) -> Option<UsageSnapshot> {
    let service_tier = service_tier_from_envelope(value);
    let mut snapshot = if let Some(usage) = value
        .get("response")
        .and_then(|response| response.get("usage"))
    {
        snapshot_from_usage_value(usage)
    } else if let Some(usage) = value.get("usage") {
        snapshot_from_usage_value(usage)
    } else if let Some(usage) = value
        .get("message")
        .and_then(|message| message.get("usage"))
    {
        snapshot_from_usage_value(usage)
    } else if let Some(metadata) = value.get("usageMetadata") {
        snapshot_from_usage_metadata_value(metadata)
    } else {
        let metadata = value
            .get("response")
            .and_then(|response| response.get("usageMetadata"))?;
        snapshot_from_usage_metadata_value(metadata)
    };
    snapshot.service_tier = service_tier.or(snapshot.service_tier);
    Some(snapshot)
}

fn snapshot_from_usage_value(value: &Value) -> UsageSnapshot {
    let raw_input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("prompt_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let raw_output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("completion_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let cache_read_tokens = cache_read_tokens_from_usage_value(value);
    let aggregate_cache_write = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get("input_tokens_details")
                .and_then(|details| details.get("cache_write_tokens"))
                .and_then(Value::as_u64)
        })
        .or_else(|| {
            value
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cache_write_tokens"))
                .and_then(Value::as_u64)
        })
        .or_else(|| {
            value
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_creation_tokens"))
                .and_then(Value::as_u64)
        })
        .or_else(|| {
            value
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_creation_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let cache_creation = value.get("cache_creation");
    let cache_write_5m_tokens = cache_creation
        .and_then(|details| details.get("ephemeral_5m_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write_1h_tokens = cache_creation
        .and_then(|details| details.get("ephemeral_1h_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write_tokens = aggregate_cache_write
        .saturating_sub(cache_write_5m_tokens)
        .saturating_sub(cache_write_1h_tokens);
    let image_input_tokens = detail_tokens(
        value,
        &["input_tokens_details", "prompt_tokens_details"],
        "image_tokens",
    );
    let image_output_tokens = detail_tokens(
        value,
        &["output_tokens_details", "completion_tokens_details"],
        "image_tokens",
    );
    let anthropic_usage = value.get("cache_read_input_tokens").is_some()
        || value.get("cache_creation_input_tokens").is_some();
    let uncached_input_tokens = if anthropic_usage {
        raw_input_tokens.saturating_sub(image_input_tokens)
    } else {
        raw_input_tokens
            .saturating_sub(cache_read_tokens)
            .saturating_sub(aggregate_cache_write)
            .saturating_sub(image_input_tokens)
    };
    let output_tokens = raw_output_tokens.saturating_sub(image_output_tokens);
    let billable_usage = BillableUsage {
        uncached_input_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
        output_tokens,
        image_input_tokens,
        image_output_tokens,
    };
    let total_input_tokens = billable_usage.total_input_tokens();
    let total_output_tokens = output_tokens.saturating_add(image_output_tokens);
    let has_input = value.get("input_tokens").is_some()
        || value.get("prompt_tokens").is_some()
        || value.get("cache_read_input_tokens").is_some()
        || value.get("cache_creation_input_tokens").is_some()
        || value
            .get("input_tokens_details")
            .and_then(|details| details.get("cache_write_tokens"))
            .is_some()
        || value
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_creation_tokens"))
            .is_some()
        || value
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cache_write_tokens"))
            .is_some()
        || value
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_creation_tokens"))
            .is_some();
    let has_output =
        value.get("output_tokens").is_some() || value.get("completion_tokens").is_some();
    let has_usage = has_input || has_output || value.get("total_tokens").is_some();

    UsageSnapshot {
        usage: has_usage.then(|| TokenUsage {
            input_tokens: has_input.then_some(total_input_tokens),
            output_tokens: has_output.then_some(total_output_tokens),
            total_tokens: value
                .get("total_tokens")
                .and_then(Value::as_u64)
                .or_else(|| {
                    (has_input && has_output)
                        .then_some(total_input_tokens.saturating_add(total_output_tokens))
                }),
        }),
        billable_usage,
        service_tier: value
            .get("service_tier")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase),
        usage_json: Some(value.clone()),
        response_model: None,
    }
}

fn snapshot_from_usage_metadata_value(value: &Value) -> UsageSnapshot {
    let raw_input_tokens = value
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let raw_output_tokens = token_proxy_protocol::gemini_usage::output_tokens(value);
    let cache_read_tokens = value
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let billable_usage = BillableUsage {
        uncached_input_tokens: raw_input_tokens.saturating_sub(cache_read_tokens),
        cache_read_tokens,
        output_tokens: raw_output_tokens,
        ..BillableUsage::default()
    };
    let has_usage = value.get("promptTokenCount").is_some()
        || value.get("candidatesTokenCount").is_some()
        || value.get("thoughtsTokenCount").is_some()
        || value.get("totalTokenCount").is_some();
    UsageSnapshot {
        usage: has_usage.then(|| TokenUsage {
            input_tokens: Some(raw_input_tokens),
            output_tokens: Some(raw_output_tokens),
            total_tokens: value
                .get("totalTokenCount")
                .and_then(Value::as_u64)
                .or_else(|| Some(raw_input_tokens.saturating_add(raw_output_tokens))),
        }),
        billable_usage,
        service_tier: None,
        usage_json: Some(value.clone()),
        response_model: None,
    }
}

/// 各协议响应信封上的模型字段。优先嵌套的 response/message，避免把外层回显当成上游实际模型。
fn response_model_from_envelope(value: &Value) -> Option<String> {
    value
        .get("response")
        .and_then(|response| model_string(response.get("model")))
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| model_string(message.get("model")))
        })
        .or_else(|| model_string(value.get("modelVersion")))
        .or_else(|| model_string(value.get("model")))
}

fn model_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn cache_read_tokens_from_usage_value(value: &Value) -> u64 {
    value
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .or_else(|| {
            value
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .or_else(|| value.get("cached_tokens").and_then(Value::as_u64))
        .unwrap_or(0)
}

fn detail_tokens(value: &Value, detail_fields: &[&str], token_field: &str) -> u64 {
    detail_fields
        .iter()
        .find_map(|detail_field| {
            value
                .get(*detail_field)
                .and_then(|details| details.get(token_field))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
}

fn service_tier_from_envelope(value: &Value) -> Option<String> {
    value
        .get("service_tier")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("service_tier"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn update_usage(snapshot: &mut UsageSnapshot, data: &str) {
    if data == "[DONE]" {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    // 后到的事件覆盖先到的。response.completed 一般在流末尾，带最终模型。
    if let Some(model) = response_model_from_envelope(&value) {
        snapshot.response_model = Some(model);
    }
    let Some(mut updated) = extract_usage_from_event(&value) else {
        return;
    };
    updated.response_model = snapshot.response_model.clone();
    if updated.usage_json.is_some() {
        *snapshot = updated;
    }
}

#[cfg(test)]
mod tests;
