use super::kiro_http::{handle_send_error, send_kiro_request};
use super::kiro_prepare::{
    build_endpoint_payload, prepare_kiro_context, refresh_and_rebuild_payload, KiroContext,
};
use super::kiro_result::{finalize_response, handle_response_action, ResponseAction};
use super::AttemptOutcome;
use crate::proxy::http;
use crate::proxy::kiro::KiroEndpointConfig;
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::{config::UpstreamRuntime, request_detail::RequestDetailSnapshot};
use crate::proxy::{ProxyState, RequestMeta};
use axum::http::{HeaderMap, Method, StatusCode};
use std::time::Instant;

pub(super) const MAX_KIRO_RETRIES: usize = 2;
pub(super) const MAX_KIRO_BACKOFF_SECS: u64 = 30;

pub(super) async fn attempt_kiro_upstream(
    state: &ProxyState,
    method: Method,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> AttemptOutcome {
    if upstream
        .kiro_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return attempt_kiro_single_account(
            state,
            upstream,
            body,
            meta,
            headers,
            method,
            inbound_path,
            response_transform,
            request_detail,
        )
        .await
        .outcome;
    }

    let mut excluded_account_ids: Vec<String> = Vec::new();
    let mut current_upstream = upstream.clone();
    loop {
        let result = attempt_kiro_single_account(
            state,
            &current_upstream,
            body,
            meta,
            headers,
            method.clone(),
            inbound_path,
            response_transform,
            request_detail.clone(),
        )
        .await;
        let Some(account_id) = result.selected_account_id.clone() else {
            return result.outcome;
        };
        if !should_failover_kiro_account(&result.outcome) {
            return result.outcome;
        }

        mark_failed_kiro_account_before_failover(state, &account_id, &result.outcome);
        excluded_account_ids.push(account_id);
        let Some(next_account_id) =
            resolve_next_kiro_account_id(state, &excluded_account_ids).await
        else {
            return into_group_retryable_kiro_outcome(result.outcome);
        };
        current_upstream.kiro_account_id = Some(next_account_id);
    }
}

struct KiroAttemptResult {
    outcome: AttemptOutcome,
    selected_account_id: Option<String>,
}

async fn attempt_kiro_single_account(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    body: &ReplayableBody,
    meta: &RequestMeta,
    headers: &HeaderMap,
    method: Method,
    inbound_path: &str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> KiroAttemptResult {
    let mut context = match prepare_kiro_context(
        state,
        upstream,
        body,
        meta,
        headers,
        method,
        inbound_path,
        response_transform,
        request_detail,
    )
    .await
    {
        Ok(context) => context,
        Err(outcome) => {
            return KiroAttemptResult {
                outcome,
                selected_account_id: None,
            };
        }
    };
    let selected_account_id = Some(context.account_id.clone());
    let outcome = run_kiro_endpoints(&mut context).await;
    KiroAttemptResult {
        outcome,
        selected_account_id,
    }
}

enum EndpointOutcome {
    Continue,
    Done(AttemptOutcome),
}

async fn run_kiro_endpoints(context: &mut KiroContext<'_>) -> AttemptOutcome {
    let endpoints = context.endpoints.clone();
    let total = endpoints.len();
    for (index, endpoint) in endpoints.iter().enumerate() {
        let is_last = index + 1 >= total;
        match attempt_endpoint(context, endpoint, is_last).await {
            EndpointOutcome::Continue => continue,
            EndpointOutcome::Done(outcome) => return outcome,
        }
    }

    AttemptOutcome::Fatal(http::error_response(
        StatusCode::BAD_GATEWAY,
        "Kiro upstream request failed.",
    ))
}

async fn attempt_endpoint(
    context: &mut KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
    is_last: bool,
) -> EndpointOutcome {
    let mut payload = match build_endpoint_payload(context, endpoint).await {
        Ok(payload) => payload,
        Err(outcome) => return EndpointOutcome::Done(outcome),
    };

    for attempt in 0..=MAX_KIRO_RETRIES {
        let (response, start_time) =
            match send_endpoint_request(context, endpoint, &payload.payload).await {
                Ok(result) => result,
                Err(outcome) => return EndpointOutcome::Done(outcome),
            };

        match handle_response_action(context, response, start_time, attempt, is_last).await {
            ResponseAction::RetryAfter(delay) => {
                tokio::time::sleep(delay).await;
                continue;
            }
            ResponseAction::RefreshAndRetry => {
                match refresh_and_rebuild_payload(context, endpoint).await {
                    Ok(updated) => payload = updated,
                    Err(outcome) => return EndpointOutcome::Done(outcome),
                }
                continue;
            }
            ResponseAction::NextEndpoint => return EndpointOutcome::Continue,
            ResponseAction::Finalize(response, start_time) => {
                return EndpointOutcome::Done(
                    finalize_response(
                        context.state,
                        &context.mapped_meta,
                        context.upstream,
                        Some(context.account_id.clone()),
                        context.inbound_path,
                        context.response_transform,
                        context.request_detail.clone(),
                        response,
                        false,
                        start_time,
                    )
                    .await,
                );
            }
            ResponseAction::Return(outcome) => return EndpointOutcome::Done(outcome),
        }
    }

    EndpointOutcome::Done(AttemptOutcome::Fatal(http::error_response(
        StatusCode::BAD_GATEWAY,
        "Kiro upstream request failed.",
    )))
}

async fn send_endpoint_request(
    context: &KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
    payload: &[u8],
) -> Result<(reqwest::Response, Instant), AttemptOutcome> {
    let start_time = Instant::now();
    let response = match send_kiro_request(
        &context.client,
        context.method.clone(),
        &endpoint.url,
        &context.record.access_token,
        endpoint.amz_target,
        context.is_idc,
        payload,
        context.upstream.header_overrides.as_deref(),
        context.state.config.upstream_no_data_timeout,
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            let outcome = handle_send_error(
                context.state,
                &context.mapped_meta,
                context.upstream,
                Some(context.account_id.clone()),
                context.inbound_path,
                context.response_transform,
                context.request_detail.clone(),
                err,
                start_time,
            )
            .await;
            return Err(outcome);
        }
    };
    Ok((response, start_time))
}

fn should_failover_kiro_account(outcome: &AttemptOutcome) -> bool {
    match outcome {
        AttemptOutcome::Success(response) => !response.status().is_success(),
        AttemptOutcome::Retryable { .. } => true,
        AttemptOutcome::Fatal(_) | AttemptOutcome::SkippedAuth => false,
    }
}

fn mark_failed_kiro_account_before_failover(
    state: &ProxyState,
    account_id: &str,
    outcome: &AttemptOutcome,
) {
    match outcome {
        AttemptOutcome::Success(response) if !response.status().is_success() => {
            let _ = state.account_selector.mark_response_status(
                "kiro",
                account_id,
                response.status(),
                response.headers(),
            );
        }
        AttemptOutcome::Retryable { .. } => {
            let _ = state
                .account_selector
                .mark_retryable_failure("kiro", account_id);
        }
        _ => {}
    }
}

async fn resolve_next_kiro_account_id(
    state: &ProxyState,
    excluded_account_ids: &[String],
) -> Option<String> {
    let ordered_account_ids = super::ordered_runtime_account_ids(state, "kiro").await;
    let ordered_account_ids = ordered_account_ids
        .into_iter()
        .filter(|account_id| !excluded_account_ids.iter().any(|value| value == account_id))
        .collect::<Vec<_>>();
    if ordered_account_ids.is_empty() {
        return None;
    }
    state
        .kiro_accounts
        .resolve_account_record_with_order(None, Some(ordered_account_ids.as_slice()))
        .await
        .ok()
        .map(|(account_id, _)| account_id)
}

fn into_group_retryable_kiro_outcome(outcome: AttemptOutcome) -> AttemptOutcome {
    match outcome {
        AttemptOutcome::Success(response)
            if super::utils::is_retryable_status(response.status()) =>
        {
            let status = response.status();
            AttemptOutcome::Retryable {
                message: format!("Upstream responded with {}", status.as_u16()),
                response: Some(response),
                is_timeout: false,
                should_cooldown: super::result::should_cooldown_retryable_status(status),
            }
        }
        other => other,
    }
}
