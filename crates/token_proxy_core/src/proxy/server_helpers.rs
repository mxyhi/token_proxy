use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, Uri},
};
use serde_json::{Map, Value};

use super::{
    gemini,
    http_client::ProxyHttpClients,
    openai_compat::{
        transform_request_body, FormatTransform, CHAT_PATH, PROVIDER_CHAT, PROVIDER_RESPONSES,
        RESPONSES_PATH,
    },
    request_body::ReplayableBody,
    request_token_estimate, RequestMeta,
};

const ANTHROPIC_MESSAGES_PREFIX: &str = "/v1/messages";
const ANTHROPIC_COMPLETE_PATH: &str = "/v1/complete";
const REQUEST_META_LIMIT_BYTES: usize = 2 * 1024 * 1024;
// Format conversion needs the full JSON body; allow up to the default max_request_body_bytes (20 MiB).
const REQUEST_TRANSFORM_LIMIT_BYTES: usize = 20 * 1024 * 1024;
const DEBUG_BODY_LOG_LIMIT_BYTES: usize = usize::MAX;
const OPENAI_REASONING_MODEL_SUFFIX_PREFIX: &str = "-reasoning-";

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
        reasoning_effort: None,
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
    let mut original_model = value
        .get("model")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .or(model_from_path);

    // KISS: only support the explicit `-reasoning-<effort>` suffix to avoid ambiguity.
    // This mirrors new-api behavior: strip the suffix from `model` and translate it into
    // OpenAI reasoning parameters when dispatching to OpenAI providers.
    let mut reasoning_effort = None;
    if let Some(model) = original_model.as_deref() {
        if let Some((base_model, effort)) = parse_openai_reasoning_effort_from_model_suffix(model) {
            original_model = Some(base_model);
            reasoning_effort = Some(effort);
        }
    }

    let estimated_input_tokens =
        request_token_estimate::estimate_request_input_tokens(&value, original_model.as_deref());
    RequestMeta {
        stream,
        original_model,
        mapped_model: None,
        reasoning_effort,
        estimated_input_tokens,
    }
}

pub(crate) fn parse_openai_reasoning_effort_from_model_suffix(
    model: &str,
) -> Option<(String, String)> {
    let (base, effort_raw) = model.rsplit_once(OPENAI_REASONING_MODEL_SUFFIX_PREFIX)?;
    let base = base.trim();
    let effort = effort_raw.trim().to_ascii_lowercase();
    if base.is_empty() || effort.is_empty() {
        return None;
    }

    match effort.as_str() {
        "low" | "medium" | "high" | "minimal" | "none" | "xhigh" => {
            Some((base.to_string(), effort))
        }
        _ => None,
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
    log_debug_headers_body(
        "inbound.request",
        Some(headers),
        Some(body),
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
}

pub(crate) async fn log_debug_headers_body(
    stage: &str,
    headers: Option<&HeaderMap>,
    body: Option<&ReplayableBody>,
    max_body_bytes: usize,
) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }

    let header_snapshot = headers.map(snapshot_headers_raw).unwrap_or_default();
    let body_text = if let Some(body) = body {
        match body.read_bytes_if_small(max_body_bytes).await {
            Ok(Some(bytes)) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Ok(None) => Some(format!(
                "[body omitted: larger than {max_body_bytes} bytes]"
            )),
            Err(err) => Some(format!("[body read failed: {err}]")),
        }
    } else {
        None
    };

    match body_text {
        Some(text) => {
            tracing::debug!(stage, headers = ?header_snapshot, body = %text, "debug dump");
        }
        None => {
            tracing::debug!(stage, headers = ?header_snapshot, "debug dump (no body)");
        }
    }
}

fn snapshot_headers_raw(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = value.to_str().unwrap_or("<binary>").to_string();
            (name.to_string(), value)
        })
        .collect()
}

pub(crate) async fn maybe_transform_request_body(
    http_clients: &ProxyHttpClients,
    _provider: &str,
    _path: &str,
    transform: FormatTransform,
    model_hint: Option<&str>,
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
    let inbound_body = ReplayableBody::from_bytes(bytes.clone());
    log_debug_headers_body(
        "transform.input",
        None,
        Some(&inbound_body),
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;

    let outbound_bytes = transform_request_body(transform, &bytes, http_clients, model_hint)
        .await
        .map_err(|message| RequestError::new(StatusCode::BAD_REQUEST, message))?;
    let outbound_body = ReplayableBody::from_bytes(outbound_bytes);
    log_debug_headers_body(
        "transform.output",
        None,
        Some(&outbound_body),
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    Ok(outbound_body)
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

    let outbound_bytes = serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
        RequestError::new(
            StatusCode::BAD_REQUEST,
            format!("Failed to serialize request: {err}"),
        )
    })?;
    let outbound_body = ReplayableBody::from_bytes(outbound_bytes);
    log_debug_headers_body(
        "stream_options.output",
        None,
        Some(&outbound_body),
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    Ok(outbound_body)
}

pub(crate) async fn maybe_rewrite_openai_reasoning_effort_from_model_suffix(
    provider: &str,
    outbound_path: &str,
    meta: &RequestMeta,
    body: &ReplayableBody,
) -> Result<Option<ReplayableBody>, RequestError> {
    let Some(effort) = meta.reasoning_effort.as_deref() else {
        return Ok(None);
    };
    if !should_apply_openai_reasoning_effort(provider, outbound_path) {
        return Ok(None);
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
        return Ok(None);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };

    let model_for_upstream = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref());
    apply_openai_reasoning_effort_to_body(
        provider,
        outbound_path,
        model_for_upstream,
        effort,
        object,
    );

    let outbound_bytes = serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
        RequestError::new(
            StatusCode::BAD_REQUEST,
            format!("Failed to serialize request: {err}"),
        )
    })?;
    Ok(Some(ReplayableBody::from_bytes(outbound_bytes)))
}

fn should_apply_openai_reasoning_effort(provider: &str, outbound_path: &str) -> bool {
    (provider == PROVIDER_CHAT && outbound_path == CHAT_PATH)
        || (provider == PROVIDER_RESPONSES && outbound_path == RESPONSES_PATH)
}

fn apply_openai_reasoning_effort_to_body(
    provider: &str,
    outbound_path: &str,
    normalized_model: Option<&str>,
    effort: &str,
    object: &mut Map<String, Value>,
) {
    // Ensure the upstream sees the normalized base model (without the `-reasoning-...` suffix).
    if let Some(model) = normalized_model {
        object.insert("model".to_string(), Value::String(model.to_string()));
    }

    if provider == PROVIDER_CHAT && outbound_path == CHAT_PATH {
        object.insert(
            "reasoning_effort".to_string(),
            Value::String(effort.to_string()),
        );
        return;
    }
    if provider == PROVIDER_RESPONSES && outbound_path == RESPONSES_PATH {
        let reasoning = ensure_json_object_field(object, "reasoning");
        reasoning.insert("effort".to_string(), Value::String(effort.to_string()));
    }
}

fn ensure_json_object_field<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> &'a mut Map<String, Value> {
    if !matches!(object.get(key), Some(Value::Object(_))) {
        object.insert(key.to_string(), Value::Object(Map::new()));
    }
    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("inserted value must be object")
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "server_helpers.test.rs"]
mod tests;
