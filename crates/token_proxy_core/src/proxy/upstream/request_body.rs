use axum::{body::Bytes, http::StatusCode};
use serde_json::Value;

use super::super::{
    config::UpstreamRuntime, http, model, request_body::ReplayableBody, RequestMeta,
};
use super::{request::split_path_query, AttemptOutcome};

const OPENAI_CHAT_PATH: &str = "/v1/chat/completions";
const OPENAI_RESPONSES_PATH: &str = "/v1/responses";
// Keep in sync with server_helpers request transform limit (20 MiB).
const REQUEST_FILTER_LIMIT_BYTES: usize = 20 * 1024 * 1024;

pub(super) async fn build_upstream_body(
    provider: &str,
    upstream: &UpstreamRuntime,
    upstream_path_with_query: &str,
    body: &ReplayableBody,
    meta: &RequestMeta,
) -> Result<reqwest::Body, AttemptOutcome> {
    let mapped_body = maybe_rewrite_request_body_model(body, meta).await?;
    let mapped_source = mapped_body.as_ref().unwrap_or(body);
    let upstream_path = split_path_query(upstream_path_with_query).0;
    let reasoning_body =
        match super::super::server_helpers::maybe_rewrite_openai_reasoning_effort_from_model_suffix(
            provider,
            upstream_path,
            meta,
            mapped_source,
        )
        .await
        {
            Ok(body) => body,
            Err(err) => {
                return Err(AttemptOutcome::Fatal(http::error_response(
                    err.status,
                    err.message,
                )))
            }
        };
    let source = reasoning_body
        .as_ref()
        .or(mapped_body.as_ref())
        .unwrap_or(body);
    let filtered = maybe_filter_openai_responses_request_fields(
        provider,
        upstream,
        upstream_path_with_query,
        source,
    )
    .await?;
    let rewrite = maybe_rewrite_developer_role_to_system(
        upstream,
        upstream_path_with_query,
        filtered.as_ref().unwrap_or(source),
    )
    .await?;
    let final_source = rewrite.as_ref().or(filtered.as_ref()).unwrap_or(source);
    final_source.to_reqwest_body().await.map_err(|err| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Failed to read cached request body: {err}"),
        ))
    })
}

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

async fn maybe_rewrite_request_body_model(
    body: &ReplayableBody,
    meta: &RequestMeta,
) -> Result<Option<ReplayableBody>, AttemptOutcome> {
    if meta.model_override().is_none() {
        return Ok(None);
    }
    let Some(mapped_model) = meta.mapped_model.as_deref() else {
        return Ok(None);
    };
    let Some(bytes) = body
        .read_bytes_if_small(super::REQUEST_MODEL_MAPPING_LIMIT_BYTES)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read request body: {err}"),
            ))
        })?
    else {
        return Ok(None);
    };
    let Some(rewritten) = model::rewrite_request_model(&bytes, mapped_model) else {
        return Ok(None);
    };
    Ok(Some(ReplayableBody::from_bytes(rewritten)))
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "request_body.test.rs"]
mod tests;
