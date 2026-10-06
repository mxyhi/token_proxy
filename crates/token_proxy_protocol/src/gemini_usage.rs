//! Gemini 思考 token 是输出的一部分，转换和持久化必须使用同一计数口径。
use serde_json::{json, Value};

/// usageMetadata 是累计快照而非增量；缺失字段保留，显式零值也必须覆盖。
pub fn merge_snapshot(accumulated: &mut Value, incoming: &Value) {
    let Some(fields) = incoming.as_object() else {
        return;
    };
    if !accumulated.is_object() {
        *accumulated = json!({});
    }
    let accumulated = accumulated.as_object_mut().expect("usage object");
    for (field, value) in fields {
        accumulated.insert(field.clone(), value.clone());
    }
}

pub fn output_tokens(usage: &Value) -> u64 {
    count(usage, "candidatesTokenCount").saturating_add(count(usage, "thoughtsTokenCount"))
}

pub fn to_chat(usage: &Value) -> Value {
    let input = count(usage, "promptTokenCount");
    let output = output_tokens(usage);
    let thoughts = count(usage, "thoughtsTokenCount");
    let mut mapped = json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": usage.get("totalTokenCount").and_then(Value::as_u64)
            .unwrap_or_else(|| input.saturating_add(output)),
    });
    if usage
        .get("thoughtsTokenCount")
        .and_then(Value::as_u64)
        .is_some()
    {
        mapped["completion_tokens_details"] = json!({"reasoning_tokens": thoughts});
    }
    if let Some(cached) = usage.get("cachedContentTokenCount").and_then(Value::as_u64) {
        mapped["cached_tokens"] = json!(cached);
        mapped["prompt_tokens_details"] = json!({"cached_tokens": cached});
    }
    tracing::trace!(
        input_tokens = input,
        output_tokens = output,
        reasoning_tokens = thoughts,
        "normalized Gemini usage including thinking"
    );
    mapped
}

fn count(usage: &Value, field: &str) -> u64 {
    usage.get(field).and_then(Value::as_u64).unwrap_or(0)
}
