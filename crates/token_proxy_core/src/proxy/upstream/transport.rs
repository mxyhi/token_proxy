use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::Client;
use tokio::time::timeout;

use super::request_body;
use super::result;
use super::retry::mark_account_retryable_failure;
use super::utils::{is_retryable_error, sanitize_upstream_error};
use super::AttemptOutcome;
use crate::proxy::cooldown_scope::CooldownScope;
use crate::proxy::http;
use crate::proxy::log::RequestTimings;
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
    codex_openai_device_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    timings: RequestTimings,
    cooldown_scope: &CooldownScope,
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
            codex_openai_device_id,
            request_detail,
            start_time,
            timings,
            cooldown_scope,
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
        codex_openai_device_id,
        request_detail,
        start_time,
        timings,
        cooldown_scope,
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
    codex_openai_device_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    timings: RequestTimings,
    cooldown_scope: &CooldownScope,
) -> Result<reqwest::Response, AttemptOutcome> {
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
        codex_openai_device_id,
        request_detail,
        start_time,
        timings,
        proxy_url,
        cooldown_scope,
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
    codex_openai_device_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    timings: RequestTimings,
    cooldown_scope: &CooldownScope,
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
    let upstream_body = request_body::build_upstream_body(
        provider,
        upstream,
        upstream_path_with_query,
        body,
        meta,
        codex_openai_device_id,
    )
    .await?;
    let response_header_timeout = response_header_timeout_for_request(&state.config, meta);
    match send_request_once(
        client,
        &method,
        upstream_url,
        request_headers,
        upstream_body,
        response_header_timeout,
        start_time,
        timings,
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
            cooldown_scope,
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
            response_header_timeout,
            cooldown_scope,
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
    codex_openai_device_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    timings: RequestTimings,
    proxy_url: Option<&str>,
    cooldown_scope: &CooldownScope,
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
            codex_openai_device_id,
            request_detail,
            start_time,
            timings.clone(),
            &attempt,
            cooldown_scope,
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
        cooldown_scope,
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
    codex_openai_device_id: Option<&str>,
    request_detail: Option<&RequestDetailSnapshot>,
    start_time: Instant,
    timings: RequestTimings,
    attempt: &CodexSendAttempt,
    cooldown_scope: &CooldownScope,
) -> Result<reqwest::Response, CodexAttemptError> {
    log_debug_headers_body(
        "upstream.request",
        Some(request_headers),
        Some(body),
        DEBUG_UPSTREAM_LOG_LIMIT_BYTES,
    )
    .await;
    let client = state
        .http_clients
        .codex_client_for_proxy_url(attempt.proxy_url.as_deref(), attempt.http1_only)
        .map_err(|message| {
            CodexAttemptError::Fatal(AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_GATEWAY,
                message,
            )))
        })?;
    let upstream_body = request_body::build_upstream_body(
        provider,
        upstream,
        upstream_path_with_query,
        body,
        meta,
        codex_openai_device_id,
    )
    .await
    .map_err(CodexAttemptError::Fatal)?;
    let response_header_timeout = response_header_timeout_for_request(&state.config, meta);
    match send_request_once(
        client,
        method,
        upstream_url,
        request_headers,
        upstream_body,
        response_header_timeout,
        start_time,
        timings,
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
            response_header_timeout,
            cooldown_scope,
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
                cooldown_scope,
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
    cooldown_scope: &CooldownScope,
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
        cooldown_scope,
    )
}

async fn send_request_once(
    client: Client,
    method: &Method,
    upstream_url: &str,
    request_headers: &HeaderMap,
    upstream_body: reqwest::Body,
    response_header_timeout: Option<Duration>,
    start_time: Instant,
    timings: RequestTimings,
) -> Result<reqwest::Response, SendFailure> {
    let request = client
        .request(method.clone(), upstream_url)
        .headers(request_headers.clone())
        .body(upstream_body)
        .send();
    let upstream_response = match response_header_timeout {
        Some(duration) => timeout(timeout_remaining(duration, start_time), request).await,
        None => Ok(request.await),
    };
    match upstream_response {
        Ok(Ok(response)) => {
            timings.mark_upstream_response_headers(start_time.elapsed().as_millis());
            Ok(response)
        }
        Ok(Err(err)) => Err(SendFailure::Transport(err)),
        Err(_) => Err(SendFailure::Timeout),
    }
}

fn timeout_remaining(timeout_duration: Duration, start_time: Instant) -> Duration {
    timeout_duration.saturating_sub(start_time.elapsed())
}

fn response_header_timeout_for_request(
    config: &crate::proxy::config::ProxyConfig,
    meta: &RequestMeta,
) -> Option<Duration> {
    // 只约束 headers 阶段；stream body 首个可见输出会用同一 attempt deadline 的剩余时间。
    if meta.stream {
        Some(config.stream_first_output_timeout)
    } else {
        Some(config.sync_response_timeout)
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

fn build_codex_send_attempts(proxy_url: Option<&str>) -> Vec<CodexSendAttempt> {
    let mut attempts = Vec::new();
    let Some(proxy_url) = proxy_url else {
        attempts.push(CodexSendAttempt {
            proxy_url: None,
            http1_only: false,
        });
        return attempts;
    };
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
    response_header_timeout: Option<Duration>,
    cooldown_scope: &CooldownScope,
) -> AttemptOutcome {
    let timeout_secs = response_header_timeout
        .unwrap_or(state.config.sync_response_timeout)
        .as_secs();
    let message = format!("Upstream did not respond within {}s.", timeout_secs);
    mark_account_retryable_failure(
        state,
        provider,
        selected_account_id,
        Some(message.clone()),
        cooldown_scope,
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
        retry_same_upstream_once: false,
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
    cooldown_scope: &CooldownScope,
) -> AttemptOutcome {
    let message = sanitize_upstream_error(provider, &err);
    if is_retryable_error(&err) {
        let status = if err.is_timeout() {
            StatusCode::GATEWAY_TIMEOUT
        } else {
            StatusCode::BAD_GATEWAY
        };
        mark_account_retryable_failure(
            state,
            provider,
            selected_account_id,
            Some(message.clone()),
            cooldown_scope,
        );
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
            retry_same_upstream_once: false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::LogLevel;
    use crate::proxy::config::{ProxyConfig, UpstreamStrategyRuntime};
    use std::collections::HashMap;

    fn test_config() -> ProxyConfig {
        ProxyConfig {
            host: "127.0.0.1".to_string(),
            port: 9208,
            local_api_key: None,
            cors_enabled: false,
            model_list_prefix: false,
            log_level: LogLevel::Silent,
            max_request_body_bytes: 1024,
            retryable_failure_cooldown: Duration::from_secs(15),
            codex_session_scoped_cooldown_enabled: false,
            stream_first_output_timeout: Duration::from_secs(60),
            sync_response_timeout: Duration::from_secs(120),
            upstream_strategy: UpstreamStrategyRuntime::default(),
            hot_model_mappings: HashMap::new(),
            upstreams: HashMap::new(),
            kiro_preferred_endpoint: None,
        }
    }

    #[test]
    fn response_header_timeout_uses_split_request_timeout() {
        let config = test_config();
        let mut meta = RequestMeta {
            client_ip: None,
            stream: true,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
            estimated_input_tokens: None,
        };

        assert_eq!(
            response_header_timeout_for_request(&config, &meta),
            Some(Duration::from_secs(60))
        );
        meta.stream = false;
        assert_eq!(
            response_header_timeout_for_request(&config, &meta),
            Some(Duration::from_secs(120))
        );
    }
}
