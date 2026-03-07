use serde_json::{json, Map, Value};

use super::super::kiro::KiroUsage;
use super::super::token_estimator;

pub(super) fn usage_from_kiro(usage: &KiroUsage) -> Option<super::super::log::TokenUsage> {
    if usage.input_tokens.is_none() && usage.output_tokens.is_none() && usage.total_tokens.is_none()
    {
        return None;
    }
    let total_tokens =
        usage
            .total_tokens
            .or_else(|| match (usage.input_tokens, usage.output_tokens) {
                (Some(input), Some(output)) => Some(input.saturating_add(output)),
                _ => None,
            });
    Some(super::super::log::TokenUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens,
    })
}

pub(super) fn usage_json_from_kiro(usage: &KiroUsage) -> Option<Value> {
    let input_tokens = usage.input_tokens?;
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let total_tokens = usage
        .total_tokens
        .or_else(|| input_tokens.checked_add(output_tokens))
        .unwrap_or(input_tokens);
    Some(json!({
        "input_tokens": input_tokens,
        "input_tokens_details": { "cached_tokens": 0 },
        "output_tokens": output_tokens,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": total_tokens
    }))
}

pub(super) fn apply_usage_fallback(
    usage: &mut KiroUsage,
    model: Option<&str>,
    estimated_input_tokens: Option<u64>,
    content: &str,
    reasoning: &str,
) {
    if usage.input_tokens.is_none() {
        if let Some(pct) = usage.context_usage_percentage {
            let input = ((pct * 200000.0) / 100.0).round() as u64;
            if input > 0 {
                usage.input_tokens = Some(input);
            }
        } else if let Some(estimate) = estimated_input_tokens {
            usage.input_tokens = Some(estimate);
        }
    }

    if usage.output_tokens.is_none() {
        if let (Some(total), Some(input)) = (usage.total_tokens, usage.input_tokens) {
            if total >= input {
                usage.output_tokens = Some(total - input);
            }
        }
    }

    if usage.output_tokens.is_none() {
        let mut output_text = String::new();
        output_text.push_str(content);
        if !reasoning.trim().is_empty() {
            output_text.push_str(reasoning);
        }
        if output_text.trim().is_empty() {
            return;
        }
        let estimated = token_estimator::estimate_text_tokens(model, &output_text);
        if estimated > 0 {
            usage.output_tokens = Some(estimated);
        }
    }

    if usage.total_tokens.is_none() {
        if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
            usage.total_tokens = Some(input.saturating_add(output));
        }
    }
}

pub(super) fn detect_event_type(event: &Map<String, Value>) -> &str {
    for key in [
        "assistantResponseEvent",
        "toolUseEvent",
        "reasoningContentEvent",
        "messageStopEvent",
        "message_stop",
        "messageMetadataEvent",
        "metadataEvent",
        "usageEvent",
        "usage",
        "metricsEvent",
        "meteringEvent",
        "supplementaryWebLinksEvent",
    ] {
        if event.contains_key(key) {
            return key;
        }
    }
    ""
}

pub(super) fn extract_error(event: &Map<String, Value>) -> Option<String> {
    if let Some(Value::String(err_type)) = event.get("_type") {
        let message = event.get("message").and_then(Value::as_str).unwrap_or("");
        return Some(format!("Kiro error: {err_type} {message}"));
    }
    if let Some(Value::String(kind)) = event.get("type") {
        if matches!(
            kind.as_str(),
            "error" | "exception" | "internalServerException"
        ) {
            let message = event.get("message").and_then(Value::as_str).unwrap_or("");
            if message.is_empty() {
                if let Some(Value::Object(err_obj)) = event.get("error") {
                    if let Some(text) = err_obj.get("message").and_then(Value::as_str) {
                        return Some(format!("Kiro error: {text}"));
                    }
                }
            }
            return Some(format!("Kiro error: {message}"));
        }
    }
    if event.contains_key("invalidStateEvent")
        || event
            .get("eventType")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "invalidStateEvent")
    {
        return Some("invalidStateEvent".to_string());
    }
    None
}

pub(super) fn update_stop_reason(event: &Map<String, Value>, stop_reason: &mut Option<String>) {
    if let Some(reason) = event.get("stop_reason").and_then(Value::as_str) {
        *stop_reason = Some(reason.to_string());
    }
    if let Some(reason) = event.get("stopReason").and_then(Value::as_str) {
        *stop_reason = Some(reason.to_string());
    }
}

pub(super) fn update_usage(event: &Map<String, Value>, usage: &mut KiroUsage) {
    if let Some(context_pct) = event.get("contextUsagePercentage").and_then(Value::as_f64) {
        usage.context_usage_percentage = Some(context_pct);
    }
    if let Some(tokens) = event.get("inputTokens").and_then(Value::as_u64) {
        usage.input_tokens = Some(tokens);
    }
    if let Some(tokens) = event.get("outputTokens").and_then(Value::as_u64) {
        usage.output_tokens = Some(tokens);
    }
    if let Some(tokens) = event.get("totalTokens").and_then(Value::as_u64) {
        usage.total_tokens = Some(tokens);
    }

    if let Some(metadata) = event.get("messageMetadataEvent").and_then(Value::as_object) {
        update_usage_from_metadata(metadata, usage);
    } else if let Some(metadata) = event.get("metadataEvent").and_then(Value::as_object) {
        update_usage_from_metadata(metadata, usage);
    }

    if let Some(usage_obj) = event.get("usage").and_then(Value::as_object) {
        update_usage_from_usage_obj(usage_obj, usage);
    }
    if let Some(usage_obj) = event.get("usageEvent").and_then(Value::as_object) {
        update_usage_from_usage_obj(usage_obj, usage);
    }

    if let Some(links) = event
        .get("supplementaryWebLinksEvent")
        .and_then(Value::as_object)
    {
        if let Some(tokens) = links.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = links.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }

    if let Some(metrics) = event.get("metricsEvent").and_then(Value::as_object) {
        if let Some(tokens) = metrics.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = metrics.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }

    if let Some(metering) = event.get("meteringEvent").and_then(Value::as_object) {
        if let Some(tokens) = metering.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = metering.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
        if let Some(tokens) = metering.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
    }
}

fn update_usage_from_metadata(metadata: &Map<String, Value>, usage: &mut KiroUsage) {
    if let Some(token_usage) = metadata.get("tokenUsage").and_then(Value::as_object) {
        if let Some(tokens) = token_usage.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage
            .get("uncachedInputTokens")
            .and_then(Value::as_u64)
        {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage
            .get("cacheReadInputTokens")
            .and_then(Value::as_u64)
        {
            let current = usage.input_tokens.unwrap_or(0);
            usage.input_tokens = Some(current + tokens);
        }
        if let Some(context_pct) = token_usage
            .get("contextUsagePercentage")
            .and_then(Value::as_f64)
        {
            usage.context_usage_percentage = Some(context_pct);
        }
    }

    if usage.input_tokens.is_none() {
        if let Some(tokens) = metadata.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
    }
    if usage.output_tokens.is_none() {
        if let Some(tokens) = metadata.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }
    if usage.total_tokens.is_none() {
        if let Some(tokens) = metadata.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
    }
}

fn update_usage_from_usage_obj(usage_obj: &Map<String, Value>, usage: &mut KiroUsage) {
    let input_tokens = usage_obj
        .get("input_tokens")
        .or_else(|| usage_obj.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage_obj
        .get("output_tokens")
        .or_else(|| usage_obj.get("completion_tokens"))
        .and_then(Value::as_u64);
    let total_tokens = usage_obj.get("total_tokens").and_then(Value::as_u64);

    if input_tokens.is_some() {
        usage.input_tokens = input_tokens;
    }
    if output_tokens.is_some() {
        usage.output_tokens = output_tokens;
    }
    if total_tokens.is_some() {
        usage.total_tokens = total_tokens;
    }
}
