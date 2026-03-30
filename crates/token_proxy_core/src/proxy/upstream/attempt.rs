use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::{Client, Proxy};
use tokio::time::timeout;

use super::request;
use super::result;
use super::utils::{is_retryable_error, sanitize_upstream_error};
use super::{AttemptOutcome, PreparedUpstreamRequest};
use crate::proxy::http;
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::server_helpers::log_debug_headers_body;
use crate::proxy::{config::UpstreamRuntime, ProxyState, RequestMeta};

const DEBUG_UPSTREAM_LOG_LIMIT_BYTES: usize = usize::MAX;

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
        response_transform,
        request_detail,
        first,
    )
    .await
}

struct UpstreamAttempt {
    response: reqwest::Response,
    selected_account_id: Option<String>,
    meta: RequestMeta,
    start_time: Instant,
}

struct UpstreamAttemptFailure {
    outcome: AttemptOutcome,
    selected_account_id: Option<String>,
}

enum CodexFailoverResult {
    Pending(UpstreamAttempt),
    Resolved(AttemptOutcome),
}

async fn retry_after_kiro_refresh(
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
    let retry = match attempt_send(
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
            response_transform,
            request_detail,
            retry,
        )
        .await,
    )
}

async fn finalize_attempt(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    attempt: UpstreamAttempt,
) -> AttemptOutcome {
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
        response_transform,
        request_detail,
    )
    .await
}

async fn retry_with_next_codex_account(
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
    let mut last_retry: Option<Result<(UpstreamRuntime, UpstreamAttempt), UpstreamAttemptFailure>> =
        Some(first.map(|attempt| (upstream.clone(), attempt)));

    // 这里做账户级 failover，而不是 upstream 级 failover。
    // 当 codex upstream 未绑定具体账户时，只要当前账户请求报错，就继续尝试下一个本地可用账户。
    loop {
        let next_account_id = match state
            .codex_accounts
            .resolve_next_account_record(&excluded_account_ids)
            .await
        {
            Ok(Some((account_id, _))) => account_id,
            Ok(None) => match last_retry {
                Some(Ok((retry_upstream, retry_attempt))) => {
                    return CodexFailoverResult::Resolved(
                        finalize_attempt(
                            state,
                            provider,
                            &retry_upstream,
                            inbound_path,
                            response_transform,
                            request_detail,
                            retry_attempt,
                        )
                        .await,
                    );
                }
                Some(Err(failure)) => return CodexFailoverResult::Resolved(failure.outcome),
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
                return CodexFailoverResult::Resolved(AttemptOutcome::Fatal(
                    http::error_response(StatusCode::UNAUTHORIZED, err),
                ));
            }
        };

        let mut retry_upstream = upstream.clone();
        retry_upstream.codex_account_id = Some(next_account_id.clone());
        let retry = attempt_send(
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
                        response_transform,
                        request_detail,
                        attempt,
                    )
                    .await,
                ),
                Err(failure) => CodexFailoverResult::Resolved(failure.outcome),
            };
        }
        last_retry = Some(retry.map(|attempt| (retry_upstream, attempt)));
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

async fn attempt_send(
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

async fn send_upstream_request(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    upstream_url: &str,
    proxy_url: Option<&str>,
    request_headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
) -> Result<reqwest::Response, AttemptOutcome> {
    if provider == "codex" {
        return send_codex_request(
            state,
            method,
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            upstream_url,
            proxy_url,
            request_headers,
            body,
            meta,
            selected_account_id,
            request_detail,
            start_time,
        )
        .await;
    }
    send_upstream_request_once(
        state,
        method,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        upstream_url,
        proxy_url,
        request_headers,
        body,
        meta,
        selected_account_id,
        request_detail,
        start_time,
    )
    .await
}

async fn send_codex_request(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    upstream_url: &str,
    proxy_url: Option<&str>,
    request_headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
) -> Result<reqwest::Response, AttemptOutcome> {
    let Some(proxy_url) = proxy_url else {
        return send_upstream_request_once(
            state,
            method,
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            upstream_url,
            proxy_url,
            request_headers,
            body,
            meta,
            selected_account_id,
            request_detail,
            start_time,
        )
        .await;
    };
    send_codex_with_fallback(
        state,
        method,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        upstream_url,
        request_headers,
        body,
        meta,
        selected_account_id,
        request_detail,
        start_time,
        proxy_url,
    )
    .await
}

async fn send_upstream_request_once(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    upstream_url: &str,
    proxy_url: Option<&str>,
    request_headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
) -> Result<reqwest::Response, AttemptOutcome> {
    log_debug_headers_body(
        "upstream.request",
        Some(request_headers),
        Some(body),
        DEBUG_UPSTREAM_LOG_LIMIT_BYTES,
    )
    .await;
    let client = state
        .http_clients
        .client_for_proxy_url(proxy_url)
        .map_err(|message| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_GATEWAY, message))
        })?;
    let upstream_body =
        request::build_upstream_body(provider, upstream, upstream_path_with_query, body, meta)
            .await?;
    match send_request_once(
        client,
        &method,
        upstream_url,
        request_headers,
        upstream_body,
        state.config.upstream_no_data_timeout,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(SendFailure::Transport(err)) => Err(map_upstream_error(
            state,
            provider,
            upstream,
            inbound_path,
            meta,
            selected_account_id,
            request_detail,
            err,
            start_time,
        )),
        Err(SendFailure::Timeout) => Err(handle_upstream_timeout(
            state,
            provider,
            upstream,
            inbound_path,
            meta,
            selected_account_id,
            request_detail,
            start_time,
        )),
    }
}

async fn send_codex_with_fallback(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    upstream_url: &str,
    request_headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    proxy_url: &str,
) -> Result<reqwest::Response, AttemptOutcome> {
    // Codex 代理回退：socks5h / http1_only，缓解 DNS/ALPN/TLS 兼容问题。
    let attempts = build_codex_send_attempts(proxy_url);
    let mut last_error: Option<reqwest::Error> = None;
    for attempt in attempts {
        match send_codex_attempt(
            state,
            &method,
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            upstream_url,
            request_headers,
            body,
            meta,
            selected_account_id,
            request_detail,
            start_time,
            &attempt,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(CodexAttemptError::Retry(err)) => last_error = Some(err),
            Err(CodexAttemptError::Fatal(outcome)) => return Err(outcome),
        }
    }
    Err(finalize_codex_fallback(
        state,
        provider,
        upstream,
        inbound_path,
        meta,
        selected_account_id,
        request_detail,
        start_time,
        last_error,
    ))
}

async fn send_codex_attempt(
    state: &ProxyState,
    method: &Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    upstream_url: &str,
    request_headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    attempt: &CodexSendAttempt,
) -> Result<reqwest::Response, CodexAttemptError> {
    log_debug_headers_body(
        "upstream.request",
        Some(request_headers),
        Some(body),
        DEBUG_UPSTREAM_LOG_LIMIT_BYTES,
    )
    .await;
    let client = build_codex_client(attempt.proxy_url.as_deref(), attempt.http1_only).map_err(
        |message| {
            CodexAttemptError::Fatal(AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                message,
            )))
        },
    )?;
    let upstream_body =
        request::build_upstream_body(provider, upstream, upstream_path_with_query, body, meta)
            .await
            .map_err(CodexAttemptError::Fatal)?;
    match send_request_once(
        client,
        method,
        upstream_url,
        request_headers,
        upstream_body,
        state.config.upstream_no_data_timeout,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(SendFailure::Timeout) => Err(CodexAttemptError::Fatal(handle_upstream_timeout(
            state,
            provider,
            upstream,
            inbound_path,
            meta,
            selected_account_id,
            request_detail,
            start_time,
        ))),
        Err(SendFailure::Transport(err)) => {
            if should_retry_codex_send(&err) {
                return Err(CodexAttemptError::Retry(err));
            }
            Err(CodexAttemptError::Fatal(map_upstream_error(
                state,
                provider,
                upstream,
                inbound_path,
                meta,
                selected_account_id,
                request_detail,
                err,
                start_time,
            )))
        }
    }
}

fn finalize_codex_fallback(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    last_error: Option<reqwest::Error>,
) -> AttemptOutcome {
    let Some(err) = last_error else {
        return AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_GATEWAY,
            "Codex upstream request failed.".to_string(),
        ));
    };
    map_upstream_error(
        state,
        provider,
        upstream,
        inbound_path,
        meta,
        selected_account_id,
        request_detail,
        err,
        start_time,
    )
}

async fn send_request_once(
    client: Client,
    method: &Method,
    upstream_url: &str,
    request_headers: &HeaderMap,
    upstream_body: reqwest::Body,
    upstream_no_data_timeout: Duration,
) -> Result<reqwest::Response, SendFailure> {
    let upstream_res = timeout(
        upstream_no_data_timeout,
        client
            .request(method.clone(), upstream_url)
            .headers(request_headers.clone())
            .body(upstream_body)
            .send(),
    )
    .await;
    match upstream_res {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(SendFailure::Transport(err)),
        Err(_) => Err(SendFailure::Timeout),
    }
}

struct CodexSendAttempt {
    proxy_url: Option<String>,
    http1_only: bool,
}

enum SendFailure {
    Transport(reqwest::Error),
    Timeout,
}

enum CodexAttemptError {
    Retry(reqwest::Error),
    Fatal(AttemptOutcome),
}

fn build_codex_send_attempts(proxy_url: &str) -> Vec<CodexSendAttempt> {
    let mut attempts = Vec::new();
    attempts.push(CodexSendAttempt {
        proxy_url: Some(proxy_url.to_string()),
        http1_only: false,
    });
    if let Some(upgraded) = upgrade_socks5(proxy_url) {
        attempts.push(CodexSendAttempt {
            proxy_url: Some(upgraded),
            http1_only: false,
        });
    }
    attempts.push(CodexSendAttempt {
        proxy_url: Some(proxy_url.to_string()),
        http1_only: true,
    });
    attempts
}

fn upgrade_socks5(proxy_url: &str) -> Option<String> {
    let value = proxy_url.trim();
    if value.starts_with("socks5h://") {
        return None;
    }
    if value.starts_with("socks5://") {
        return Some(value.replacen("socks5://", "socks5h://", 1));
    }
    None
}

fn build_codex_client(proxy_url: Option<&str>, http1_only: bool) -> Result<Client, String> {
    let mut builder = Client::builder();
    if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        let proxy = Proxy::all(proxy_url)
            .map_err(|_| "proxy_url is invalid or not supported.".to_string())?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.no_proxy();
    }
    if http1_only {
        builder = builder.http1_only();
    }
    builder
        .build()
        .map_err(|err| format!("Failed to build Codex upstream client: {err}"))
}

fn should_retry_codex_send(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_request()
}

fn handle_upstream_timeout(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
) -> AttemptOutcome {
    let message = format!(
        "Upstream did not respond within {}s.",
        state.config.upstream_no_data_timeout.as_secs()
    );
    result::log_upstream_error_if_needed(
        &state.log,
        request_detail,
        meta,
        provider,
        &upstream.id,
        selected_account_id,
        inbound_path,
        StatusCode::GATEWAY_TIMEOUT,
        message.clone(),
        start_time,
    );
    AttemptOutcome::Retryable {
        message,
        response: None,
        is_timeout: true,
        should_cooldown: true,
    }
}

fn map_upstream_error(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    meta: &RequestMeta,
    selected_account_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    err: reqwest::Error,
    start_time: Instant,
) -> AttemptOutcome {
    let message = sanitize_upstream_error(provider, &err);
    if is_retryable_error(&err) {
        let status = if err.is_timeout() {
            StatusCode::GATEWAY_TIMEOUT
        } else {
            StatusCode::BAD_GATEWAY
        };
        result::log_upstream_error_if_needed(
            &state.log,
            request_detail,
            meta,
            provider,
            &upstream.id,
            selected_account_id,
            inbound_path,
            status,
            message.clone(),
            start_time,
        );
        return AttemptOutcome::Retryable {
            message,
            response: None,
            is_timeout: err.is_timeout(),
            should_cooldown: true,
        };
    }
    let error_message = format!("Upstream request failed: {message}");
    result::log_upstream_error_if_needed(
        &state.log,
        request_detail,
        meta,
        provider,
        &upstream.id,
        selected_account_id,
        inbound_path,
        StatusCode::BAD_GATEWAY,
        error_message.clone(),
        start_time,
    );
    AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_GATEWAY, error_message))
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
