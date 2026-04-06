use super::kiro_http::{
    build_client, handle_send_error, read_request_json, refresh_kiro_account, send_kiro_request,
};
use super::{result, AttemptOutcome};
use crate::kiro::KiroTokenRecord;
use crate::proxy::http;
use crate::proxy::kiro::{
    build_payload_from_claude, build_payload_from_responses, determine_agentic_mode,
    map_model_to_kiro, select_endpoints, BuildPayloadResult, KiroEndpointConfig,
};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::{config::UpstreamRuntime, request_detail::RequestDetailSnapshot};
use crate::proxy::{ProxyState, RequestMeta};
use axum::body::Body;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode};
use serde_json::Value;
use std::time::{Duration, Instant};

const MAX_KIRO_RETRIES: usize = 2;
const MAX_KIRO_BACKOFF_SECS: u64 = 30;

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

struct KiroContext<'a> {
    state: &'a ProxyState,
    method: Method,
    upstream: &'a UpstreamRuntime,
    inbound_path: &'a str,
    headers: &'a HeaderMap,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    mapped_meta: RequestMeta,
    request_value: Value,
    account_id: String,
    record: KiroTokenRecord,
    profile_arn: Option<String>,
    endpoints: Vec<KiroEndpointConfig>,
    is_idc: bool,
    model_id: String,
    is_agentic: bool,
    is_chat_only: bool,
    source_format: KiroSourceFormat,
    client: reqwest::Client,
}

#[derive(Clone, Copy, Debug)]
enum KiroSourceFormat {
    Responses,
    Anthropic,
}

enum EndpointOutcome {
    Continue,
    Done(AttemptOutcome),
}

enum ResponseAction {
    RetryAfter(Duration),
    RefreshAndRetry,
    NextEndpoint,
    Finalize(reqwest::Response, Instant),
    Return(AttemptOutcome),
}

async fn prepare_kiro_context<'a>(
    state: &'a ProxyState,
    upstream: &'a UpstreamRuntime,
    body: &ReplayableBody,
    meta: &RequestMeta,
    headers: &'a HeaderMap,
    method: Method,
    inbound_path: &'a str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> Result<KiroContext<'a>, AttemptOutcome> {
    let mapped_meta = super::build_mapped_meta(meta, upstream);
    let request_value = read_request_json(state, body).await?;
    let ordered_account_ids = if upstream
        .kiro_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        None
    } else {
        Some(super::ordered_runtime_account_ids(state, "kiro").await)
    };
    let (account_id, record) = state
        .kiro_accounts
        .resolve_account_record_with_order(
            upstream.kiro_account_id.as_deref(),
            ordered_account_ids.as_deref(),
        )
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err))
        })?;
    let is_idc = record.auth_method.trim().eq_ignore_ascii_case("idc");
    let profile_arn = resolve_profile_arn(&record);
    let endpoints = resolve_endpoints(state, upstream, is_idc);
    let (model_id, is_agentic, is_chat_only) = resolve_model(&mapped_meta);
    let source_format = resolve_source_format(response_transform);
    let client_proxy_url = record
        .proxy_url
        .clone()
        .or_else(|| upstream.proxy_url.clone())
        .or(state.kiro_accounts.app_proxy_url().await);
    let client = build_client(state, client_proxy_url.as_deref())?;

    Ok(KiroContext {
        state,
        method,
        upstream,
        inbound_path,
        headers,
        response_transform,
        request_detail,
        mapped_meta,
        request_value,
        account_id,
        record,
        profile_arn,
        endpoints,
        is_idc,
        model_id,
        is_agentic,
        is_chat_only,
        source_format,
        client,
    })
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

async fn build_endpoint_payload(
    context: &KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
) -> Result<BuildPayloadResult, AttemptOutcome> {
    let payload = match context.source_format {
        KiroSourceFormat::Anthropic => build_payload_from_anthropic(context, endpoint.origin).await,
        KiroSourceFormat::Responses => build_payload_from_responses(
            &context.request_value,
            &context.model_id,
            context.profile_arn.as_deref(),
            endpoint.origin,
            context.is_agentic,
            context.is_chat_only,
            context.headers,
        ),
    };
    payload.map_err(|message| {
        AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_REQUEST, message))
    })
}

async fn handle_response_action(
    context: &mut KiroContext<'_>,
    response: reqwest::Response,
    start_time: Instant,
    attempt: usize,
    is_last: bool,
) -> ResponseAction {
    let status = response.status();
    // Kiro-specific retry/fallback: 5xx backoff, 401 refresh, 403 token-only refresh, 429 endpoint switch.
    if status == StatusCode::TOO_MANY_REQUESTS {
        return if is_last {
            ResponseAction::Finalize(response, start_time)
        } else {
            ResponseAction::NextEndpoint
        };
    }
    if status.is_server_error() {
        if attempt < MAX_KIRO_RETRIES {
            return ResponseAction::RetryAfter(backoff_delay(attempt));
        }
        return ResponseAction::Finalize(response, start_time);
    }
    if status == StatusCode::UNAUTHORIZED {
        if attempt < MAX_KIRO_RETRIES {
            return ResponseAction::RefreshAndRetry;
        }
        return ResponseAction::Finalize(response, start_time);
    }
    if status == StatusCode::FORBIDDEN {
        return handle_forbidden_response(context, response, start_time, attempt).await;
    }
    if status == StatusCode::PAYMENT_REQUIRED {
        return ResponseAction::Finalize(response, start_time);
    }

    ResponseAction::Finalize(response, start_time)
}

async fn handle_forbidden_response(
    context: &mut KiroContext<'_>,
    response: reqwest::Response,
    start_time: Instant,
    attempt: usize,
) -> ResponseAction {
    let status = response.status();
    let headers = response.headers().clone();
    let body = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            let message = format!("Failed to read upstream response: {err}");
            return ResponseAction::Return(AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                message,
            )));
        }
    };
    let body_text = String::from_utf8_lossy(&body);

    if contains_suspended_flag(&body_text) {
        let outcome = build_error_outcome(context, status, &headers, body, start_time);
        return ResponseAction::Return(outcome);
    }

    if contains_token_error(&body_text) && attempt < MAX_KIRO_RETRIES {
        return ResponseAction::RefreshAndRetry;
    }

    let outcome = build_error_outcome(context, status, &headers, body, start_time);
    ResponseAction::Return(outcome)
}

async fn refresh_and_rebuild_payload(
    context: &mut KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
) -> Result<BuildPayloadResult, AttemptOutcome> {
    refresh_kiro_account(context.state, &context.account_id).await?;
    context.record = load_account_record(context.state, &context.account_id).await?;
    let was_idc = context.is_idc;
    context.is_idc = context
        .record
        .auth_method
        .trim()
        .eq_ignore_ascii_case("idc");
    if context.is_idc != was_idc {
        context.endpoints = resolve_endpoints(context.state, context.upstream, context.is_idc);
    }
    build_endpoint_payload(context, endpoint).await
}

fn backoff_delay(attempt: usize) -> Duration {
    let exp = 1u64 << attempt;
    Duration::from_secs(exp.min(MAX_KIRO_BACKOFF_SECS))
}

fn contains_suspended_flag(body: &str) -> bool {
    let upper = body.to_ascii_uppercase();
    upper.contains("SUSPENDED") || upper.contains("TEMPORARILY_SUSPENDED")
}

fn contains_token_error(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("expired")
        || lower.contains("invalid")
        || lower.contains("unauthorized")
}

fn build_error_outcome(
    context: &KiroContext<'_>,
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
    start_time: Instant,
) -> AttemptOutcome {
    let message = summarize_error_body(&body);
    result::log_upstream_error_if_needed(
        &context.state.log,
        context.request_detail.as_ref(),
        &context.mapped_meta,
        "kiro",
        &context.upstream.id,
        Some(context.account_id.as_str()),
        context.inbound_path,
        status,
        message,
        start_time,
    );
    AttemptOutcome::Success(build_passthrough_response(status, headers, body))
}

fn build_passthrough_response(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let filtered = http::filter_response_headers(headers);
    http::build_response(status, filtered, Body::from(body))
}

fn summarize_error_body(body: &Bytes) -> String {
    const LIMIT: usize = 2048;
    let text = String::from_utf8_lossy(body);
    if text.len() > LIMIT {
        format!("{}…", &text[..LIMIT])
    } else {
        text.to_string()
    }
}

async fn build_payload_from_anthropic(
    context: &KiroContext<'_>,
    origin: &str,
) -> Result<BuildPayloadResult, String> {
    build_payload_from_claude(
        &context.request_value,
        &context.model_id,
        context.profile_arn.as_deref(),
        origin,
        context.is_agentic,
        context.is_chat_only,
        context.headers,
    )
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

fn resolve_profile_arn(record: &KiroTokenRecord) -> Option<String> {
    match record.auth_method.as_str() {
        "builder-id" | "idc" => None,
        _ => record.profile_arn.clone(),
    }
}

async fn load_account_record(
    state: &ProxyState,
    account_id: &str,
) -> Result<KiroTokenRecord, AttemptOutcome> {
    state
        .kiro_accounts
        .get_account_record(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))
}

fn resolve_endpoints(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    is_idc: bool,
) -> Vec<KiroEndpointConfig> {
    let preferred = upstream
        .kiro_preferred_endpoint
        .clone()
        .or(state.config.kiro_preferred_endpoint.clone());
    select_endpoints(preferred, is_idc, Some(upstream.base_url.as_str()))
}

fn resolve_model(meta: &RequestMeta) -> (String, bool, bool) {
    let model_source = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref())
        .unwrap_or("claude-sonnet-4.5");
    let (is_agentic, is_chat_only) =
        determine_agentic_mode(meta.original_model.as_deref().unwrap_or(model_source));
    (map_model_to_kiro(model_source), is_agentic, is_chat_only)
}

fn resolve_source_format(transform: FormatTransform) -> KiroSourceFormat {
    match transform {
        FormatTransform::KiroToAnthropic => KiroSourceFormat::Anthropic,
        _ => KiroSourceFormat::Responses,
    }
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
            let retry_after = response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let reason_detail = match retry_after {
                Some(value) => format!("{} retry-after={value}", response.status().as_u16()),
                None => response.status().as_u16().to_string(),
            };
            if let Some(cooldown_until_ms) = state.account_selector.mark_response_status(
                "kiro",
                account_id,
                response.status(),
                response.headers(),
            ) {
                let entry = crate::proxy::logs::build_account_state_log_entry(
                    "kiro",
                    account_id,
                    "cooldown_started",
                    "http_status",
                    "cooling_down",
                    Some(reason_detail),
                    Some(cooldown_until_ms),
                );
                state.log.clone().write_account_state_detached(entry);
            }
        }
        AttemptOutcome::Retryable { .. } => {
            let reason_detail = match outcome {
                AttemptOutcome::Retryable { message, .. } => Some(message.clone()),
                _ => None,
            };
            if let Some(cooldown_until_ms) = state
                .account_selector
                .mark_retryable_failure("kiro", account_id)
            {
                let entry = crate::proxy::logs::build_account_state_log_entry(
                    "kiro",
                    account_id,
                    "cooldown_started",
                    "retryable_error",
                    "cooling_down",
                    reason_detail,
                    Some(cooldown_until_ms),
                );
                state.log.clone().write_account_state_detached(entry);
            }
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

async fn finalize_response(
    state: &ProxyState,
    meta: &RequestMeta,
    upstream: &UpstreamRuntime,
    account_id: Option<String>,
    inbound_path: &str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    response: reqwest::Response,
    force_success: bool,
    start_time: Instant,
) -> AttemptOutcome {
    if force_success {
        let proxy_base_url = crate::proxy::http::local_proxy_base_url(&state.config);
        let output = crate::proxy::response::build_proxy_response(
            meta,
            "kiro",
            &upstream.id,
            account_id.clone(),
            inbound_path,
            response,
            state.log.clone(),
            state.token_rate.clone(),
            start_time,
            &proxy_base_url,
            None,
            response_transform,
            request_detail,
            state.config.upstream_no_data_timeout,
        )
        .await;
        return AttemptOutcome::Success(output);
    }
    result::handle_upstream_result(
        state,
        Ok(response),
        meta,
        "kiro",
        &upstream.id,
        account_id,
        inbound_path,
        state.log.clone(),
        state.token_rate.clone(),
        start_time,
        None,
        response_transform,
        request_detail,
    )
    .await
}
