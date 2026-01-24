use axum::body::Bytes;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::proxy::kiro::{parse_event_stream, utils::random_uuid, KiroToolUse, KiroUsage};
use super::super::kiro_to_responses_helpers::{apply_usage_fallback, usage_json_from_kiro};

pub(crate) fn convert_kiro_response(
    bytes: &Bytes,
    model: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> Result<Bytes, String> {
    let parsed = parse_event_stream(bytes)
        .map_err(|message| format!("Failed to parse Kiro response: {message}"))?;
    let mut usage = parsed.usage.clone();
    apply_usage_fallback(
        &mut usage,
        model,
        estimated_input_tokens,
        &parsed.content,
        &parsed.reasoning,
    );
    let response = build_claude_response(
        parsed.content,
        parsed.reasoning,
        parsed.tool_uses,
        usage,
        parsed.stop_reason.as_deref(),
        model.unwrap_or("unknown"),
    );
    serde_json::to_vec(&response)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

pub(super) fn split_partial_tag(segment: &str, tag: &str) -> (String, String) {
    if tag.len() <= 1 || segment.is_empty() {
        return (segment.to_string(), String::new());
    }
    let max_len = std::cmp::min(segment.len(), tag.len() - 1);
    for len in (1..=max_len).rev() {
        if segment.ends_with(&tag[..len]) {
            let emit_end = segment.len() - len;
            return (segment[..emit_end].to_string(), segment[emit_end..].to_string());
        }
    }
    (segment.to_string(), String::new())
}

fn build_claude_response(
    content: String,
    reasoning: String,
    tool_uses: Vec<KiroToolUse>,
    usage: KiroUsage,
    stop_reason: Option<&str>,
    model: &str,
) -> Value {
    let mut blocks = Vec::new();
    if !reasoning.trim().is_empty() {
        blocks.push(json!({
            "type": "thinking",
            "thinking": reasoning,
            "signature": thinking_signature(&reasoning)
        }));
    }
    if !content.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": content }));
    }
    for tool_use in tool_uses.iter() {
        blocks.push(json!({
            "type": "tool_use",
            "id": tool_use.tool_use_id,
            "name": tool_use.name,
            "input": tool_use.input
        }));
    }
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "" }));
    }
    let stop_reason = stop_reason.unwrap_or_else(|| {
        if tool_uses.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        }
    });
    let usage_value = usage_json_from_kiro(&usage).unwrap_or_else(|| json!({
        "input_tokens": 0,
        "output_tokens": 0
    }));
    json!({
        "id": format!("msg_{}", random_uuid()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": blocks,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage_value
    })
}

fn thinking_signature(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    STANDARD.encode(hasher.finalize())
}
