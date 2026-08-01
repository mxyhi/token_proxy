use serde_json::Value;

/// 跨 Provider 保留推理摘要可见性；缺失与显式关闭必须保持不同语义。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryVisibility {
    #[default]
    Unspecified,
    Disabled,
    Enabled,
}

impl SummaryVisibility {
    pub fn from_responses_reasoning(reasoning: Option<&Value>) -> Self {
        let Some(summary) = reasoning
            .and_then(Value::as_object)
            .and_then(|reasoning| reasoning.get("summary"))
        else {
            return Self::Unspecified;
        };
        match summary {
            Value::Null => Self::Disabled,
            Value::String(value) if value == "none" => Self::Disabled,
            Value::String(value) if matches!(value.as_str(), "auto" | "concise" | "detailed") => {
                Self::Enabled
            }
            _ => Self::Unspecified,
        }
    }

    pub fn from_anthropic_thinking(thinking: Option<&Value>) -> Self {
        let Some(thinking) = thinking.and_then(Value::as_object) else {
            return Self::Unspecified;
        };
        if !matches!(
            thinking.get("type").and_then(Value::as_str),
            Some("adaptive" | "enabled")
        ) {
            return Self::Unspecified;
        }
        match thinking.get("display").and_then(Value::as_str) {
            Some("summarized") => Self::Enabled,
            Some("omitted") => Self::Disabled,
            _ => Self::Unspecified,
        }
    }

    pub fn from_gemini_generation_config(generation_config: Option<&Value>) -> Self {
        match generation_config
            .and_then(Value::as_object)
            .and_then(|config| config.get("thinkingConfig"))
            .and_then(Value::as_object)
            .and_then(|thinking| thinking.get("includeThoughts"))
            .and_then(Value::as_bool)
        {
            Some(true) => Self::Enabled,
            Some(false) => Self::Disabled,
            None => Self::Unspecified,
        }
    }

    pub fn responses_summary(self) -> Option<Value> {
        match self {
            Self::Unspecified => None,
            Self::Disabled => Some(Value::Null),
            Self::Enabled => Some(Value::String("auto".to_string())),
        }
    }

    pub fn anthropic_display(self) -> Option<Value> {
        match self {
            Self::Unspecified => None,
            Self::Disabled => Some(Value::String("omitted".to_string())),
            Self::Enabled => Some(Value::String("summarized".to_string())),
        }
    }

    pub fn gemini_include_thoughts(self) -> Option<bool> {
        match self {
            Self::Unspecified => None,
            Self::Disabled => Some(false),
            Self::Enabled => Some(true),
        }
    }

    pub fn is_specified(self) -> bool {
        self != Self::Unspecified
    }
}

pub fn chat_finish_reason_from_responses(
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

pub fn chat_finish_reason_from_response_object(
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

pub fn responses_status_from_chat_finish_reason(
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

pub fn anthropic_stop_reason_from_chat_finish_reason(reason: &str) -> &'static str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

pub fn responses_status_from_anthropic_stop_reason(
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summary_visibility_parsers_preserve_three_states() {
        assert_eq!(
            SummaryVisibility::from_responses_reasoning(Some(&json!({ "summary": "concise" }))),
            SummaryVisibility::Enabled
        );
        assert_eq!(
            SummaryVisibility::from_responses_reasoning(Some(&json!({ "summary": null }))),
            SummaryVisibility::Disabled
        );
        assert_eq!(
            SummaryVisibility::from_anthropic_thinking(Some(
                &json!({ "type": "adaptive", "display": "omitted" })
            )),
            SummaryVisibility::Disabled
        );
        assert_eq!(
            SummaryVisibility::from_gemini_generation_config(Some(
                &json!({ "thinkingConfig": { "includeThoughts": true } })
            )),
            SummaryVisibility::Enabled
        );
        assert_eq!(
            SummaryVisibility::from_responses_reasoning(Some(&json!({ "effort": "high" }))),
            SummaryVisibility::Unspecified
        );
    }
}
