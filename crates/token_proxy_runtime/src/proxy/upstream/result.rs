use std::sync::Arc;
use std::time::Instant;

use axum::{http::StatusCode, response::Response};

use super::dispatch::ForwardAttemptState;
use super::utils::{is_retryable_error, is_retryable_status, sanitize_upstream_error};
use super::{AttemptOutcome, RetryDirective, RetryScope};
use crate::proxy::account_selector::AccountSelectorRuntime;
use crate::proxy::cooldown_scope::CooldownScope;
use crate::proxy::http;
use crate::proxy::log::{build_log_entry, LogContext, LogWriter, RequestTimings, UsageSnapshot};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::response::{
    build_proxy_response, build_proxy_response_buffered, AccountCooldownHint,
    NonRetryableSemanticResponse, RetryableStreamResponse,
};
use crate::proxy::token_rate::RequestTokenTracker;
use crate::proxy::ProxyState;
use crate::proxy::RequestMeta;
use token_proxy_config::ProviderUpstreams;
use token_proxy_protocol::xai_client_tools::XaiClientToolMapping;

const LOCAL_UPSTREAM_ID: &str = "local";

pub(crate) struct ForwardUpstreamResult {
    pub(crate) response: Response,
    pub(crate) should_fallback: bool,
}

pub(super) fn should_cooldown_retryable_status(status: StatusCode) -> bool {
    // cooldown 只用于“更像上游账号/节点短时异常”的错误，避免把请求内容问题扩散到后续请求。
    // 因此 400/404/422/307 虽然可在当前请求内换路重试，但不会跨请求冷却整个 upstream。
    // 402 表示账户计费/订阅不可用，属于账户访问失败，需要冷却后再选择其它账户。
    matches!(
        status,
        StatusCode::PAYMENT_REQUIRED
            | StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

/// 是否把可重试 HTTP 状态标为 `RetryScope::NextOnly`（跳过 same-upstream 再放）。
///
/// 规则：
/// - `413 Payload Too Large`：全局 NextOnly（body 过大，原地重放无意义）。
/// - `400/401/402/403/404/422`：仅固定账户 credential（`account_id` 非空，且 provider 为
///   codex/xai；Kiro 走 `kiro_result` 自管 NextOnly）时 NextOnly。
/// - 普通 API-key upstream：保持缺省 SameThenNext，保留 `same_upstream_retry_count`。
/// - transport / 超时 / 5xx / 429 / prelude：仍走缺省 SameThenNext。
pub(super) fn is_next_only_retryable_status(
    status: StatusCode,
    provider: &str,
    account_id: Option<&str>,
) -> bool {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return true;
    }
    let account_semantic = matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::UNPROCESSABLE_ENTITY
    );
    if !account_semantic {
        return false;
    }
    let has_account = account_id
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    // Kiro 失败路径在 kiro_result 内单独插入 NextOnly，不经过本函数。
    has_account && matches!(provider, "codex" | "xai")
}

/// Codex/Kiro/xAI 固定账户：仅当跨多个 distinct runtime Upstream 仍鉴权失败时，
/// 才将最终 401/403 转为 503。单 Upstream（含 Agent Identity 内部 recovery/replay）保留原状态。
fn is_fixed_account_auth_status_to_mask(
    provider: &str,
    status: StatusCode,
    distinct_attempted_upstreams: usize,
) -> bool {
    distinct_attempted_upstreams > 1
        && matches!(provider, "codex" | "kiro" | "xai")
        && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

pub(super) async fn handle_upstream_result(
    state: &ProxyState,
    upstream_res: Result<reqwest::Response, reqwest::Error>,
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    account_id: Option<String>,
    inbound_path: &str,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    start_time: Instant,
    timings: RequestTimings,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    xai_client_tools: Option<XaiClientToolMapping>,
    request_detail: Option<RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
) -> AttemptOutcome {
    let account_id_value = account_id.as_deref().map(str::to_string);
    let proxy_base_url = http::local_proxy_base_url(&state.config);
    match upstream_res {
        Ok(res) if is_retryable_status(res.status()) => {
            let status = res.status();
            let response_headers = res.headers().clone();
            let response = build_proxy_response_buffered(
                meta,
                provider,
                upstream_id,
                account_id_value.clone(),
                inbound_path,
                res,
                log,
                request_tracker,
                start_time,
                timings.clone(),
                &proxy_base_url,
                client_gemini_api_key,
                response_transform,
                xai_client_tools.clone(),
                request_detail.clone(),
                state.config.sync_response_timeout,
            )
            .await;
            if response
                .extensions()
                .get::<NonRetryableSemanticResponse>()
                .is_some()
            {
                return AttemptOutcome::Success(response);
            }
            update_account_cooldown_from_response(
                &state.account_selector,
                provider,
                account_id_value.as_deref(),
                status,
                &response_headers,
                &response,
                cooldown_scope,
            );
            let mut response = response;
            // 固定账户 401/403/400 等：同 Upstream 再打无意义，直接跨 Upstream。
            // API-key upstream 不进此分支，保留 same_upstream_retry_count。
            if is_next_only_retryable_status(status, provider, account_id_value.as_deref()) {
                response.extensions_mut().insert(RetryDirective {
                    scope: RetryScope::NextOnly,
                    effective_body: None,
                });
                tracing::debug!(
                    provider,
                    upstream = upstream_id,
                    account_id = account_id_value.as_deref().unwrap_or(""),
                    status = status.as_u16(),
                    "retryable status marked NextOnly for cross-upstream failover"
                );
            }
            let retryable_response = response
                .extensions()
                .get::<RetryableStreamResponse>()
                .cloned();
            let should_cooldown = retryable_response.as_ref().map_or_else(
                || should_cooldown_retryable_status(status),
                |retryable| retryable.should_cooldown,
            );
            AttemptOutcome::Retryable {
                message: format!("Upstream responded with {}", response.status()),
                response: Some(response),
                is_timeout: false,
                should_cooldown,
                deferred_log: None,
            }
        }
        Ok(res) => {
            let status = res.status();
            let response_headers = res.headers().clone();
            let response = build_proxy_response(
                meta,
                provider,
                upstream_id,
                account_id_value.clone(),
                inbound_path,
                res,
                log,
                request_tracker,
                start_time,
                timings,
                &proxy_base_url,
                client_gemini_api_key,
                response_transform,
                xai_client_tools,
                request_detail.clone(),
                state.config.stream_first_output_timeout,
                state.config.sync_response_timeout,
            )
            .await;
            if let Some(retryable) = response
                .extensions()
                .get::<RetryableStreamResponse>()
                .cloned()
            {
                if retryable.should_cooldown
                    || response.extensions().get::<AccountCooldownHint>().is_some()
                {
                    update_account_cooldown_from_response(
                        &state.account_selector,
                        provider,
                        account_id_value.as_deref(),
                        retryable.status,
                        &response_headers,
                        &response,
                        cooldown_scope,
                    );
                }
                return AttemptOutcome::Retryable {
                    message: retryable.message,
                    response: Some(response),
                    is_timeout: false,
                    should_cooldown: retryable.should_cooldown,
                    deferred_log: None,
                };
            }
            update_account_cooldown_from_status(
                state,
                provider,
                account_id_value.as_deref(),
                status,
                &response_headers,
                cooldown_scope,
            );
            AttemptOutcome::Success(response)
        }
        Err(err) if is_retryable_error(&err) => {
            // 无 response body 可统计，释放发送前 register 的窗口。
            drop(request_tracker);
            let message = sanitize_upstream_error(provider, &err);
            mark_retryable_account_failure(
                state,
                provider,
                account_id_value.as_deref(),
                Some(message.clone()),
                cooldown_scope,
            );
            // 延后到本请求终态失败再写 SQLite，避免中间 attempt 刷 502。
            AttemptOutcome::Retryable {
                message: message.clone(),
                response: None,
                is_timeout: err.is_timeout(),
                should_cooldown: true,
                deferred_log: Some(message),
            }
        }
        Err(err) => {
            drop(request_tracker);
            let message = sanitize_upstream_error(provider, &err);
            log_upstream_error_if_needed(
                &log,
                request_detail.as_ref(),
                meta,
                provider,
                upstream_id,
                account_id.as_deref(),
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

pub(super) fn resolve_provider_upstreams<'a>(
    state: &'a ProxyState,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
) -> Result<&'a ProviderUpstreams, Response> {
    match state.config.provider_upstreams(provider) {
        Some(upstreams) => Ok(upstreams),
        None => {
            log_upstream_error_if_needed(
                &state.log,
                request_detail,
                meta,
                provider,
                LOCAL_UPSTREAM_ID,
                None,
                inbound_path,
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.".to_string(),
                Instant::now(),
            );
            Err(http::error_response(
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.",
            ))
        }
    }
}

pub(super) fn finalize_forward_result(
    state: &ProxyState,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
    summary: ForwardAttemptState,
) -> ForwardUpstreamResult {
    if let Some(response) = summary.response {
        return ForwardUpstreamResult {
            response,
            should_fallback: false,
        };
    }
    let should_fallback = summary.last_retry_response.is_some()
        || summary.last_timeout_error.is_some()
        || summary.last_retry_error.is_some()
        || summary.attempted == 0;
    let response = finalize_forward_response(
        &state.log,
        provider,
        inbound_path,
        meta,
        request_detail,
        summary,
    );
    ForwardUpstreamResult {
        response,
        should_fallback,
    }
}

fn finalize_forward_response(
    log: &Arc<LogWriter>,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
    summary: ForwardAttemptState,
) -> Response {
    if summary.attempted == 0 && summary.missing_auth {
        log_upstream_error_if_needed(
            log,
            request_detail,
            meta,
            provider,
            LOCAL_UPSTREAM_ID,
            None,
            inbound_path,
            StatusCode::BAD_GATEWAY,
            "Missing upstream API key.".to_string(),
            Instant::now(),
        );
        tracing::warn!(
            provider,
            status = StatusCode::BAD_GATEWAY.as_u16(),
            exclusion_reason = "missing_upstream_credential",
            "request rejected because upstream credential is not configured"
        );
        return http::error_response(StatusCode::BAD_GATEWAY, "Missing upstream API key.");
    }
    if summary.attempted == 0 && summary.model_unsupported {
        let message = format!(
            "Model '{}' is not supported by any configured upstream.",
            meta.original_model.as_deref().unwrap_or("unknown")
        );
        log_upstream_error_if_needed(
            log,
            request_detail,
            meta,
            provider,
            LOCAL_UPSTREAM_ID,
            None,
            inbound_path,
            StatusCode::NOT_FOUND,
            message.clone(),
            Instant::now(),
        );
        tracing::warn!(
            provider,
            model = meta.original_model.as_deref().unwrap_or(""),
            status = StatusCode::NOT_FOUND.as_u16(),
            exclusion_reason = "model_not_supported",
            "request rejected because no upstream supports the model"
        );
        return http::error_response(StatusCode::NOT_FOUND, message);
    }
    if let Some(response) = summary.last_retry_response {
        // 固定账户：仅 distinct runtime Upstream > 1 时 mask 401/403→503。
        // should_fallback 已在 finalize_forward_result 按 last_retry_response 存在算 true，
        // 本转换不改 fallback 能力，跨 provider 仍可继续。
        // 勿用 summary.attempted：same-upstream retry / Agent Identity 内部恢复会放大它。
        let distinct = summary.attempted_upstream_keys.len();
        if is_fixed_account_auth_status_to_mask(provider, response.status(), distinct) {
            let message =
                format!("All {provider} fixed-account upstreams exhausted after auth failure");
            tracing::warn!(
                provider,
                original_status = response.status().as_u16(),
                status = StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                distinct_attempted_upstreams = distinct,
                exclusion_reason = "fixed_account_auth_exhausted",
                "masking fixed-account auth failure as 503 for client"
            );
            log_upstream_error_if_needed(
                log,
                request_detail,
                meta,
                provider,
                LOCAL_UPSTREAM_ID,
                None,
                inbound_path,
                StatusCode::SERVICE_UNAVAILABLE,
                message.clone(),
                Instant::now(),
            );
            return http::error_response(StatusCode::SERVICE_UNAVAILABLE, message);
        }
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) && matches!(provider, "codex" | "kiro" | "xai")
        {
            tracing::debug!(
                provider,
                status = response.status().as_u16(),
                distinct_attempted_upstreams = distinct,
                "keeping single-upstream fixed-account auth failure status for client"
            );
        }
        return response;
    }
    // 仅终态失败落一条 deferred transport 诊断（中间 attempt 已跳过写库）。
    if let Some(deferred) = summary.last_deferred_log.as_ref() {
        let status = StatusCode::from_u16(deferred.status).unwrap_or(StatusCode::BAD_GATEWAY);
        let upstream_id = if deferred.upstream_id.is_empty() {
            LOCAL_UPSTREAM_ID
        } else {
            deferred.upstream_id.as_str()
        };
        log_upstream_error_if_needed(
            log,
            request_detail,
            meta,
            provider,
            upstream_id,
            deferred.account_id.as_deref(),
            inbound_path,
            status,
            deferred.message.clone(),
            deferred.start_time,
        );
    }
    if let Some(err) = summary.last_timeout_error {
        return http::error_response(StatusCode::GATEWAY_TIMEOUT, err);
    }
    if let Some(err) = summary.last_retry_error {
        return http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Upstream request failed: {err}"),
        );
    }
    http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured.")
}

fn update_account_cooldown_from_status(
    state: &ProxyState,
    provider: &str,
    account_id: Option<&str>,
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    cooldown_scope: &CooldownScope,
) {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if status.is_success() {
        state
            .account_selector
            .clear_cooldown_scoped(provider, account_id, cooldown_scope);
        return;
    }
    let _ = state.account_selector.mark_response_status_scoped(
        provider,
        account_id,
        status,
        headers,
        cooldown_scope,
    );
}

fn update_account_cooldown_from_response(
    account_selector: &AccountSelectorRuntime,
    provider: &str,
    account_id: Option<&str>,
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    response: &Response,
    cooldown_scope: &CooldownScope,
) {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if let Some(hint) = response.extensions().get::<AccountCooldownHint>() {
        let until = account_selector.mark_explicit_cooldown_scoped(
            provider,
            account_id,
            hint.duration,
            cooldown_scope,
        );
        tracing::warn!(
            provider,
            account_id,
            reason = hint.reason,
            cooldown_seconds = hint.duration.as_secs(),
            cooldown_until_epoch_ms = until,
            "account entered provider-directed cooldown"
        );
        return;
    }
    let _ = account_selector.mark_response_status_scoped(
        provider,
        account_id,
        status,
        headers,
        cooldown_scope,
    );
}

fn mark_retryable_account_failure(
    state: &ProxyState,
    provider: &str,
    account_id: Option<&str>,
    _reason_detail: Option<String>,
    cooldown_scope: &CooldownScope,
) {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let _ =
        state
            .account_selector
            .mark_retryable_failure_scoped(provider, account_id, cooldown_scope);
}

pub(super) fn log_upstream_error_if_needed(
    log: &Arc<LogWriter>,
    request_detail: Option<&RequestDetailSnapshot>,
    meta: &RequestMeta,
    provider: &str,
    upstream_id: &str,
    account_id: Option<&str>,
    inbound_path: &str,
    status: StatusCode,
    response_error: String,
    start_time: Instant,
) {
    let (request_headers, request_body) = request_detail
        .map(|detail| (detail.request_headers.clone(), detail.request_body.clone()))
        .unwrap_or((None, None));
    let context = LogContext {
        client_ip: meta.client_ip.clone(),
        path: inbound_path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        account_id: account_id.map(str::to_string),
        model: meta.original_model.clone(),
        mapped_model: meta.mapped_model.clone(),
        stream: meta.stream,
        status: status.as_u16(),
        upstream_request_id: None,
        request_headers,
        request_body,
        ttfb_ms: None,
        timings: RequestTimings::with_billing(meta.billing.clone()),
        start: start_time,
    };
    let usage = UsageSnapshot::default();
    let entry = build_log_entry(&context, usage, Some(response_error));
    log.clone().write_detached(entry);
}

#[cfg(test)]
#[path = "result.test.rs"]
mod tests;
