use std::time::Instant;

use axum::http::{HeaderMap, Method};

use super::retry::{
    finalize_attempt, retry_after_kiro_refresh, retry_with_next_codex_account, CodexFailoverResult,
    UpstreamAttempt, UpstreamAttemptFailure,
};
use super::transport::send_upstream_request;
use super::{AttemptOutcome, PreparedUpstreamRequest};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::{config::UpstreamRuntime, ProxyState, RequestMeta};

pub(super) async fn attempt_upstream(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &crate::proxy::http::RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> AttemptOutcome {
    if provider == "kiro" {
        return super::kiro::attempt_kiro_upstream(
            state,
            method,
            upstream,
            inbound_path,
            headers,
            body,
            meta,
            response_transform,
            request_detail,
        )
        .await;
    }
    let first = attempt_send(
        state,
        method.clone(),
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        request_detail.as_ref(),
    )
    .await;
    let first = match retry_with_next_codex_account(
        state,
        method.clone(),
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        client_gemini_api_key,
        response_transform,
        request_detail.clone(),
        first,
    )
    .await
    {
        CodexFailoverResult::Pending(attempt) => attempt,
        CodexFailoverResult::Resolved(outcome) => return outcome,
    };
    if let Some(outcome) = retry_after_kiro_refresh(
        state,
        method.clone(),
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        client_gemini_api_key,
        response_transform,
        request_detail.clone(),
        &first,
    )
    .await
    {
        return outcome;
    }
    finalize_attempt(
        state,
        provider,
        upstream,
        inbound_path,
        client_gemini_api_key,
        response_transform,
        request_detail,
        first,
    )
    .await
}

pub(super) async fn attempt_send(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &crate::proxy::http::RequestAuth,
    request_detail: Option<&RequestDetailSnapshot>,
) -> Result<UpstreamAttempt, UpstreamAttemptFailure> {
    let prepared = super::prepare_upstream_request(
        state,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        meta,
        request_auth,
    )
    .await
    .map_err(|outcome| UpstreamAttemptFailure {
        outcome,
        selected_account_id: None,
    })?;
    let PreparedUpstreamRequest {
        upstream_path_with_query,
        upstream_url,
        request_headers,
        proxy_url,
        selected_account_id,
        meta,
    } = prepared;
    let start_time = Instant::now();
    let response = send_upstream_request(
        state,
        method,
        provider,
        upstream,
        inbound_path,
        &upstream_path_with_query,
        &upstream_url,
        proxy_url.as_deref(),
        &request_headers,
        body,
        &meta,
        selected_account_id.as_deref(),
        request_detail,
        start_time,
    )
    .await
    .map_err(|outcome| UpstreamAttemptFailure {
        outcome,
        selected_account_id: selected_account_id.clone(),
    })?;
    Ok(UpstreamAttempt {
        response,
        selected_account_id,
        meta,
        start_time,
    })
}
