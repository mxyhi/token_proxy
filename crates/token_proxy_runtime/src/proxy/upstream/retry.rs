use axum::http::{HeaderMap, Method, StatusCode};

use super::super::http::RequestAuth;
use super::super::{
    config::UpstreamRuntime, openai_compat::FormatTransform, request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot, ProxyState, RequestMeta,
};
use super::{attempt, result, AttemptOutcome, RetryDirective, RetryScope};
use crate::proxy::cooldown_scope::CooldownScope;
use crate::proxy::http;
use crate::proxy::log::RequestTimings;
use crate::proxy::token_rate::RequestTokenTracker;
use token_proxy_protocol::xai_client_tools::XaiClientToolMapping;

pub(super) struct UpstreamAttempt {
    pub(super) response: reqwest::Response,
    pub(super) selected_account_id: Option<String>,
    pub(super) meta: RequestMeta,
    pub(super) start_time: std::time::Instant,
    pub(super) timings: RequestTimings,
    /// 发送上游前已 register，覆盖 TTFB；成功后移入 response stream。
    pub(super) token_tracker: RequestTokenTracker,
    pub(super) xai_client_tools: Option<XaiClientToolMapping>,
    /// Agent Identity 401 已由账户层分类或恢复，禁止 dispatch 再原地重放整条链路。
    pub(super) skip_same_upstream_retry: bool,
}

pub(super) struct UpstreamAttemptFailure {
    pub(super) outcome: AttemptOutcome,
    pub(super) selected_account_id: Option<String>,
}

/// Same-upstream 账户鉴权恢复结果；不再做跨账户 failover。
pub(super) enum AccountAuthRecoveryResult {
    Pending(UpstreamAttempt),
    Resolved(AttemptOutcome),
}

pub(super) async fn retry_after_kiro_refresh(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    first: &UpstreamAttempt,
    cooldown_scope: &CooldownScope,
) -> Option<AttemptOutcome> {
    if !should_refresh_kiro(provider, &first.response) {
        return None;
    }
    if let Err(outcome) = refresh_kiro_account(state, upstream).await {
        return Some(outcome);
    }
    let retry = match attempt::attempt_send(
        state,
        method,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        request_detail.as_ref(),
        cooldown_scope,
    )
    .await
    {
        Ok(attempt) => attempt,
        Err(failure) => return Some(failure.outcome),
    };
    Some(
        finalize_attempt(
            state,
            provider,
            upstream,
            inbound_path,
            client_gemini_api_key,
            response_transform,
            request_detail,
            retry,
            cooldown_scope,
        )
        .await,
    )
}

pub(super) async fn finalize_attempt(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    attempt: UpstreamAttempt,
    cooldown_scope: &CooldownScope,
) -> AttemptOutcome {
    let skip_same_upstream_retry = attempt.skip_same_upstream_retry;
    schedule_account_response_tasks(
        state,
        provider,
        attempt.selected_account_id.as_deref(),
        &attempt.response,
    );
    let mut outcome = result::handle_upstream_result(
        state,
        Ok(attempt.response),
        &attempt.meta,
        provider,
        &upstream.id,
        attempt.selected_account_id.clone(),
        inbound_path,
        state.log.clone(),
        attempt.token_tracker,
        attempt.start_time,
        attempt.timings,
        client_gemini_api_key,
        response_transform,
        attempt.xai_client_tools,
        request_detail,
        cooldown_scope,
    )
    .await;
    if skip_same_upstream_retry {
        if let AttemptOutcome::Retryable {
            response: Some(response),
            ..
        } = &mut outcome
        {
            response.extensions_mut().insert(RetryDirective {
                scope: RetryScope::NextOnly,
                effective_body: None,
            });
        }
    }
    outcome
}

pub(super) fn mark_account_retryable_failure(
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

/// Codex/xAI 在固定 account_id 上做 401 refresh / Agent Identity 恢复；
/// 失败不切换其它账户，跨 upstream failover 由 upstream selector 负责。
pub(super) async fn retry_with_account_auth_recovery(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    first: Result<UpstreamAttempt, UpstreamAttemptFailure>,
    cooldown_scope: &CooldownScope,
) -> AccountAuthRecoveryResult {
    if !matches!(provider, "codex" | "xai") {
        return match first {
            Ok(attempt) => AccountAuthRecoveryResult::Pending(attempt),
            Err(failure) => AccountAuthRecoveryResult::Resolved(failure.outcome),
        };
    }

    // Same-Upstream Retry 固定当前 credential account_id。
    let mut fixed_upstream = upstream.clone();
    if let Some(account_id) = attempt_selected_account_id(&first) {
        pin_account(provider, &mut fixed_upstream, account_id);
    } else if let Some(account_id) = provider_account_id(provider, upstream)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        pin_account(provider, &mut fixed_upstream, account_id.to_string());
    }

    let current_attempt = retry_after_account_refresh(
        state,
        method,
        provider,
        &fixed_upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        first,
        request_detail.as_ref(),
        cooldown_scope,
    )
    .await;
    let outcome = finalize_account_auth_recovery_attempt(
        state,
        provider,
        &fixed_upstream,
        inbound_path,
        client_gemini_api_key,
        response_transform,
        request_detail,
        current_attempt,
        cooldown_scope,
    )
    .await;
    AccountAuthRecoveryResult::Resolved(outcome)
}

async fn retry_after_account_refresh(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    attempt: Result<UpstreamAttempt, UpstreamAttemptFailure>,
    request_detail: Option<&RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
) -> Result<UpstreamAttempt, UpstreamAttemptFailure> {
    let Ok(first) = attempt else {
        return attempt;
    };
    if !should_refresh_account(provider, &first.response) {
        return Ok(first);
    }
    let Some(account_id) = first.selected_account_id.clone() else {
        return Ok(first);
    };
    if provider == "codex" {
        match state.codex_accounts.get_account_record(&account_id).await {
            Ok(record) if record.agent_identity().is_some() => {
                return retry_after_agent_identity_task_recovery(
                    state,
                    method,
                    provider,
                    upstream,
                    inbound_path,
                    upstream_path_with_query,
                    headers,
                    body,
                    meta,
                    request_auth,
                    first,
                    record,
                    request_detail,
                    cooldown_scope,
                )
                .await;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(account_id, error = %error, "codex account reload after 401 failed");
                return Ok(first);
            }
        }
    }
    if let Err(err) = refresh_account(state, provider, &account_id).await {
        tracing::warn!(
            provider,
            account_id,
            error = %err,
            "account refresh after unauthorized failed"
        );
        return Ok(first);
    }
    tracing::info!(
        provider,
        account_id,
        "account refreshed after unauthorized response"
    );
    let mut retry_upstream = upstream.clone();
    pin_account(provider, &mut retry_upstream, account_id);
    attempt::attempt_send(
        state,
        method,
        provider,
        &retry_upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        request_detail,
        cooldown_scope,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn retry_after_agent_identity_task_recovery(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    first: UpstreamAttempt,
    record: token_proxy_account_codex::CodexTokenRecord,
    request_detail: Option<&RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
) -> Result<UpstreamAttempt, UpstreamAttemptFailure> {
    let account_id = first
        .selected_account_id
        .clone()
        .expect("selected Codex account checked before Agent Identity recovery");
    let expected_task_id = record
        .agent_identity()
        .and_then(|identity| identity.task_id)
        .unwrap_or_default()
        .to_string();
    let UpstreamAttempt {
        response,
        selected_account_id,
        meta: attempt_meta,
        start_time,
        timings,
        token_tracker,
        xai_client_tools,
        skip_same_upstream_retry: _,
    } = first;
    let status = response.status();
    let version = response.version();
    let headers_snapshot = response.headers().clone();
    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(account_id, error = %error, "agent identity 401 response body read failed");
            return Err(UpstreamAttemptFailure {
                outcome: AttemptOutcome::Fatal(http::error_response(
                    StatusCode::BAD_GATEWAY,
                    "Failed to read Agent Identity upstream response.",
                )),
                selected_account_id,
            });
        }
    };
    if !token_proxy_account_codex::CodexAccountStore::is_agent_identity_task_invalid(
        status.as_u16(),
        &body_bytes,
    ) {
        let mut rebuilt = axum::http::Response::builder()
            .status(status)
            .version(version)
            .body(reqwest::Body::from(body_bytes))
            .expect("valid upstream response status and version");
        *rebuilt.headers_mut() = headers_snapshot;
        return Ok(UpstreamAttempt {
            response: reqwest::Response::from(rebuilt),
            selected_account_id,
            meta: attempt_meta,
            start_time,
            timings,
            token_tracker,
            xai_client_tools,
            skip_same_upstream_retry: true,
        });
    }

    tracing::warn!(
        account_id,
        "codex upstream rejected stale agent identity task"
    );
    if let Err(error) = state
        .codex_accounts
        .recover_agent_identity_task(&account_id, &expected_task_id)
        .await
    {
        tracing::warn!(account_id, error = %error, "agent identity task recovery failed");
        return Err(UpstreamAttemptFailure {
            outcome: AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, error)),
            selected_account_id,
        });
    }
    let mut retry_upstream = upstream.clone();
    pin_account(provider, &mut retry_upstream, account_id.clone());
    tracing::info!(
        account_id,
        "agent identity task recovered after unauthorized response"
    );
    let mut retry = attempt::attempt_send(
        state,
        method,
        provider,
        &retry_upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        request_detail,
        cooldown_scope,
    )
    .await;
    if let Ok(attempt) = &mut retry {
        attempt.skip_same_upstream_retry = true;
    }
    retry
}

async fn finalize_account_auth_recovery_attempt(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    attempt: Result<UpstreamAttempt, UpstreamAttemptFailure>,
    cooldown_scope: &CooldownScope,
) -> AttemptOutcome {
    match attempt {
        Ok(attempt) => {
            finalize_attempt(
                state,
                provider,
                upstream,
                inbound_path,
                client_gemini_api_key,
                response_transform,
                request_detail,
                attempt,
                cooldown_scope,
            )
            .await
        }
        Err(failure) => failure.outcome,
    }
}

fn attempt_selected_account_id(
    attempt: &Result<UpstreamAttempt, UpstreamAttemptFailure>,
) -> Option<String> {
    match attempt {
        Ok(attempt) => attempt.selected_account_id.clone(),
        Err(failure) => failure.selected_account_id.clone(),
    }
}

fn schedule_account_response_tasks(
    state: &ProxyState,
    provider: &str,
    account_id: Option<&str>,
    response: &reqwest::Response,
) {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let account_id = account_id.to_string();
    match provider {
        "kiro" if response.status().is_success() => {
            let store = state.kiro_accounts.clone();
            tokio::spawn(async move {
                let _ = store.refresh_quota_if_stale(&account_id).await;
            });
        }
        "codex" if response.status().is_success() => {
            let store = state.codex_accounts.clone();
            tokio::spawn(async move {
                let _ = store.refresh_quota_if_stale(&account_id).await;
            });
        }
        "xai" => {
            let store = state.xai_accounts.clone();
            let headers = response.headers().clone();
            let status = response.status().as_u16();
            tokio::spawn(async move {
                if let Err(error) = store
                    .record_quota_headers(&account_id, &headers, status)
                    .await
                {
                    tracing::warn!(
                        account_id,
                        status,
                        error = %error,
                        "xai response quota snapshot persistence failed"
                    );
                }
            });
        }
        _ => {}
    }
}

fn should_refresh_kiro(provider: &str, response: &reqwest::Response) -> bool {
    provider == "kiro"
        && (response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::FORBIDDEN)
}

fn should_refresh_account(provider: &str, response: &reqwest::Response) -> bool {
    matches!(provider, "codex" | "xai") && response.status() == StatusCode::UNAUTHORIZED
}

async fn refresh_account(
    state: &ProxyState,
    provider: &str,
    account_id: &str,
) -> Result<(), String> {
    match provider {
        "codex" => state.codex_accounts.refresh_account(account_id).await,
        "xai" => state.xai_accounts.refresh_account(account_id).await,
        _ => Err(format!(
            "Provider {provider} does not support account refresh."
        )),
    }
}

fn provider_account_id<'a>(provider: &str, upstream: &'a UpstreamRuntime) -> Option<&'a str> {
    match provider {
        "codex" => upstream.codex_account_id.as_deref(),
        "xai" => upstream.xai_account_id.as_deref(),
        _ => None,
    }
}

/// Same-Upstream Retry / request repair 必须固定当前 account_id。
pub(super) fn pin_account_if_missing(
    provider: &str,
    upstream: &mut UpstreamRuntime,
    account_id: Option<&str>,
) {
    if provider_account_id(provider, upstream)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return;
    }
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    pin_account(provider, upstream, account_id.to_string());
}

fn pin_account(provider: &str, upstream: &mut UpstreamRuntime, account_id: String) {
    match provider {
        "codex" => {
            tracing::debug!(account_id, "pinning codex account for same-upstream retry");
            upstream.codex_account_id = Some(account_id);
        }
        "xai" => {
            tracing::debug!(account_id, "pinning xai account for same-upstream retry");
            upstream.xai_account_id = Some(account_id);
        }
        "kiro" => {
            tracing::debug!(account_id, "pinning kiro account for same-upstream retry");
            upstream.kiro_account_id = Some(account_id);
        }
        _ => {}
    }
}

async fn refresh_kiro_account(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
) -> Result<(), AttemptOutcome> {
    let Some(account_id) = upstream.kiro_account_id.as_deref() else {
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Kiro account is not configured.",
        )));
    };
    state
        .kiro_accounts
        .refresh_account(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))
}

#[cfg(test)]
#[path = "retry.test.rs"]
mod tests;
