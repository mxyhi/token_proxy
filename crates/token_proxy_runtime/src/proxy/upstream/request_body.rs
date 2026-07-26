use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
};
use serde_json::{Map, Value};

use super::super::{
    config::UpstreamRuntime, http, model, request_body::ReplayableBody, RequestMeta,
};
use super::{request::split_path_query, AttemptOutcome};
use token_proxy_protocol::xai_client_tools::XaiClientToolMapping;

const OPENAI_CHAT_PATH: &str = "/v1/chat/completions";
const OPENAI_RESPONSES_PATH: &str = "/v1/responses";
const XAI_RESPONSES_COMPACT_PATH: &str = "/v1/responses/compact";
const ANTHROPIC_COUNT_TOKENS_PATH: &str = "/v1/messages/count_tokens";
const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";
const REQUEST_MODEL_MAPPING_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_REASONING_LIMIT_BYTES: usize = 100 * 1024 * 1024;
const REQUEST_FILTER_LIMIT_BYTES: usize = 20 * 1024 * 1024;
const CODEX_INSTALLATION_ID_KEY: &str = "x-codex-installation-id";
const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const CODEX_RESPONSES_LITE_METADATA_KEY: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";

pub(super) async fn build_upstream_body(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
    meta: &RequestMeta,
    codex_openai_device_id: Option<&str>,
    request_headers: &HeaderMap,
) -> Result<reqwest::Body, AttemptOutcome> {
    let transformed = build_json_transformed_body_with_headers(
        provider,
        upstream,
        upstream_path_with_query,
        body,
        meta,
        codex_openai_device_id,
        request_headers,
    )
    .await?;
    let final_source = transformed.as_ref().unwrap_or(body);
    final_source.to_reqwest_body().await.map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to read cached request body: {err}"),
        ))
    })
}

pub(super) async fn xai_client_tool_mapping(
    provider: &str,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
) -> Result<Option<XaiClientToolMapping>, AttemptOutcome> {
    let upstream_path = split_path_query(upstream_path_with_query).0;
    if !should_filter_xai_responses_request(provider, upstream_path) {
        return Ok(None);
    }
    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_FILTER_LIMIT_BYTES)
        .await
        .map_err(|error| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read cached request body: {error}"),
            ))
        })?
    else {
        return Ok(None);
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    promote_xai_additional_tools(object);
    let (mapping, _) =
        token_proxy_protocol::xai_client_tools::adapt_request(object).map_err(|message| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_REQUEST, message))
        })?;
    Ok((!mapping.is_empty()).then_some(mapping))
}

#[cfg(test)]
async fn build_json_transformed_body(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
    meta: &RequestMeta,
    codex_openai_device_id: Option<&str>,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    build_json_transformed_body_with_headers(
        provider,
        upstream,
        upstream_path_with_query,
        body,
        meta,
        codex_openai_device_id,
        &HeaderMap::new(),
    )
    .await
}

async fn build_json_transformed_body_with_headers(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
    meta: &RequestMeta,
    codex_openai_device_id: Option<&str>,
    request_headers: &HeaderMap,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    let upstream_path = split_path_query(upstream_path_with_query).0;
    if !needs_json_transform(
        provider,
        upstream,
        upstream_path,
        meta,
        codex_openai_device_id,
        request_headers,
    ) {
        return Ok(None);
    }

    let must_strip_sampling =
        should_strip_openai_responses_sampling_params(provider, upstream_path, meta);
    let must_filter_xai = should_filter_xai_responses_request(provider, upstream_path);
    let must_sanitize_responses_input =
        should_sanitize_native_openai_responses_input(provider, upstream_path);
    let read_limit = json_transform_read_limit(
        provider,
        upstream,
        upstream_path,
        meta,
        codex_openai_device_id,
        request_headers,
    );
    let Some(bytes) = body.read_bytes_if_small(read_limit).await.map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to read cached request body: {err}"),
        ))
    })?
    else {
        if must_strip_sampling {
            return Err(openai_responses_sampling_params_payload_too_large());
        }
        if must_filter_xai {
            return Err(xai_responses_payload_too_large());
        }
        if must_sanitize_responses_input {
            return Err(openai_responses_input_payload_too_large());
        }
        return Ok(None);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };

    let mut changed = false;
    let body_len = bytes.len();
    changed |= normalize_anthropic_model(provider, upstream_path, object, meta, body_len);
    changed |= rewrite_model_mapping(object, meta, body_len);
    changed |= apply_reasoning_effort(provider, upstream_path, object, meta, body_len);
    changed |= filter_openai_responses_fields(provider, upstream, upstream_path, object, body_len);
    changed |= sanitize_native_openai_responses_input(provider, upstream_path, object, body_len);
    changed |= filter_xai_responses_request(provider, upstream_path, object, meta, body_len)?;
    changed |= normalize_xai_image_refs(provider, object, body_len);
    changed |= force_codex_responses_lite_parallel_tool_calls(
        provider,
        upstream_path,
        request_headers,
        object,
        body_len,
    );
    changed |= strip_openai_responses_sampling_params(
        provider,
        upstream_path,
        object,
        meta,
        body_len,
        must_strip_sampling,
    )?;
    changed |= rewrite_developer_roles_if_needed(upstream, upstream_path, object, body_len);
    changed |= filter_anthropic_count_tokens_request(provider, upstream_path, object, body_len);
    changed |= inject_codex_installation_id(object, provider, codex_openai_device_id);
    if !changed {
        return Ok(None);
    }

    replayable_from_json(value).map(Some)
}

fn json_transform_read_limit(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path: &str,
    meta: &RequestMeta,
    codex_openai_device_id: Option<&str>,
    request_headers: &HeaderMap,
) -> usize {
    let mut limit = 0usize;
    if meta.model_override().is_some() && meta.mapped_model.is_some() {
        limit = limit.max(REQUEST_MODEL_MAPPING_LIMIT_BYTES);
    }
    if should_normalize_anthropic_model(provider, upstream_path, meta) {
        limit = limit.max(REQUEST_MODEL_MAPPING_LIMIT_BYTES);
    }
    if should_apply_reasoning_effort(provider, upstream_path, meta) {
        limit = limit.max(REQUEST_REASONING_LIMIT_BYTES);
    }
    if should_filter_openai_responses_fields(provider, upstream, upstream_path) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_sanitize_native_openai_responses_input(provider, upstream_path) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_filter_xai_responses_request(provider, upstream_path) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_normalize_xai_image_refs(provider) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_strip_openai_responses_sampling_params(provider, upstream_path, meta) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_rewrite_developer_roles(upstream, upstream_path) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_filter_anthropic_count_tokens_request(provider, upstream_path) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_inject_codex_installation_id(provider, codex_openai_device_id) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    if should_inspect_codex_responses_lite(provider, upstream_path, request_headers) {
        limit = limit.max(REQUEST_FILTER_LIMIT_BYTES);
    }
    limit
}

fn needs_json_transform(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path: &str,
    meta: &RequestMeta,
    codex_openai_device_id: Option<&str>,
    request_headers: &HeaderMap,
) -> bool {
    (meta.model_override().is_some() && meta.mapped_model.is_some())
        || should_normalize_anthropic_model(provider, upstream_path, meta)
        || should_apply_reasoning_effort(provider, upstream_path, meta)
        || should_filter_openai_responses_fields(provider, upstream, upstream_path)
        || should_sanitize_native_openai_responses_input(provider, upstream_path)
        || should_filter_xai_responses_request(provider, upstream_path)
        || should_normalize_xai_image_refs(provider)
        || should_strip_openai_responses_sampling_params(provider, upstream_path, meta)
        || should_rewrite_developer_roles(upstream, upstream_path)
        || should_filter_anthropic_count_tokens_request(provider, upstream_path)
        || should_inject_codex_installation_id(provider, codex_openai_device_id)
        || should_inspect_codex_responses_lite(provider, upstream_path, request_headers)
}

fn should_inspect_codex_responses_lite(
    provider: &str,
    upstream_path: &str,
    _request_headers: &HeaderMap,
) -> bool {
    provider == "codex"
        && (upstream_path == OPENAI_RESPONSES_PATH || upstream_path == XAI_RESPONSES_COMPACT_PATH)
}

fn force_codex_responses_lite_parallel_tool_calls(
    provider: &str,
    upstream_path: &str,
    request_headers: &HeaderMap,
    object: &mut Map<String, Value>,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_FILTER_LIMIT_BYTES
        || !should_inspect_codex_responses_lite(provider, upstream_path, request_headers)
    {
        return false;
    }
    let header_lite = request_headers
        .get(CODEX_RESPONSES_LITE_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
    let metadata_lite = object
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(CODEX_RESPONSES_LITE_METADATA_KEY))
        .is_some_and(|value| {
            value.as_bool() == Some(true)
                || value
                    .as_str()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
        });
    if !header_lite && !metadata_lite {
        return false;
    }
    if object.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false) {
        return false;
    }
    object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    tracing::debug!("forced parallel_tool_calls=false for Codex Responses Lite");
    true
}

fn should_normalize_anthropic_model(
    provider: &str,
    upstream_path: &str,
    meta: &RequestMeta,
) -> bool {
    provider == "anthropic"
        && (upstream_path == ANTHROPIC_MESSAGES_PATH
            || upstream_path.starts_with(&format!("{ANTHROPIC_MESSAGES_PATH}/")))
        && meta.mapped_model.is_none()
        && meta.original_model.is_some()
}

fn normalize_anthropic_model(
    provider: &str,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    meta: &RequestMeta,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_MODEL_MAPPING_LIMIT_BYTES
        || !should_normalize_anthropic_model(provider, upstream_path, meta)
    {
        return false;
    }
    let Some(normalized_model) = meta.original_model.as_deref() else {
        return false;
    };
    let Some(body_model) = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return false;
    };
    if body_model == normalized_model {
        return false;
    }
    object.insert(
        "model".to_string(),
        Value::String(normalized_model.to_string()),
    );
    tracing::debug!(
        body_model,
        normalized_model,
        "normalized Anthropic upstream model"
    );
    true
}

fn rewrite_model_mapping(
    object: &mut Map<String, Value>,
    meta: &RequestMeta,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_MODEL_MAPPING_LIMIT_BYTES {
        return false;
    }
    if meta.model_override().is_none() {
        return false;
    }
    let Some(mapped_model) = meta.mapped_model.as_deref() else {
        return false;
    };
    if !object.contains_key("model") {
        return false;
    }
    object.insert("model".to_string(), Value::String(mapped_model.to_string()));
    true
}

fn should_apply_reasoning_effort(provider: &str, upstream_path: &str, meta: &RequestMeta) -> bool {
    meta.reasoning_effort.is_some()
        && ((provider == "openai" && upstream_path == OPENAI_CHAT_PATH)
            || (provider == "openai-response" && upstream_path == OPENAI_RESPONSES_PATH))
}

fn apply_reasoning_effort(
    provider: &str,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    meta: &RequestMeta,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_REASONING_LIMIT_BYTES {
        return false;
    }
    let Some(effort) = meta.reasoning_effort.as_deref() else {
        return false;
    };
    if !should_apply_reasoning_effort(provider, upstream_path, meta) {
        return false;
    }

    let model_for_upstream = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref());
    let effort = normalize_glm_reasoning_effort(model_for_upstream, effort).unwrap_or(effort);
    if let Some(model) = model_for_upstream {
        object.insert("model".to_string(), Value::String(model.to_string()));
    }
    if provider == "openai" {
        object.insert(
            "reasoning_effort".to_string(),
            Value::String(effort.to_string()),
        );
        return true;
    }

    let reasoning = ensure_json_object_field(object, "reasoning");
    reasoning.insert("effort".to_string(), Value::String(effort.to_string()));
    true
}

fn normalize_glm_reasoning_effort(model: Option<&str>, effort: &str) -> Option<&'static str> {
    let model = model?.trim().to_ascii_lowercase();
    if !model.starts_with("glm-") {
        return None;
    }
    let normalized = effort
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_', ' '], "");
    match normalized.as_str() {
        "low" | "medium" | "high" => Some("high"),
        "xhigh" | "extrahigh" | "max" | "ultracode" => Some("max"),
        _ => None,
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

fn should_filter_openai_responses_fields(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path: &str,
) -> bool {
    provider == "openai-response"
        && upstream_path == OPENAI_RESPONSES_PATH
        && (upstream.filter_prompt_cache_retention || upstream.filter_safety_identifier)
}

fn filter_openai_responses_fields(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_FILTER_LIMIT_BYTES {
        return false;
    }
    if !should_filter_openai_responses_fields(provider, upstream, upstream_path) {
        return false;
    }
    let mut changed = false;
    if upstream.filter_prompt_cache_retention {
        changed |= object.remove("prompt_cache_retention").is_some();
    }
    if upstream.filter_safety_identifier {
        changed |= object.remove("safety_identifier").is_some();
    }
    changed
}

fn should_sanitize_native_openai_responses_input(provider: &str, upstream_path: &str) -> bool {
    matches!(provider, "openai" | "openai-response") && upstream_path == OPENAI_RESPONSES_PATH
}

/// 删除上游不接受的客户端历史元数据；只处理 direct input item，不触碰 tools 或嵌套 content。
fn sanitize_native_openai_responses_input(
    provider: &str,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_FILTER_LIMIT_BYTES
        || !should_sanitize_native_openai_responses_input(provider, upstream_path)
    {
        return false;
    }
    let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut removed_namespace_count = 0usize;
    let mut removed_id_count = 0usize;
    for item in items {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        removed_namespace_count += usize::from(item.remove("namespace").is_some());

        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let required_id_prefix = match item_type {
            "message" => Some("msg"),
            "function_call" | "tool_call" | "local_shell_call" | "tool_search_call"
            | "custom_tool_call" | "mcp_tool_call" => Some("fc"),
            _ => None,
        };
        if required_id_prefix.is_some_and(|prefix| {
            item.get("id")
                .is_some_and(|id| !id.as_str().is_some_and(|id| id.starts_with(prefix)))
        }) {
            item.remove("id");
            removed_id_count += 1;
        }
    }

    let changed = removed_namespace_count > 0 || removed_id_count > 0;
    if changed {
        tracing::debug!(
            provider,
            removed_namespace_count,
            removed_id_count,
            "sanitized native OpenAI Responses input metadata"
        );
    }
    changed
}

fn openai_responses_input_payload_too_large() -> AttemptOutcome {
    AttemptOutcome::Fatal(http::error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "OpenAI Responses request is too large to sanitize input metadata; limit is {REQUEST_FILTER_LIMIT_BYTES} bytes."
        ),
    ))
}

fn should_filter_xai_responses_request(provider: &str, upstream_path: &str) -> bool {
    provider == "xai"
        && (upstream_path == OPENAI_RESPONSES_PATH || upstream_path == XAI_RESPONSES_COMPACT_PATH)
}

fn filter_xai_responses_request(
    provider: &str,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    meta: &RequestMeta,
    body_len: usize,
) -> Result<bool, AttemptOutcome> {
    if body_len > REQUEST_FILTER_LIMIT_BYTES
        || !should_filter_xai_responses_request(provider, upstream_path)
    {
        return Ok(false);
    }

    let mut changed = remove_json_fields(
        object,
        &[
            "previous_response_id",
            "prompt_cache_retention",
            "safety_identifier",
            "stream_options",
        ],
    );
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .or(meta.mapped_model.as_deref())
        .or(meta.original_model.as_deref());
    if model.is_some_and(is_xai_grok_45_model) {
        changed |= remove_json_fields(
            object,
            &[
                "presence_penalty",
                "presencePenalty",
                "frequency_penalty",
                "frequencyPenalty",
                "stop",
            ],
        );
    }

    if upstream_path == XAI_RESPONSES_COMPACT_PATH {
        changed |= remove_json_fields(
            object,
            &[
                "stream",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "compaction_trigger",
            ],
        );
        changed |= remove_xai_input_items_by_type(object, "compaction_trigger");
    } else {
        changed |= promote_xai_additional_tools(object);
        let (_, client_tools_changed) =
            token_proxy_protocol::xai_client_tools::adapt_request(object).map_err(|message| {
                AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_REQUEST, message))
            })?;
        changed |= client_tools_changed;
        changed |= normalize_xai_root_tool_union_branches(object);
        if !has_xai_request_tools(object) {
            changed |= remove_json_fields(object, &["tools", "tool_choice", "parallel_tool_calls"]);
        }
    }

    if changed {
        tracing::debug!(
            endpoint = upstream_path,
            "filtered unsupported xAI Responses request fields"
        );
    }
    Ok(changed)
}

// Responses Lite encodes extra declarations as input items, while xAI accepts
// declarations only in the root tools array. Preserve root order, then append extras.
fn promote_xai_additional_tools(object: &mut Map<String, Value>) -> bool {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };

    let original_len = input.len();
    let mut promoted = Vec::new();
    input.retain(|item| {
        if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
            return true;
        }
        if let Some(tools) = item.get("tools").and_then(Value::as_array) {
            promoted.extend(tools.iter().cloned());
        }
        false
    });
    if input.len() == original_len {
        return false;
    }

    if !promoted.is_empty() {
        match object.get_mut("tools") {
            Some(Value::Array(tools)) => tools.extend(promoted),
            _ => {
                object.insert("tools".to_string(), Value::Array(promoted));
            }
        }
    }
    tracing::debug!("promoted xAI additional_tools into root tools");
    true
}

// xAI requires every branch of an object-only root union to declare its type.
// Nested unions keep their original schema semantics.
fn normalize_xai_root_tool_union_branches(object: &mut Map<String, Value>) -> bool {
    let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for tool in tools {
        let Some(tool) = tool.as_object_mut() else {
            continue;
        };
        let tool_type = tool.get("type").and_then(Value::as_str);
        if !matches!(tool_type, Some("function" | "custom")) {
            continue;
        }
        if let Some(parameters) = tool.get_mut("parameters") {
            changed |= normalize_xai_object_root_union(parameters);
        }
        if let Some(parameters) = tool
            .get_mut("function")
            .and_then(Value::as_object_mut)
            .and_then(|function| function.get_mut("parameters"))
        {
            changed |= normalize_xai_object_root_union(parameters);
        }
    }
    if changed {
        tracing::debug!("normalized xAI root tool union branch types");
    }
    changed
}

fn normalize_xai_object_root_union(parameters: &mut Value) -> bool {
    let Some(parameters) = parameters.as_object_mut() else {
        return false;
    };
    if parameters.get("type").and_then(Value::as_str) != Some("object") {
        return false;
    }

    let mut changed = false;
    for union_name in ["anyOf", "oneOf"] {
        let Some(branches) = parameters.get_mut(union_name).and_then(Value::as_array_mut) else {
            continue;
        };
        for branch in branches {
            let Some(branch) = branch.as_object_mut() else {
                continue;
            };
            if !branch.contains_key("type") {
                branch.insert("type".to_string(), Value::String("object".to_string()));
                changed = true;
            }
        }
    }
    changed
}

fn should_normalize_xai_image_refs(provider: &str) -> bool {
    provider == "xai"
}

fn normalize_xai_image_refs(
    provider: &str,
    object: &mut Map<String, Value>,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_FILTER_LIMIT_BYTES || !should_normalize_xai_image_refs(provider) {
        return false;
    }
    let changed = normalize_xai_image_refs_in_object(object);
    if changed {
        tracing::debug!("normalized xAI image reference fields");
    }
    changed
}

// Only image API reference containers use `url`; chat content parts keep
// `image_url` because it is a different OpenAI wire shape.
fn normalize_xai_image_refs_in_object(object: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    for (key, value) in object {
        match key.as_str() {
            "image" => changed |= normalize_xai_image_ref(value),
            "images" | "reference_images" => {
                if let Some(references) = value.as_array_mut() {
                    for reference in references {
                        changed |= normalize_xai_image_ref(reference);
                    }
                }
            }
            _ => changed |= normalize_xai_image_refs_in_value(value),
        }
    }
    changed
}

fn normalize_xai_image_refs_in_value(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => normalize_xai_image_refs_in_object(object),
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= normalize_xai_image_refs_in_value(value);
            }
            changed
        }
        _ => false,
    }
}

fn normalize_xai_image_ref(value: &mut Value) -> bool {
    let Some(reference) = value.as_object_mut() else {
        return false;
    };
    let existing_url = reference
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);
    let image_url = reference.get("image_url").and_then(|value| match value {
        Value::String(url) => Some(url.as_str()),
        Value::Object(object) => object.get("url").and_then(Value::as_str),
        _ => None,
    });
    let Some(url) = existing_url.or_else(|| {
        image_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
    }) else {
        return false;
    };

    let inserted = reference.get("url").and_then(Value::as_str) != Some(url.as_str());
    if inserted {
        reference.insert("url".to_string(), Value::String(url));
    }
    inserted | reference.remove("image_url").is_some()
}

fn remove_json_fields(object: &mut Map<String, Value>, fields: &[&str]) -> bool {
    let mut changed = false;
    for field in fields {
        changed |= object.remove(*field).is_some();
    }
    changed
}

fn has_xai_request_tools(object: &Map<String, Value>) -> bool {
    if object
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
    {
        return true;
    }
    object
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("additional_tools")
                    && item
                        .get("tools")
                        .and_then(Value::as_array)
                        .is_some_and(|tools| !tools.is_empty())
            })
        })
}

fn remove_xai_input_items_by_type(object: &mut Map<String, Value>, item_type: &str) -> bool {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let original_len = input.len();
    input.retain(|item| item.get("type").and_then(Value::as_str) != Some(item_type));
    input.len() != original_len
}

fn is_xai_grok_45_model(model: &str) -> bool {
    let model = model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .split_once('(')
        .map_or_else(|| model.trim(), |(base, _)| base.trim());
    model.eq_ignore_ascii_case("grok-4.5")
}

fn xai_responses_payload_too_large() -> AttemptOutcome {
    AttemptOutcome::Fatal(http::error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "xAI Responses request is too large to filter unsupported fields; limit is {REQUEST_FILTER_LIMIT_BYTES} bytes."
        ),
    ))
}

fn should_strip_openai_responses_sampling_params(
    provider: &str,
    upstream_path: &str,
    meta: &RequestMeta,
) -> bool {
    let model = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref());
    provider == "openai-response"
        && upstream_path == OPENAI_RESPONSES_PATH
        && model.is_some_and(model::is_openai_responses_reasoning_model)
}

fn strip_openai_responses_sampling_params(
    provider: &str,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    meta: &RequestMeta,
    body_len: usize,
    must_strip_sampling: bool,
) -> Result<bool, AttemptOutcome> {
    if must_strip_sampling && body_len > REQUEST_FILTER_LIMIT_BYTES {
        return Err(openai_responses_sampling_params_payload_too_large());
    }
    if !should_strip_openai_responses_sampling_params(provider, upstream_path, meta) {
        return Ok(false);
    }
    let mut changed = false;
    changed |= object.remove("temperature").is_some();
    changed |= object.remove("top_p").is_some();
    Ok(changed)
}

fn openai_responses_sampling_params_payload_too_large() -> AttemptOutcome {
    AttemptOutcome::Fatal(http::error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "OpenAI Responses reasoning model request is too large to validate sampling parameters; limit is {REQUEST_FILTER_LIMIT_BYTES} bytes."
        ),
    ))
}

fn should_rewrite_developer_roles(upstream: &UpstreamRuntime, upstream_path: &str) -> bool {
    upstream.rewrite_developer_role_to_system
        && (upstream_path == OPENAI_CHAT_PATH || upstream_path == OPENAI_RESPONSES_PATH)
}

fn rewrite_developer_roles_if_needed(
    upstream: &UpstreamRuntime,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_FILTER_LIMIT_BYTES {
        return false;
    }
    if !should_rewrite_developer_roles(upstream, upstream_path) {
        return false;
    }
    if upstream_path == OPENAI_CHAT_PATH {
        return rewrite_chat_developer_roles(object);
    }
    rewrite_responses_developer_roles(object)
}

fn should_filter_anthropic_count_tokens_request(provider: &str, upstream_path: &str) -> bool {
    provider == "anthropic" && upstream_path == ANTHROPIC_COUNT_TOKENS_PATH
}

fn filter_anthropic_count_tokens_request(
    provider: &str,
    upstream_path: &str,
    object: &mut Map<String, Value>,
    body_len: usize,
) -> bool {
    if body_len > REQUEST_FILTER_LIMIT_BYTES {
        return false;
    }
    if !should_filter_anthropic_count_tokens_request(provider, upstream_path) {
        return false;
    }

    // Anthropic count_tokens rejects generation-only fields accepted by messages.
    let mut changed = false;
    for key in [
        "temperature",
        "top_p",
        "top_k",
        "stream",
        "stop_sequences",
        "stop",
        "metadata",
    ] {
        changed |= object.remove(key).is_some();
    }
    if changed {
        tracing::debug!("filtered Anthropic count_tokens generation-only fields");
    }
    changed
}

fn should_inject_codex_installation_id(
    provider: &str,
    codex_openai_device_id: Option<&str>,
) -> bool {
    provider == "codex"
        && codex_openai_device_id
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

fn inject_codex_installation_id(
    object: &mut Map<String, Value>,
    provider: &str,
    codex_openai_device_id: Option<&str>,
) -> bool {
    if provider != "codex" {
        return false;
    }
    let Some(device_id) = codex_openai_device_id.map(str::trim) else {
        return false;
    };
    if device_id.is_empty() {
        return false;
    }

    // Codex OAuth requests expect the account installation id inside client metadata.
    let client_metadata = ensure_json_object_field(object, "client_metadata");
    if client_metadata.contains_key(CODEX_INSTALLATION_ID_KEY) {
        return false;
    }
    client_metadata.insert(
        CODEX_INSTALLATION_ID_KEY.to_string(),
        Value::String(device_id.to_string()),
    );
    tracing::debug!("injected Codex installation id into client_metadata");
    true
}

fn replayable_from_json(value: Value) -> Result<ReplayableBody, AttemptOutcome> {
    let outbound_bytes = serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to serialize request: {err}"),
        ))
    })?;
    Ok(ReplayableBody::from_bytes(outbound_bytes))
}

#[cfg(test)]
async fn maybe_rewrite_developer_role_to_system(
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    if !upstream.rewrite_developer_role_to_system {
        return Ok(None);
    }

    let upstream_path = split_path_query(upstream_path_with_query).0;
    if upstream_path != OPENAI_CHAT_PATH && upstream_path != OPENAI_RESPONSES_PATH {
        return Ok(None);
    }

    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_FILTER_LIMIT_BYTES)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read cached request body: {err}"),
            ))
        })?
    else {
        return Ok(None);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };

    let changed = if upstream_path == OPENAI_CHAT_PATH {
        rewrite_chat_developer_roles(object)
    } else {
        rewrite_responses_developer_roles(object)
    };
    if !changed {
        return Ok(None);
    }

    let outbound_bytes = serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to serialize request: {err}"),
        ))
    })?;
    Ok(Some(ReplayableBody::from_bytes(outbound_bytes)))
}

fn rewrite_chat_developer_roles(object: &mut serde_json::Map<String, Value>) -> bool {
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for message in messages {
        let Some(item) = message.as_object_mut() else {
            continue;
        };
        changed |= rewrite_role_field(item);
    }
    changed
}

fn rewrite_responses_developer_roles(object: &mut serde_json::Map<String, Value>) -> bool {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        changed |= rewrite_role_field(item);
    }
    changed
}

fn rewrite_role_field(object: &mut serde_json::Map<String, Value>) -> bool {
    let Some(role) = object.get_mut("role") else {
        return false;
    };
    if role.as_str() != Some("developer") {
        return false;
    }
    *role = Value::String("system".to_string());
    true
}

#[cfg(test)]
async fn maybe_filter_openai_responses_request_fields(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    let should_filter_prompt_cache_retention = upstream.filter_prompt_cache_retention;
    let should_filter_safety_identifier = upstream.filter_safety_identifier;
    if provider != "openai-response"
        || (!should_filter_prompt_cache_retention && !should_filter_safety_identifier)
    {
        return Ok(None);
    }
    let upstream_path = split_path_query(upstream_path_with_query).0;
    if upstream_path != OPENAI_RESPONSES_PATH {
        return Ok(None);
    }

    let Some(bytes) = body
        .read_bytes_if_small(REQUEST_FILTER_LIMIT_BYTES)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read cached request body: {err}"),
            ))
        })?
    else {
        // Best-effort: request body too large to rewrite.
        return Ok(None);
    };

    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    let mut changed = false;
    if should_filter_prompt_cache_retention {
        changed = changed || object.remove("prompt_cache_retention").is_some();
    }
    if should_filter_safety_identifier {
        changed = changed || object.remove("safety_identifier").is_some();
    }
    if !changed {
        return Ok(None);
    }

    let outbound_bytes = serde_json::to_vec(&value).map(Bytes::from).map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to serialize request: {err}"),
        ))
    })?;
    Ok(Some(ReplayableBody::from_bytes(outbound_bytes)))
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "request_body.test.rs"]
mod tests;
