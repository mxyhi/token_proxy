use axum::http::{HeaderMap, Method, StatusCode};

use super::super::http::RequestAuth;
use super::super::{
    config::UpstreamRuntime, openai_compat::FormatTransform, request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot, ProxyState, RequestMeta,
};
use super::{attempt, result, AttemptOutcome};
use crate::proxy::http;

pub(super) struct UpstreamAttempt {
    pub(super) response: reqwest::Response,
    pub(super) selected_account_id: Option<String>,
    pub(super) meta: RequestMeta,
    pub(super) start_time: std::time::Instant,
}

pub(super) struct UpstreamAttemptFailure {
    pub(super) outcome: AttemptOutcome,
    pub(super) selected_account_id: Option<String>,
}

pub(super) enum CodexFailoverResult {
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
) -> AttemptOutcome {
    schedule_account_quota_refresh(
        state,
        provider,
        attempt.selected_account_id.as_deref(),
        attempt.response.status(),
    );
    result::handle_upstream_result(
        state,
        Ok(attempt.response),
        &attempt.meta,
        provider,
        &upstream.id,
        attempt.selected_account_id.clone(),
        inbound_path,
        state.log.clone(),
        state.token_rate.clone(),
        attempt.start_time,
        client_gemini_api_key,
        response_transform,
        request_detail,
    )
    .await
}

pub(super) fn mark_account_retryable_failure(
    state: &ProxyState,
    provider: &str,
    account_id: Option<&str>,
    _reason_detail: Option<String>,
) {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let _ = state
        .account_selector
        .mark_retryable_failure(provider, account_id);
}

pub(super) async fn retry_with_next_codex_account(
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
) -> CodexFailoverResult {
    let Some(first_selected_account_id) = failover_selected_account_id(provider, upstream, &first)
    else {
        return match first {
            Ok(attempt) => CodexFailoverResult::Pending(attempt),
            Err(failure) => CodexFailoverResult::Resolved(failure.outcome),
        };
    };

    let mut excluded_account_ids = vec![first_selected_account_id];
    let mut last_outcome = Some(
        finalize_codex_failover_attempt(
            state,
            provider,
            upstream,
            inbound_path,
            client_gemini_api_key,
            response_transform,
            request_detail.clone(),
            first,
        )
        .await,
    );

    loop {
        let ordered_account_ids = state
            .codex_accounts
            .list_accounts()
            .await
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| item.account_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let ordered_account_ids = state
            .account_selector
            .order_accounts(provider, &ordered_account_ids);
        let next_account_id = match state
            .codex_accounts
            .resolve_next_account_record_with_order(
                &excluded_account_ids,
                Some(ordered_account_ids.as_slice()),
            )
            .await
        {
            Ok(Some((account_id, _))) => account_id,
            Ok(None) => match last_outcome {
                Some(outcome) => return CodexFailoverResult::Resolved(outcome),
                None => {
                    return CodexFailoverResult::Resolved(AttemptOutcome::Fatal(
                        http::error_response(
                            StatusCode::BAD_GATEWAY,
                            "No available Codex account remained after failover.",
                        ),
                    ));
                }
            },
            Err(err) => {
                return CodexFailoverResult::Resolved(AttemptOutcome::Fatal(http::error_response(
                    StatusCode::UNAUTHORIZED,
                    err,
                )));
            }
        };

        let mut retry_upstream = upstream.clone();
        retry_upstream.codex_account_id = Some(next_account_id.clone());
        let retry = attempt::attempt_send(
            state,
            method.clone(),
            provider,
            &retry_upstream,
            inbound_path,
            upstream_path_with_query,
            headers,
            body,
            meta,
            request_auth,
            request_detail.as_ref(),
        )
        .await;
        excluded_account_ids.push(next_account_id);

        let should_retry_again = failover_selected_account_id(provider, upstream, &retry).is_some();
        if !should_retry_again {
            return match retry {
                Ok(attempt) => CodexFailoverResult::Resolved(
                    finalize_attempt(
                        state,
                        provider,
                        &retry_upstream,
                        inbound_path,
                        client_gemini_api_key,
                        response_transform,
                        request_detail,
                        attempt,
                    )
                    .await,
                ),
                Err(failure) => CodexFailoverResult::Resolved(failure.outcome),
            };
        }
        last_outcome = Some(
            finalize_codex_failover_attempt(
                state,
                provider,
                &retry_upstream,
                inbound_path,
                client_gemini_api_key,
                response_transform,
                request_detail.clone(),
                retry,
            )
            .await,
        );
    }
}

async fn finalize_codex_failover_attempt(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    attempt: Result<UpstreamAttempt, UpstreamAttemptFailure>,
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
            )
            .await
        }
        Err(failure) => failure.outcome,
    }
}

fn failover_selected_account_id(
    provider: &str,
    upstream: &UpstreamRuntime,
    attempt: &Result<UpstreamAttempt, UpstreamAttemptFailure>,
) -> Option<String> {
    if provider != "codex"
        || upstream
            .codex_account_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    {
        return None;
    }

    match attempt {
        Ok(attempt) if should_failover_codex_account(provider, &attempt.response) => {
            attempt.selected_account_id.clone()
        }
        Err(failure) => failure.selected_account_id.clone(),
        _ => None,
    }
}

fn should_failover_codex_account(provider: &str, response: &reqwest::Response) -> bool {
    provider == "codex" && !response.status().is_success()
}

fn schedule_account_quota_refresh(
    state: &ProxyState,
    provider: &str,
    account_id: Option<&str>,
    status: StatusCode,
) {
    if !status.is_success() {
        return;
    }
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let account_id = account_id.to_string();
    match provider {
        "kiro" => {
            let store = state.kiro_accounts.clone();
            tokio::spawn(async move {
                let _ = store.refresh_quota_if_stale(&account_id).await;
            });
        }
        "codex" => {
            let store = state.codex_accounts.clone();
            tokio::spawn(async move {
                let _ = store.refresh_quota_if_stale(&account_id).await;
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
