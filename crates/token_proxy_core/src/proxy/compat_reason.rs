use serde_json::Value;

pub(crate) fn chat_finish_reason_from_responses(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
    has_tool_calls: bool,
) -> &'static str {
    // Prefer explicit incomplete reason, then status, then tool calls.
    if let Some(reason) = incomplete_reason {
        return map_responses_reason_to_chat_finish_reason(reason);
    }
    if matches!(status, Some("incomplete")) {
        return "length";
    }
    if has_tool_calls {
        return "tool_calls";
    }
    "stop"
}

pub(crate) fn chat_finish_reason_from_response_object(
    response: &serde_json::Map<String, Value>,
    has_tool_calls: bool,
) -> &'static str {
    let status = response.get("status").and_then(Value::as_str);
    let incomplete_reason = response
        .get("incomplete_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str);
    chat_finish_reason_from_responses(status, incomplete_reason, has_tool_calls)
}

pub(crate) fn responses_status_from_chat_finish_reason(
    finish_reason: Option<&str>,
) -> (Option<&'static str>, Option<&'static str>) {
    let Some(reason) = finish_reason else {
        return (None, None);
    };
    match reason {
        "length" => (Some("incomplete"), Some("max_tokens")),
        "content_filter" => (Some("incomplete"), Some("content_filter")),
        _ => (None, None),
    }
}

pub(crate) fn anthropic_stop_reason_from_chat_finish_reason(reason: &str) -> &'static str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

pub(crate) fn responses_status_from_anthropic_stop_reason(
    stop_reason: Option<&str>,
) -> (Option<&'static str>, Option<&'static str>) {
    let finish_reason = match stop_reason {
        Some("max_tokens") => Some("length"),
        Some("refusal") => Some("content_filter"),
        _ => None,
    };
    responses_status_from_chat_finish_reason(finish_reason)
}

fn map_responses_reason_to_chat_finish_reason(reason: &str) -> &'static str {
    match reason {
        "max_output_tokens" | "max_tokens" => "length",
        "content_filter" => "content_filter",
        "tool_calls" | "tool_use" => "tool_calls",
        "stop" | "stop_sequence" | "end_turn" => "stop",
        _ => "stop",
    }
}
