use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, Uri},
};
use serde_json::{Map, Value};

use super::{
    gemini,
    openai_compat::{transform_request_body, FormatTransform, CHAT_PATH, PROVIDER_CHAT},
    request_body::ReplayableBody,
    token_rate,
    RequestMeta,
};

const ANTHROPIC_MESSAGES_PREFIX: &str = "/v1/messages";
const ANTHROPIC_COMPLETE_PATH: &str = "/v1/complete";
const REQUEST_META_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_TRANSFORM_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const DEBUG_BODY_LOG_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) struct RequestError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
}

impl RequestError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

pub(crate) fn extract_request_path(uri: &Uri) -> (String, String) {
    let path = uri.path().to_string();
    let path_with_query = uri
        .query()
        .map(|query| format!("{path}?{query}"))
        .unwrap_or_else(|| path.clone());
    (path, path_with_query)
}

pub(crate) fn is_anthropic_path(path: &str) -> bool {
    if path == ANTHROPIC_COMPLETE_PATH || path == ANTHROPIC_MESSAGES_PREFIX {
        return true;
    }
    if !path.starts_with(ANTHROPIC_MESSAGES_PREFIX) {
        return false;
    }
    path.as_bytes()
        .get(ANTHROPIC_MESSAGES_PREFIX.len())
        .is_some_and(|byte| *byte == b'/')
}

pub(crate) async fn parse_request_meta_best_effort(
    path: &str,
    body: &ReplayableBody,
) -> RequestMeta {
    let stream_from_path = gemini::is_gemini_stream_path(path);
    let model_from_path = gemini::parse_gemini_model_from_path(path);
    let fallback_meta = RequestMeta {
        stream: stream_from_path,
        original_model: model_from_path.clone(),
        mapped_model: None,
        estimated_input_tokens: None,
    };

    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_META_LIMIT_BYTES)
        .await
        .unwrap_or(None)
    else {
        return fallback_meta;
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return fallback_meta,
    };
    let stream = value
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || stream_from_path;
    let original_model = value
        .get("model")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .or(model_from_path);
    let estimated_input_tokens = estimate_input_tokens(&value, original_model.as_deref());
    RequestMeta {
        stream,
        original_model,
        mapped_model: None,
        estimated_input_tokens,
    }
}

fn estimate_input_tokens(value: &Value, model: Option<&str>) -> Option<u64> {
    let mut total = 0u64;

    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        for message in messages {
            total = total.saturating_add(sum_message_tokens(message, model));
        }
    }

    if let Some(prompt) = value.get("prompt") {
        total = total.saturating_add(sum_text_value(prompt, model));
    }

    if let Some(input) = value.get("input") {
        total = total.saturating_add(sum_input_tokens(input, model));
    }

    if let Some(system) = value.get("system") {
        total = total.saturating_add(sum_text_value(system, model));
    }

    if let Some(system_instruction) = value.get("system_instruction") {
        total = total.saturating_add(sum_text_value(system_instruction, model));
    }

    if let Some(system_instruction) = value.get("systemInstruction") {
        total = total.saturating_add(sum_text_value(system_instruction, model));
    }

    if let Some(instructions) = value.get("instructions") {
        total = total.saturating_add(sum_text_value(instructions, model));
    }

    if let Some(contents) = value.get("contents") {
        total = total.saturating_add(sum_gemini_contents(contents, model));
    }

    if total == 0 {
        None
    } else {
        Some(total)
    }
}

fn sum_message_tokens(message: &Value, model: Option<&str>) -> u64 {
    let Some(content) = message.get("content") else {
        return 0;
    };
    sum_content_tokens(content, model)
}

fn sum_input_tokens(input: &Value, model: Option<&str>) -> u64 {
    match input {
        Value::String(_) => sum_text_value(input, model),
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            let mut next = acc;
            if item.is_string() {
                next = next.saturating_add(sum_text_value(item, model));
            } else if let Some(content) = item.get("content") {
                next = next.saturating_add(sum_content_tokens(content, model));
            }
            next
        }),
        Value::Object(object) => object
            .get("content")
            .map(|content| sum_content_tokens(content, model))
            .unwrap_or(0),
        _ => 0,
    }
}

fn sum_gemini_contents(contents: &Value, model: Option<&str>) -> u64 {
    let Some(contents) = contents.as_array() else {
        return 0;
    };
    contents.iter().fold(0u64, |acc, content| {
        let mut total = acc;
        if let Some(parts) = content.get("parts").and_then(Value::as_array) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    total = total.saturating_add(token_rate::estimate_text_tokens(model, text));
                }
            }
        }
        total
    })
}

fn sum_content_tokens(content: &Value, model: Option<&str>) -> u64 {
    match content {
        Value::String(_) => sum_text_value(content, model),
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            let mut total = acc;
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                total = total.saturating_add(token_rate::estimate_text_tokens(model, text));
            } else if item.is_string() {
                total = total.saturating_add(sum_text_value(item, model));
            }
            total
        }),
        _ => 0,
    }
}

fn sum_text_value(value: &Value, model: Option<&str>) -> u64 {
    match value {
        Value::String(text) => token_rate::estimate_text_tokens(model, text),
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            acc.saturating_add(sum_text_value(item, model))
        }),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(|text| token_rate::estimate_text_tokens(model, text))
            .unwrap_or(0),
        _ => 0,
    }
}

fn ensure_stream_options_include_usage(object: &mut Map<String, Value>) -> bool {
    let include_usage = object
        .get("stream_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("include_usage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if include_usage {
        return false;
    }

    let options = match object.get_mut("stream_options") {
        Some(Value::Object(options)) => options,
        _ => {
            object.insert("stream_options".to_string(), Value::Object(Map::new()));
            object
                .get_mut("stream_options")
                .and_then(Value::as_object_mut)
                .expect("stream_options must be object")
        }
    };
    options.insert("include_usage".to_string(), Value::Bool(true));
    true
}

pub(crate) async fn log_debug_request(headers: &HeaderMap, body: &ReplayableBody) {
    let header_snapshot: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| {
            let redacted = if is_sensitive_header(name.as_str()) {
                "***".to_string()
            } else {
                value.to_str().unwrap_or("").to_string()
            };
            (name.to_string(), redacted)
        })
        .collect();

    let body_text = match body.read_bytes_if_small(DEBUG_BODY_LOG_LIMIT_BYTES).await {
        Ok(Some(bytes)) => {
            let text = String::from_utf8_lossy(&bytes);
            Some(text.into_owned())
        }
        Ok(None) => None,
        Err(err) => {
            tracing::debug!(error = %err, "debug body read failed");
            None
        }
    };

    match body_text {
        Some(text) => {
            tracing::debug!(headers = ?header_snapshot, body = %text, "incoming request debug dump");
        }
        None => {
            tracing::debug!(headers = ?header_snapshot, "incoming request body omitted (too large)");
        }
    }
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "x-api-key"
    )
}

pub(crate) async fn maybe_transform_request_body(
    transform: FormatTransform,
    body: ReplayableBody,
) -> Result<ReplayableBody, RequestError> {
    if transform == FormatTransform::None {
        return Ok(body);
    }

    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_TRANSFORM_LIMIT_BYTES)
        .await
        .map_err(|err| {
            RequestError::new(
                StatusCode::BAD_REQUEST,
                format!("Failed to read request body: {err}"),
            )
        })?
    else {
        return Err(RequestError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request body is too large to transform.",
        ));
    };

    let outbound_bytes = transform_request_body(transform, &bytes)
        .map_err(|message| RequestError::new(StatusCode::BAD_REQUEST, message))?;
    Ok(ReplayableBody::from_bytes(outbound_bytes))
}

pub(crate) async fn maybe_force_openai_stream_options_include_usage(
    provider: &str,
    outbound_path: &str,
    meta: &RequestMeta,
    body: ReplayableBody,
) -> Result<ReplayableBody, RequestError> {
    if provider != PROVIDER_CHAT || outbound_path != CHAT_PATH || !meta.stream {
        return Ok(body);
    }

    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_TRANSFORM_LIMIT_BYTES)
        .await
        .map_err(|err| {
            RequestError::new(
                StatusCode::BAD_REQUEST,
                format!("Failed to read request body: {err}"),
            )
        })?
    else {
        // Best-effort: request body too large, keep original.
        return Ok(body);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(body);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(body);
    };
    if !ensure_stream_options_include_usage(object) {
        return Ok(body);
    }

    let outbound_bytes = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|err| {
            RequestError::new(
                StatusCode::BAD_REQUEST,
                format!("Failed to serialize request: {err}"),
            )
        })?;
    Ok(ReplayableBody::from_bytes(outbound_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Bytes;

    #[test]
    fn force_openai_chat_stream_usage_inserts_stream_options_include_usage() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let input = Bytes::from_static(br#"{"stream":true,"messages":[]}"#);
            let meta = RequestMeta {
                stream: true,
                original_model: None,
                mapped_model: None,
                estimated_input_tokens: None,
            };
            let body = ReplayableBody::from_bytes(input);
            let output = maybe_force_openai_stream_options_include_usage(
                PROVIDER_CHAT,
                CHAT_PATH,
                &meta,
                body,
            )
            .await
            .expect("ok");
            let bytes = output
                .read_bytes_if_small(1024)
                .await
                .expect("read")
                .expect("bytes");
            let value: Value = serde_json::from_slice(&bytes).expect("json");
            assert_eq!(value["stream_options"]["include_usage"], Value::Bool(true));
        });
    }

    #[test]
    fn gemini_meta_prefers_path_for_stream_and_model() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let body = ReplayableBody::from_bytes(Bytes::from_static(b"{}"));
            let meta = parse_request_meta_best_effort(
                "/v1beta/models/gemini-1.5-flash:streamGenerateContent",
                &body,
            )
            .await;
            assert!(meta.stream);
            assert_eq!(meta.original_model.as_deref(), Some("gemini-1.5-flash"));
        });
    }
}
