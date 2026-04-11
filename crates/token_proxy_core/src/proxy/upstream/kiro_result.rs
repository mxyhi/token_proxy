use super::kiro::{MAX_KIRO_BACKOFF_SECS, MAX_KIRO_RETRIES};
use super::kiro_prepare::KiroContext;
use super::{result, AttemptOutcome};
use crate::proxy::http;
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::{config::UpstreamRuntime, ProxyState, RequestMeta};
use axum::body::{Body, Bytes};
use axum::http::StatusCode;
use std::time::{Duration, Instant};

pub(super) enum ResponseAction {
    RetryAfter(Duration),
    RefreshAndRetry,
    NextEndpoint,
    Finalize(reqwest::Response, Instant),
    Return(AttemptOutcome),
}

pub(super) async fn handle_response_action(
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

pub(super) async fn finalize_response(
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
