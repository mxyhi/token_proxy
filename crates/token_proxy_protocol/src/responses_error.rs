use http::StatusCode;
use serde_json::{Map, Value};

use crate::sse::SseEventParser;

const INVALID_EVENT_LIMIT: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ResponsesPreludeDecision {
    Pending,
    RetryableError(ResponsesStreamError),
    ReadyForPassThrough,
}

pub struct ResponsesPreludeInspector {
    parser: SseEventParser,
}

impl ResponsesPreludeInspector {
    pub fn new() -> Self {
        Self {
            parser: SseEventParser::new(),
        }
    }

    pub fn inspect_chunk(&mut self, chunk: &[u8]) -> ResponsesPreludeDecision {
        let mut events = Vec::new();
        self.parser.push_chunk(chunk, |data| events.push(data));
        for data in events {
            match inspect_prelude_event(&data) {
                ResponsesPreludeDecision::Pending => {}
                decision => return decision,
            }
        }
        ResponsesPreludeDecision::Pending
    }
}

impl Default for ResponsesPreludeInspector {
    fn default() -> Self {
        Self::new()
    }
}

/// Responses 流把失败编码在 HTTP 200 的 SSE 事件里；这里集中保存协议错误语义，
/// 供 failover、格式转换和日志共用，避免各路径对同一事件得出不同结论。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponsesStreamError {
    pub message: String,
    pub error_type: String,
    pub code: Option<Value>,
    pub status: StatusCode,
    pub retryable_before_output: bool,
}

fn inspect_prelude_event(data: &str) -> ResponsesPreludeDecision {
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
    match value.get("type").and_then(Value::as_str) {
        Some("response.created" | "response.in_progress") => ResponsesPreludeDecision::Pending,
        Some(_) => ResponsesPreludeDecision::ReadyForPassThrough,
        None => ResponsesPreludeDecision::RetryableError(protocol_error(format!(
            "OpenAI Responses upstream emitted malformed stream event: {}",
            truncate_event_text(&value.to_string())
        ))),
    }
}

fn protocol_error(message: String) -> ResponsesStreamError {
    ResponsesStreamError {
        message,
        error_type: "upstream_protocol_error".to_string(),
        code: Some(Value::String("invalid_sse_event".to_string())),
        status: StatusCode::BAD_GATEWAY,
        retryable_before_output: true,
    }
}

fn truncate_event_text(text: &str) -> String {
    if text.len() <= INVALID_EVENT_LIMIT {
        return text.trim().to_string();
    }
    let end = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= INVALID_EVENT_LIMIT)
        .last()
        .unwrap_or(INVALID_EVENT_LIMIT);
    format!("{}... (truncated)", text[..end].trim())
}

impl ResponsesStreamError {
    pub fn display_message(&self) -> String {
        match self.code.as_ref().and_then(Value::as_str) {
            Some(code) if !code.trim().is_empty() => format!("{code}: {}", self.message),
            _ => self.message.clone(),
        }
    }

    pub fn openai_error_object(&self) -> Map<String, Value> {
        let mut error = Map::new();
        error.insert("message".to_string(), Value::String(self.message.clone()));
        error.insert("type".to_string(), Value::String(self.error_type.clone()));
        if let Some(code) = self.code.clone() {
            error.insert("code".to_string(), code);
        }
        error
    }
}

pub fn responses_stream_error(value: &Value) -> Option<ResponsesStreamError> {
    let event_type = value.get("type").and_then(Value::as_str)?;
    if !matches!(event_type, "response.failed" | "response.error" | "error") {
        return None;
    }

    let source = responses_error_source(value);
    let message = source
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .or_else(|| source.as_str())
        .unwrap_or("OpenAI Responses stream failed")
        .to_string();
    let error_type = source
        .get("type")
        .and_then(Value::as_str)
        .filter(|error_type| !error_type.trim().is_empty())
        .unwrap_or("proxy_error")
        .to_string();
    let code = source.get("code").cloned();
    let status_hint = response_status_hint(value, source);
    let (status, retryable_before_output) =
        classify_error_semantics(&error_type, code.as_ref(), &message, status_hint);
    Some(ResponsesStreamError {
        message,
        error_type,
        code,
        status,
        retryable_before_output,
    })
}

fn responses_error_source(value: &Value) -> &Value {
    value
        .pointer("/response/error")
        .or_else(|| value.get("error"))
        .unwrap_or(value)
}

/// 格式转换复用同一语义状态，避免渲染结果与 failover 判断漂移。
pub fn openai_error_status(error: &Value) -> StatusCode {
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = error.get("code");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .unwrap_or_default();
    let status_hint = error.get("status").and_then(parse_numeric_status);
    classify_error_semantics(error_type, code, message, status_hint).0
}

fn classify_error_semantics(
    error_type: &str,
    code: Option<&Value>,
    message: &str,
    status_hint: Option<StatusCode>,
) -> (StatusCode, bool) {
    let error_type = error_type.to_ascii_lowercase();
    let numeric_status = status_hint.or_else(|| code.and_then(parse_numeric_status));
    let code = code
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or_default();
    let detail = format!("{code} {message}").to_ascii_lowercase();

    // 上游会把容量/账号错误包在 invalid_request_error 中，具体信号必须优先于宽泛类型。
    let explicit_retryable = numeric_status.is_some_and(is_retryable_status_hint)
        || is_retryable_account_or_transient_error(&detail);
    let non_retryable = !explicit_retryable
        && (is_context_window_error(&detail)
            || is_invalid_request_error(&detail)
            || is_policy_or_safety_error(&detail)
            || is_context_window_error(&error_type)
            || is_invalid_request_error(&error_type)
            || is_policy_or_safety_error(&error_type));
    let status = numeric_status
        .or_else(|| known_semantic_status(&detail))
        .or_else(|| known_semantic_status(&error_type))
        .unwrap_or(StatusCode::BAD_GATEWAY);
    (status, !non_retryable)
}

/// xAI 错误事件会把 HTTP 语义状态放在事件顶层；Responses failed 事件也可能放在 response 内。
fn response_status_hint(value: &Value, source: &Value) -> Option<StatusCode> {
    source
        .get("status")
        .and_then(parse_numeric_status)
        .or_else(|| value.get("status").and_then(parse_numeric_status))
        .or_else(|| {
            value
                .pointer("/response/status")
                .and_then(parse_numeric_status)
        })
}

fn parse_numeric_status(value: &Value) -> Option<StatusCode> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
        .and_then(|value| u16::try_from(value).ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .filter(|status| status.is_client_error() || status.is_server_error())
}

fn is_retryable_status_hint(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::PAYMENT_REQUIRED
            | StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

fn known_semantic_status(text: &str) -> Option<StatusCode> {
    if is_rate_limit_error(text) {
        return Some(StatusCode::TOO_MANY_REQUESTS);
    }
    if is_authentication_error(text) {
        return Some(StatusCode::UNAUTHORIZED);
    }
    if is_permission_error(text) {
        return Some(StatusCode::FORBIDDEN);
    }
    if is_capacity_error(text) {
        return Some(StatusCode::SERVICE_UNAVAILABLE);
    }
    if is_context_window_error(text)
        || is_invalid_request_error(text)
        || is_policy_or_safety_error(text)
    {
        return Some(StatusCode::BAD_REQUEST);
    }
    None
}

fn is_retryable_account_or_transient_error(text: &str) -> bool {
    is_rate_limit_error(text)
        || is_authentication_error(text)
        || is_permission_error(text)
        || is_capacity_error(text)
}

fn is_rate_limit_error(text: &str) -> bool {
    text.contains("rate_limit")
        || text.contains("rate limit")
        || text.contains("too_many_requests")
        || text.contains("resource_exhausted")
        || text.contains("free-usage-exhausted")
        || text.contains("free_usage_exhausted")
        || text.contains("included free usage")
}

fn is_authentication_error(text: &str) -> bool {
    text.contains("authentication")
        || text.contains("unauthorized")
        || text.contains("invalid_api_key")
}

fn is_permission_error(text: &str) -> bool {
    text.contains("permission") || text.contains("forbidden") || text.contains("access denied")
}

fn is_capacity_error(text: &str) -> bool {
    if text.contains("server_is_overloaded")
        || text.contains("server_overloaded")
        || text.contains("slow_down")
        || text.contains("selected model is at capacity")
    {
        return true;
    }
    text.contains("model")
        && text.contains("capacity")
        && (text.contains("try a different model")
            || text.contains("please try again")
            || text.contains("temporarily unavailable")
            || text.contains("at capacity"))
}

fn is_context_window_error(combined: &str) -> bool {
    if combined.contains("context_too_large")
        || combined.contains("context_length_exceeded")
        || combined.contains("context_window_exceeded")
        || combined.contains("maximum context length")
        || combined.contains("max context length")
    {
        return true;
    }
    let exceeded = combined.contains("exceed")
        || combined.contains("too large")
        || combined.contains("too long");
    (combined.contains("context window") || combined.contains("context length")) && exceeded
        || combined.contains("token limit") && combined.contains("context") && exceeded
}

fn is_invalid_request_error(combined: &str) -> bool {
    combined.contains("invalid_request")
        || combined.contains("invalid request")
        || combined.contains("bad_request")
}

fn is_policy_or_safety_error(combined: &str) -> bool {
    [
        "content_policy",
        "content_filter",
        "policy",
        "safety",
        "high-risk cyber",
        "not allowed",
        "violat",
    ]
    .iter()
    .any(|marker| combined.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_non_retryable_request_and_policy_errors() {
        for (error, status) in [
            (
                json!({
                    "type": "invalid_request_error",
                    "code": "invalid_request",
                    "message": "invalid input"
                }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({
                    "type": "server_error",
                    "code": "context_length_exceeded",
                    "message": "context window exceeded"
                }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({
                    "type": "content_policy_error",
                    "code": "safety_violation",
                    "message": "request violates safety policy"
                }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({
                    "type": "api_error",
                    "code": "bad_request",
                    "message": "request cannot be processed"
                }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({
                    "type": "api_error",
                    "code": "content_filter",
                    "message": "blocked"
                }),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let event = json!({
                "type": "response.failed",
                "response": { "error": error }
            });
            let classified = responses_stream_error(&event).expect("error event");

            assert_eq!(classified.status, status);
            assert!(!classified.retryable_before_output, "event: {event}");
        }
    }

    #[test]
    fn classifies_account_transient_and_unknown_errors_as_retryable() {
        for (error, status) in [
            (
                json!({ "type": "rate_limit_error", "code": "rate_limit", "message": "slow down" }),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                json!({ "type": "authentication_error", "code": "invalid_api_key", "message": "bad key" }),
                StatusCode::UNAUTHORIZED,
            ),
            (
                json!({ "type": "server_error", "code": "server_is_overloaded", "message": "busy" }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                json!({ "type": "server_error", "code": "server_overloaded", "message": "busy" }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                json!({ "type": "server_error", "code": "unexpected", "message": "try later" }),
                StatusCode::BAD_GATEWAY,
            ),
            (
                json!({ "type": "api_error", "code": 503, "message": "upstream unavailable" }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ] {
            let event = json!({ "type": "error", "error": error });
            let classified = responses_stream_error(&event).expect("error event");

            assert_eq!(classified.status, status);
            assert!(classified.retryable_before_output, "event: {event}");
        }
    }

    #[test]
    fn explicit_retryable_signal_overrides_generic_invalid_request_type() {
        for (error, expected_status) in [
            (
                json!({
                    "type": "invalid_request_error",
                    "code": "server_is_overloaded",
                    "message": "capacity is temporarily exhausted"
                }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                json!({
                    "type": "invalid_request_error",
                    "code": "invalid_request",
                    "message": "Selected model is at capacity. Please try again."
                }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                json!({
                    "type": "invalid_request_error",
                    "code": "rate_limit",
                    "message": "slow down"
                }),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                json!({
                    "type": "invalid_request_error",
                    "code": "subscription:free-usage-exhausted",
                    "message": "You've used all the included free usage for now."
                }),
                StatusCode::TOO_MANY_REQUESTS,
            ),
        ] {
            let event = json!({
                "type": "response.failed",
                "response": { "error": error }
            });
            let classified = responses_stream_error(&event).expect("error event");

            assert_eq!(classified.status, expected_status, "event: {event}");
            assert!(classified.retryable_before_output, "event: {event}");
        }
    }

    #[test]
    fn honors_numeric_status_from_xai_error_event_and_response() {
        for event in [
            json!({
                "type": "error",
                "status": 429,
                "error": { "code": "subscription:quota", "message": "quota exhausted" }
            }),
            json!({
                "type": "response.failed",
                "response": {
                    "status": "429",
                    "error": { "type": "invalid_request_error", "message": "quota exhausted" }
                }
            }),
        ] {
            let classified = responses_stream_error(&event).expect("error event");

            assert_eq!(classified.status, StatusCode::TOO_MANY_REQUESTS);
            assert!(classified.retryable_before_output, "event: {event}");
        }
    }

    #[test]
    fn treats_payment_required_status_as_retryable_before_output() {
        for event in [
            json!({
                "type": "error",
                "status": 402,
                "error": { "type": "invalid_request_error", "message": "payment required" }
            }),
            json!({
                "type": "response.failed",
                "response": {
                    "status": "402",
                    "error": { "type": "invalid_request_error", "message": "payment required" }
                }
            }),
        ] {
            let classified = responses_stream_error(&event).expect("error event");

            assert_eq!(classified.status, StatusCode::PAYMENT_REQUIRED);
            assert!(classified.retryable_before_output, "event: {event}");
        }
    }

    #[test]
    fn ignores_non_error_responses_events() {
        assert!(responses_stream_error(&json!({
            "type": "response.output_text.delta",
            "delta": "hello"
        }))
        .is_none());
    }

    #[test]
    fn prelude_waits_for_business_output_and_retries_only_retryable_errors() {
        let mut inspector = ResponsesPreludeInspector::new();
        assert_eq!(
            inspector.inspect_chunk(
                br#"data: {"type":"response.created","response":{"id":"resp_1"}}

"#
            ),
            ResponsesPreludeDecision::Pending
        );

        let retry = inspector.inspect_chunk(
            br#"data: {"type":"response.failed","response":{"error":{"code":"server_overloaded","message":"busy"}}}

"#,
        );
        assert!(matches!(
            retry,
            ResponsesPreludeDecision::RetryableError(ResponsesStreamError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                ..
            })
        ));

        let mut inspector = ResponsesPreludeInspector::new();
        assert_eq!(
            inspector.inspect_chunk(
                br#"data: {"type":"response.failed","response":{"error":{"type":"invalid_request_error","message":"bad input"}}}

"#
            ),
            ResponsesPreludeDecision::ReadyForPassThrough
        );

        let mut inspector = ResponsesPreludeInspector::new();
        assert_eq!(
            inspector.inspect_chunk(
                br#"data: {"type":"response.output_text.delta","delta":"hello"}

"#
            ),
            ResponsesPreludeDecision::ReadyForPassThrough
        );
    }
}
