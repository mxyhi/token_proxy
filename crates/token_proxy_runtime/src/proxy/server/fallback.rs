use axum::{
    body::{to_bytes, Body},
    http::{header::CONTENT_LENGTH, HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use serde_json::Value;
use std::{collections::HashSet, sync::Arc, time::Instant};

use super::super::upstream::forward_upstream_request;
use super::{
    dispatch::resolve_retry_fallback_plan, execute::forward_retry_fallback_request,
    prepared::PreparedRequest, ProxyState,
};
use crate::proxy::{
    config::InboundApiFormat, cooldown_scope::CooldownScope, inbound::detect_inbound_api_format,
    openai_compat::FormatTransform,
};

const CODEX_PROVIDER: &str = "codex";

pub(super) async fn forward_with_provider_fallbacks(
    state: Arc<ProxyState>,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
) -> Response {
    let codex_cooldown_scope = CooldownScope::codex_responses_request(
        &state.config,
        detect_inbound_api_format(&prepared.path),
        headers,
    );
    let primary_inbound_format = bridge_inbound_format(prepared.plan.request_transform);
    let primary = forward_upstream_request(
        state.clone(),
        method.clone(),
        prepared.plan.provider,
        &prepared.path,
        primary_inbound_format,
        &prepared.outbound_path_with_query,
        headers,
        &prepared.outbound_body,
        &prepared.meta,
        &prepared.request_auth,
        prepared.client_gemini_api_key.clone(),
        prepared.plan.response_transform,
        prepared.request_detail.clone(),
        &codex_cooldown_scope,
    )
    .await;

    let mut current_response = primary.response;
    let mut current_provider = prepared.plan.provider;
    let mut should_fallback = primary.should_fallback;
    let mut attempted_fallback_providers = HashSet::from([current_provider]);

    while should_fallback {
        let Some(fallback_plan) =
            resolve_retry_fallback_plan(&state.config, &prepared.path, current_provider)
        else {
            tracing::warn!(
                path = %prepared.path,
                primary = %current_provider,
                "primary provider exhausted, but no compatible alternate provider is available"
            );
            break;
        };
        if !attempted_fallback_providers.insert(fallback_plan.provider) {
            tracing::warn!(
                path = %prepared.path,
                provider = %fallback_plan.provider,
                "alternate provider fallback cycle detected"
            );
            break;
        }
        tracing::warn!(
            path = %prepared.path,
            primary = %current_provider,
            fallback = %fallback_plan.provider,
            "primary provider exhausted, falling back to alternate provider"
        );
        match forward_retry_fallback_request(
            state.clone(),
            method.clone(),
            uri,
            headers,
            prepared,
            request_start,
            &fallback_plan,
            &codex_cooldown_scope,
        )
        .await
        {
            Ok(fallback) => {
                current_provider = fallback_plan.provider;
                should_fallback = fallback.should_fallback;
                current_response = fallback.response;
            }
            Err(_) => {
                tracing::warn!(
                    path = %prepared.path,
                    primary = %current_provider,
                    fallback = %fallback_plan.provider,
                    "alternate provider fallback aborted before dispatch"
                );
                break;
            }
        }
    }

    current_response =
        augment_codex_models_manifest(state.clone(), headers, prepared, current_response).await;
    finalize_codex_responses_cooldown(&state, &codex_cooldown_scope, current_response.status());
    state
        .codex_turn_state
        .note_committed_response(headers, &current_response);
    current_response
}

async fn augment_codex_models_manifest(
    state: Arc<ProxyState>,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    response: Response,
) -> Response {
    if prepared.plan.provider != CODEX_PROVIDER
        || prepared.path != "/v1/models"
        || response.status() != StatusCode::OK
    {
        return response;
    }

    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, state.config.max_request_body_bytes).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(error = %error, "failed to read Codex models manifest for augmentation");
            return Response::from_parts(parts, Body::empty());
        }
    };
    let mut manifest = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(error = %error, "kept non-JSON Codex models manifest unchanged");
            return Response::from_parts(parts, Body::from(bytes));
        }
    };
    let Some(models) = manifest.get_mut("models").and_then(Value::as_array_mut) else {
        tracing::debug!("Codex models manifest has no models array; kept upstream body");
        return Response::from_parts(parts, Body::from(bytes));
    };

    let mut known_ids = models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>();
    let entries = super::super::upstream::collect_model_catalog_entries_for_manifest(
        state.as_ref(),
        headers,
        &prepared.request_auth,
    )
    .await;
    let mut added = 0usize;
    for (id, display_name) in entries {
        if !known_ids.insert(id.clone()) {
            continue;
        }
        let display_name = display_name.unwrap_or_else(|| id.clone());
        models.push(serde_json::json!({
            "slug": id,
            "display_name": display_name,
        }));
        added += 1;
    }
    if added == 0 {
        return Response::from_parts(parts, Body::from(bytes));
    }

    tracing::info!(
        added,
        "augmented Codex models manifest with local model entries"
    );
    let output = match serde_json::to_vec(&manifest) {
        Ok(output) => output,
        Err(error) => {
            tracing::warn!(error = %error, "failed to serialize augmented Codex models manifest");
            return Response::from_parts(parts, Body::from(bytes));
        }
    };
    let mut parts = parts;
    parts.headers.remove(CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(output))
}

fn bridge_inbound_format(transform: FormatTransform) -> Option<InboundApiFormat> {
    match transform {
        FormatTransform::ImagesGenerationsToCodex => Some(InboundApiFormat::OpenaiResponses),
        _ => None,
    }
}

fn finalize_codex_responses_cooldown(
    state: &ProxyState,
    scope: &CooldownScope,
    status: axum::http::StatusCode,
) {
    // Session-scoped cooldown follows the final client-visible result, not
    // intermediate same-turn failover attempts. Request scopes are always cleared.
    if scope.is_global() || (!status.is_success() && !scope.is_request()) {
        return;
    }
    state
        .account_selector
        .clear_provider_scope(CODEX_PROVIDER, scope);
    state
        .upstream_selector
        .clear_provider_scope(CODEX_PROVIDER, scope);
}
