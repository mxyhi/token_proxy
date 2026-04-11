use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::{Client, Proxy};
use tokio::time::timeout;

use super::request;
use super::result;
use super::retry::mark_account_retryable_failure;
use super::utils::{is_retryable_error, sanitize_upstream_error};
use super::AttemptOutcome;
use crate::proxy::http;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::server_helpers::log_debug_headers_body;
use crate::proxy::{config::UpstreamRuntime, ProxyState, RequestMeta};

const DEBUG_UPSTREAM_LOG_LIMIT_BYTES: usize = usize::MAX;

pub(super) async fn send_upstream_request(
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
        Ok(response) => Ok(response),
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
            Ok(response) => return Ok(response),
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
        Ok(response) => Ok(response),
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
    let upstream_response = timeout(
        upstream_no_data_timeout,
        client
            .request(method.clone(), upstream_url)
            .headers(request_headers.clone())
            .body(upstream_body)
            .send(),
    )
    .await;
    match upstream_response {
        Ok(Ok(response)) => Ok(response),
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
    mark_account_retryable_failure(state, provider, selected_account_id, Some(message.clone()));
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
        mark_account_retryable_failure(state, provider, selected_account_id, Some(message.clone()));
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
