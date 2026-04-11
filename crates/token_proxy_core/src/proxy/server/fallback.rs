use axum::{
    http::{HeaderMap, Method, Uri},
    response::Response,
};
use std::{collections::HashSet, sync::Arc, time::Instant};

use super::super::upstream::forward_upstream_request;
use super::{
    dispatch::resolve_retry_fallback_plan, execute::forward_retry_fallback_request,
    prepared::PreparedRequest, ProxyState,
};

pub(super) async fn forward_with_provider_fallbacks(
    state: Arc<ProxyState>,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
) -> Response {
    let primary = forward_upstream_request(
        state.clone(),
        method.clone(),
        prepared.plan.provider,
        &prepared.path,
        None,
        &prepared.outbound_path_with_query,
        headers,
        &prepared.outbound_body,
        &prepared.meta,
        &prepared.request_auth,
        prepared.client_gemini_api_key.clone(),
        prepared.plan.response_transform,
        prepared.request_detail.clone(),
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

    current_response
}
