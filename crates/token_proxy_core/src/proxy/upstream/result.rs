use std::sync::Arc;
use std::time::Instant;

use axum::http::StatusCode;

use super::utils::{is_retryable_error, is_retryable_status, sanitize_upstream_error};
use super::AttemptOutcome;
use crate::proxy::http;
use crate::proxy::log::{build_log_entry, LogContext, LogWriter, UsageSnapshot};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::response::{build_proxy_response, build_proxy_response_buffered};
use crate::proxy::token_rate::TokenRateTracker;
use crate::proxy::ProxyState;
use crate::proxy::RequestMeta;

pub(super) fn should_cooldown_retryable_status(status: StatusCode) -> bool {
    // cooldown 只用于“更像上游账号/节点短时异常”的错误，避免把请求内容问题扩散到后续请求。
    // 因此 400/404/422/307 虽然可在当前请求内换路重试，但不会跨请求冷却整个 upstream。
    matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

pub(super) async fn handle_upstream_result(
    state: &ProxyState,
    upstream_res: Result<reqwest::Response, reqwest::Error>,
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    inbound_path: &str,
    log: Arc<LogWriter>,
    token_rate: Arc<TokenRateTracker>,
    start_time: Instant,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> AttemptOutcome {
    match upstream_res {
        Ok(res) if is_retryable_status(res.status()) => {
            let status = res.status();
            let response = build_proxy_response_buffered(
                meta,
                provider,
                upstream_id,
                inbound_path,
                res,
                log,
                token_rate,
                start_time,
                response_transform,
                request_detail.clone(),
                state.config.upstream_no_data_timeout,
            )
            .await;
            AttemptOutcome::Retryable {
                message: format!("Upstream responded with {}", response.status()),
                response: Some(response),
                is_timeout: false,
                should_cooldown: should_cooldown_retryable_status(status),
            }
        }
        Ok(res) => {
            let response = build_proxy_response(
                meta,
                provider,
                upstream_id,
                inbound_path,
                res,
                log,
                token_rate,
                start_time,
                response_transform,
                request_detail.clone(),
                state.config.upstream_no_data_timeout,
            )
            .await;
            AttemptOutcome::Success(response)
        }
        Err(err) if is_retryable_error(&err) => {
            let message = sanitize_upstream_error(provider, &err);
            let status = if err.is_timeout() {
                StatusCode::GATEWAY_TIMEOUT
            } else {
                StatusCode::BAD_GATEWAY
            };
            log_upstream_error_if_needed(
                &log,
                request_detail.as_ref(),
                meta,
                provider,
                upstream_id,
                inbound_path,
                status,
                message.clone(),
                start_time,
            );
            AttemptOutcome::Retryable {
                message,
                response: None,
                is_timeout: err.is_timeout(),
                should_cooldown: true,
            }
        }
        Err(err) => {
            let message = sanitize_upstream_error(provider, &err);
            log_upstream_error_if_needed(
                &log,
                request_detail.as_ref(),
                meta,
                provider,
                upstream_id,
                inbound_path,
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {message}"),
                start_time,
            );
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {message}"),
            ))
        }
    }
}

pub(super) fn log_upstream_error_if_needed(
    log: &Arc<LogWriter>,
    request_detail: Option<&RequestDetailSnapshot>,
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    inbound_path: &str,
    status: StatusCode,
    response_error: String,
    start_time: Instant,
) {
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers.clone(), detail.request_body.clone()))
        .unwrap_or((None, None));
    let context = LogContext {
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: None,
        request_headers,
        request_body,
        ttfb_ms: None,
        start: start_time,
    };
    let usage = UsageSnapshot {
        usage: None,
        cached_tokens: None,
        usage_json: None,
    };
    let entry = build_log_entry(&context, usage, Some(response_error));
    log.clone().write_detached(entry);
}
