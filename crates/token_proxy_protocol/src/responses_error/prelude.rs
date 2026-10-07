use serde_json::Value;

use super::{
    protocol_error, responses_stream_error, truncate_event_text, ResponsesPreludeDecision,
};
use crate::sse::SseEventParser;

const MAX_PRELUDE_BYTES: usize = 1024 * 1024;
const MAX_PRELUDE_EVENTS: usize = 128;

/// 只缓存明确没有执行副作用的空事件；一旦交付流，后续错误永远不能重放。
pub struct ResponsesPreludeInspector {
    parser: SseEventParser,
    bytes: usize,
    events: usize,
    released: bool,
}

impl Default for ResponsesPreludeInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponsesPreludeInspector {
    pub fn new() -> Self {
        Self {
            parser: SseEventParser::new(),
            bytes: 0,
            events: 0,
            released: false,
        }
    }

    pub fn inspect_chunk(&mut self, chunk: &[u8]) -> ResponsesPreludeDecision {
        if self.released {
            return ResponsesPreludeDecision::ReadyForPassThrough;
        }
        self.bytes = self.bytes.saturating_add(chunk.len());
        if self.bytes >= MAX_PRELUDE_BYTES {
            return self.release_budget();
        }
        let mut events = Vec::new();
        if let Err(error) = self.parser.push_chunk(chunk, |data| events.push(data)) {
            return ResponsesPreludeDecision::RetryableError(protocol_error(error.to_string()));
        }
        for data in events {
            self.events += 1;
            if self.events > MAX_PRELUDE_EVENTS {
                return self.release_budget();
            }
            let decision = inspect(&data);
            match decision {
                ResponsesPreludeDecision::Pending => {}
                ResponsesPreludeDecision::ReadyForPassThrough => {
                    self.released = true;
                    return decision;
                }
                _ => return decision,
            }
        }
        ResponsesPreludeDecision::Pending
    }

    fn release_budget(&mut self) -> ResponsesPreludeDecision {
        self.released = true;
        tracing::debug!(
            bytes = self.bytes,
            events = self.events,
            "Responses prelude budget reached; releasing buffered stream"
        );
        ResponsesPreludeDecision::ReadyForPassThrough
    }
}

fn inspect(data: &str) -> ResponsesPreludeDecision {
    if data == "[DONE]" {
        return ResponsesPreludeDecision::ReadyForPassThrough;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return ResponsesPreludeDecision::RetryableError(protocol_error(format!(
            "OpenAI Responses upstream emitted invalid JSON stream event: {}",
            truncate_event_text(data)
        )));
    };
    if let Some(error) = responses_stream_error(&value) {
        return if error.retryable_before_output {
            ResponsesPreludeDecision::RetryableError(error)
        } else {
            ResponsesPreludeDecision::ReadyForPassThrough
        };
    }
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return ResponsesPreludeDecision::RetryableError(protocol_error(format!(
            "OpenAI Responses upstream emitted malformed stream event: {}",
            truncate_event_text(data)
        )));
    };
    if kind == "response.incomplete" {
        let response = value.get("response").unwrap_or(&value);
        let empty = response
            .get("output")
            .is_none_or(|output| output.as_array().is_some_and(Vec::is_empty));
        if empty
            && response
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                == Some(0)
        {
            tracing::debug!("retrying empty Responses incomplete before business output");
            return ResponsesPreludeDecision::RetryableError(protocol_error(
                "OpenAI Responses upstream returned an empty incomplete response".to_string(),
            ));
        }
    }
    let bufferable = match kind {
        "response.created"
        | "response.in_progress"
        | "codex.rate_limits"
        | "codex.response.metadata"
        | "keepalive" => true,
        "response.output_item.added" => value.get("item").is_some_and(empty_item),
        "response.content_part.added" | "response.reasoning_summary_part.added" => {
            value.get("part").is_some_and(empty_part)
        }
        // 空字符串才是空输出；空格仍是模型已生成的正文。
        "response.output_text.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning_summary_text.delta"
        | "response.function_call_arguments.delta"
        | "response.custom_tool_call_input.delta" => {
            value.get("delta").and_then(Value::as_str) == Some("")
        }
        _ => false,
    };
    if bufferable {
        ResponsesPreludeDecision::Pending
    } else {
        ResponsesPreludeDecision::ReadyForPassThrough
    }
}

fn empty_item(item: &Value) -> bool {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => empty_parts(item.get("content")),
        Some("reasoning") => {
            item.get("encrypted_content")
                .is_none_or(|value| value.as_str() == Some(""))
                && empty_parts(item.get("summary"))
                && empty_parts(item.get("content"))
        }
        Some("function_call") => item
            .get("arguments")
            .is_none_or(|value| value.as_str() == Some("")),
        Some("custom_tool_call") => item
            .get("input")
            .is_none_or(|value| value.as_str() == Some("")),
        _ => false,
    }
}

fn empty_parts(parts: Option<&Value>) -> bool {
    parts.is_none_or(|parts| {
        parts
            .as_array()
            .is_some_and(|parts| parts.iter().all(empty_part))
    })
}

fn empty_part(part: &Value) -> bool {
    let field = match part.get("type").and_then(Value::as_str) {
        Some("output_text" | "summary_text" | "text" | "reasoning_text") => "text",
        Some("refusal") => "refusal",
        _ => return false,
    };
    part.get(field)
        .is_none_or(|value| value.as_str() == Some(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(inspector: &mut ResponsesPreludeInspector, event: Value) -> ResponsesPreludeDecision {
        inspector.inspect_chunk(format!("data: {event}\n\n").as_bytes())
    }

    #[test]
    fn empty_announcements_allow_overload_retry_but_effectful_events_do_not() {
        let failure = json!({"type":"response.failed","response":{"error":{"code":"server_is_overloaded","message":"overloaded"}}});
        for item in [
            json!({"type":"message","content":[]}),
            json!({"type":"function_call","arguments":""}),
            json!({"type":"reasoning","summary":[]}),
        ] {
            let mut inspector = ResponsesPreludeInspector::new();
            assert_eq!(
                feed(
                    &mut inspector,
                    json!({"type":"response.output_item.added","item":item})
                ),
                ResponsesPreludeDecision::Pending
            );
            assert!(matches!(
                feed(&mut inspector, failure.clone()),
                ResponsesPreludeDecision::RetryableError(_)
            ));
        }
        for item in [
            json!({"type":"web_search_call"}),
            json!({"type":"shell_call"}),
            json!({"type":"reasoning","encrypted_content":"signature"}),
            json!({"type":"message","content":[{"type":"output_audio"}]}),
        ] {
            let mut inspector = ResponsesPreludeInspector::new();
            assert_eq!(
                feed(
                    &mut inspector,
                    json!({"type":"response.output_item.added","item":item})
                ),
                ResponsesPreludeDecision::ReadyForPassThrough
            );
            assert_eq!(
                feed(&mut inspector, failure.clone()),
                ResponsesPreludeDecision::ReadyForPassThrough
            );
        }
    }

    #[test]
    fn bounded_partial_frames_and_heartbeats_release_without_replay() {
        let mut inspector = ResponsesPreludeInspector::new();
        assert_eq!(
            inspector.inspect_chunk(&vec![b' '; MAX_PRELUDE_BYTES]),
            ResponsesPreludeDecision::ReadyForPassThrough
        );
        let mut inspector = ResponsesPreludeInspector::new();
        for _ in 0..MAX_PRELUDE_EVENTS {
            assert_eq!(
                feed(&mut inspector, json!({"type":"keepalive"})),
                ResponsesPreludeDecision::Pending
            );
        }
        assert_eq!(
            feed(&mut inspector, json!({"type":"keepalive"})),
            ResponsesPreludeDecision::ReadyForPassThrough
        );
    }
}
