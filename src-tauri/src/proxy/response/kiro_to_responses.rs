use axum::body::Bytes;
use super::super::log::UsageSnapshot;
use super::kiro_to_responses_helpers::{
    apply_usage_fallback,
    build_response_object,
    usage_from_kiro,
    usage_json_from_kiro,
};

pub(super) use super::kiro_to_responses_stream::stream_kiro_to_responses;

pub(super) fn convert_kiro_response(
    bytes: &Bytes,
    model: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> Result<Bytes, String> {
    let parsed = crate::proxy::kiro::parse_event_stream(bytes)
        .map_err(|message| format!("Failed to parse Kiro response: {message}"))?;
    let mut usage = parsed.usage.clone();
    apply_usage_fallback(
        &mut usage,
        model,
        estimated_input_tokens,
        &parsed.content,
        &parsed.reasoning,
    );
    let now_ms = super::now_ms();
    let response_id = format!("resp_{now_ms}");
    let created_at = (now_ms / 1000) as i64;
    let response = build_response_object(
        parsed.content,
        parsed.reasoning,
        parsed.tool_uses,
        usage,
        parsed.stop_reason.as_deref(),
        model,
        response_id,
        created_at,
    );
    serde_json::to_vec(&response)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

pub(super) fn extract_kiro_usage_snapshot(
    bytes: &Bytes,
    model: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> Option<UsageSnapshot> {
    let parsed = crate::proxy::kiro::parse_event_stream(bytes).ok()?;
    let mut usage = parsed.usage.clone();
    apply_usage_fallback(
        &mut usage,
        model,
        estimated_input_tokens,
        &parsed.content,
        &parsed.reasoning,
    );
    let usage_snapshot = UsageSnapshot {
        usage: usage_from_kiro(&usage),
        cached_tokens: None,
        usage_json: usage_json_from_kiro(&usage),
    };
    if usage_snapshot.usage.is_none()
        && usage_snapshot.usage_json.is_none()
        && usage_snapshot.cached_tokens.is_none()
    {
        return None;
    }
    Some(usage_snapshot)
}
