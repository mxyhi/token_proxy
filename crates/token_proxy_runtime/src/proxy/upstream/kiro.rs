use super::kiro_http::{handle_send_error, send_kiro_request};
use super::kiro_prepare::{
    build_endpoint_payload, prepare_kiro_context, refresh_and_rebuild_payload, KiroContext,
};
use super::kiro_result::{finalize_response, handle_response_action, ResponseAction};
use super::AttemptOutcome;
use crate::proxy::http;
use crate::proxy::kiro::KiroEndpointConfig;
use crate::proxy::log::RequestTimings;
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::{ProxyState, RequestMeta};
use axum::http::{HeaderMap, Method, StatusCode};
use std::time::Instant;
use token_proxy_config::UpstreamRuntime;

pub(super) const MAX_KIRO_RETRIES: usize = 2;
pub(super) const MAX_KIRO_BACKOFF_SECS: u64 = 30;

/// Kiro 账户型 upstream 固定使用 credential account_id；无 same-upstream 账户 failover。
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
        Err(outcome) => return outcome,
    };
    tracing::debug!(
        account_id = context.account_id.as_str(),
        upstream = %upstream.id,
        "attempting pinned kiro upstream"
    );
    run_kiro_endpoints(&mut context).await
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
        let (response, start_time, timings, token_tracker) =
            match send_endpoint_request(context, endpoint, &payload.payload).await {
                Ok(result) => result,
                Err(outcome) => return EndpointOutcome::Done(outcome),
            };

        match handle_response_action(context, response, start_time, attempt, is_last).await {
            ResponseAction::RetryAfter(delay) => {
                // 本轮不 finalize，释放 TTFB 期间的 connections 计数。
                drop(token_tracker);
                tokio::time::sleep(delay).await;
                continue;
            }
            ResponseAction::RefreshAndRetry => {
                drop(token_tracker);
                match refresh_and_rebuild_payload(context, endpoint).await {
                    Ok(updated) => payload = updated,
                    Err(outcome) => return EndpointOutcome::Done(outcome),
                }
                continue;
            }
            ResponseAction::NextEndpoint => {
                drop(token_tracker);
                return EndpointOutcome::Continue;
            }
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
                        timings,
                        token_tracker,
                    )
                    .await,
                );
            }
            ResponseAction::Return(outcome) => {
                drop(token_tracker);
                return EndpointOutcome::Done(outcome);
            }
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
) -> Result<
    (
        reqwest::Response,
        Instant,
        RequestTimings,
        crate::proxy::token_rate::RequestTokenTracker,
    ),
    AttemptOutcome,
> {
    let model_for_tokens = context
        .mapped_meta
        .mapped_model
        .as_deref()
        .or(context.mapped_meta.original_model.as_deref())
        .map(|value| value.to_string());
    // 发送前 register，保证 Kiro TTFB 期间托盘 connections 可见。
    let token_tracker = context
        .state
        .token_rate
        .register(model_for_tokens, None)
        .await;
    tracing::debug!(
        account_id = %context.account_id,
        endpoint = %endpoint.url,
        "token_rate register before kiro send"
    );
    let start_time = Instant::now();
    let timings = RequestTimings::with_billing(context.mapped_meta.billing.clone());
    let response = match send_kiro_request(
        &context.client,
        context.method.clone(),
        &endpoint.url,
        &context.record.access_token,
        endpoint.amz_target,
        context.is_idc,
        payload,
        context.upstream.header_overrides.as_deref(),
        context.state.config.sync_response_timeout,
    )
    .await
    {
        Ok(response) => {
            timings.mark_upstream_response_headers(start_time.elapsed().as_millis());
            response
        }
        Err(err) => {
            drop(token_tracker);
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
    Ok((response, start_time, timings, token_tracker))
}
