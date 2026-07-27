use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::Client;
use tokio::time::timeout;

use super::request_body;
use super::result;
use super::retry::mark_account_retryable_failure;
use super::transport_error::{analyze_transport_error, TransportRecovery};
use super::AttemptOutcome;
use crate::proxy::cooldown_scope::CooldownScope;
use crate::proxy::http;
use crate::proxy::http_client::proxy_log_target;
use crate::proxy::log::RequestTimings;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::server_helpers::log_debug_headers_body;
use crate::proxy::{ProxyState, RequestMeta};
use token_proxy_config::UpstreamRuntime;

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
    // 0: 共享 H2 池；1: rotate 后 H2（force fresh）；2: HTTP/1.1 降级。
    // 每步都重建 body，因为 reqwest::Body 只能消费一次。
    let mut last_transport_error: Option<reqwest::Error> = None;
    for transport_step in 0u8..3 {
        let client = match resolve_upstream_client(state, provider, proxy_url, transport_step) {
            Ok(client) => client,
            Err(message) => {
                return Err(AttemptOutcome::Fatal(http::error_response(
                    StatusCode::BAD_GATEWAY,
                    message,
                )));
            }
        };
        let upstream_body = request_body::build_upstream_body(
            provider,
            upstream,
            upstream_path_with_query,
            body,
            meta,
            codex_openai_device_id,
            request_headers,
            state.config.xai_inject_x_search,
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
            timings.clone(),
        )
        .await
        {
            Ok(response) => {
                if transport_step > 0 {
                    tracing::info!(
                        provider,
                        upstream = %upstream.id,
                        transport_step,
                        "upstream request recovered after transport recovery step"
                    );
                }
                return Ok(response);
            }
            Err(SendFailure::Timeout) => {
                return Err(handle_upstream_timeout(
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
                    TransportRecovery::SameUpstreamOnce,
                ));
            }
            Err(SendFailure::Transport(err)) => {
                let stale = super::utils::is_stale_connection_transport_error(&err);
                tracing::warn!(
                    provider,
                    upstream = %upstream.id,
                    transport_step,
                    stale,
                    error = %err,
                    "upstream send failed before response headers"
                );
                if stale && transport_step < 2 {
                    last_transport_error = Some(err);
                    continue;
                }
                return Err(map_upstream_error(
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
                    TransportRecovery::SameUpstreamOnce,
                ));
            }
        }
    }
    let err = last_transport_error.expect("stale transport loop always records an error");
    Err(map_upstream_error(
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
        TransportRecovery::SameUpstreamOnce,
    ))
}

fn resolve_upstream_client(
    state: &ProxyState,
    provider: &str,
    proxy_url: Option<&str>,
    transport_step: u8,
) -> Result<reqwest::Client, String> {
    match transport_step {
        0 if provider == "xai" => state.http_clients.xai_client_for_proxy_url(proxy_url),
        0 => state.http_clients.client_for_proxy_url(proxy_url),
        1 if provider == "xai" => {
            // xAI bearer 不允许跨主机重放；rotate 后仍使用禁重定向池。
            state
                .http_clients
                .rotate_xai_client_for_proxy_url(proxy_url)
        }
        1 => {
            // 丢弃毒 H2 session，强制下一次走新 TCP/TLS/H2。
            state.http_clients.rotate_client_for_proxy_url(proxy_url)
        }
        _ => {
            tracing::warn!(
                proxy = %proxy_log_target(proxy_url),
                "falling back to HTTP/1.1 after repeated pre-header H2/transport failures"
            );
            if provider == "xai" {
                state.http_clients.xai_client_for_proxy_url_http1(proxy_url)
            } else {
                state.http_clients.client_for_proxy_url_http1(proxy_url)
            }
        }
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
    // Codex 代理失败只重放一次；第二次合并 socks5h 与 HTTP/1，避免同一 POST 第三次发送。
    let attempts = build_codex_send_attempts(proxy_url);
    let attempt_count = attempts.len();
    let mut last_error: Option<reqwest::Error> = None;
    for (attempt_index, attempt) in attempts.into_iter().enumerate() {
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
            Err(CodexAttemptError::Retry(err)) => {
                if attempt_index + 1 < attempt_count {
                    tracing::warn!(
                        attempt = attempt_index + 1,
                        attempt_count,
                        http1_only = attempt.http1_only,
                        "Codex request failed before headers; retrying with fallback transport"
                    );
                }
                last_error = Some(err);
            }
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
        request_headers,
        state.config.xai_inject_x_search,
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
            TransportRecovery::NextUpstream,
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
                TransportRecovery::NextUpstream,
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
    // Codex 已在本层穷尽 proxy 模式，外层还会处理账号与 upstream；禁止重新跑整条账号链。
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
        TransportRecovery::NextUpstream,
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
    config: &token_proxy_config::ProxyConfig,
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
    attempts.push(CodexSendAttempt {
        proxy_url: Some(upgrade_socks5(proxy_url).unwrap_or_else(|| proxy_url.to_string())),
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
    _request_detail: Option<&RequestDetailSnapshot>,
    _start_time: Instant,
    response_header_timeout: Option<Duration>,
    cooldown_scope: &CooldownScope,
    request_recovery: TransportRecovery,
) -> AttemptOutcome {
    let timeout_secs = response_header_timeout
        .unwrap_or(state.config.sync_response_timeout)
        .as_secs();
    let message = format!("Upstream did not respond within {}s.", timeout_secs);
    tracing::warn!(
        provider,
        upstream = %upstream.id,
        recovery = request_recovery.as_str(),
        status = StatusCode::GATEWAY_TIMEOUT.as_u16(),
        timeout_secs,
        "upstream request timed out before response headers"
    );
    mark_account_retryable_failure(
        state,
        provider,
        selected_account_id,
        Some(message.clone()),
        cooldown_scope,
    );
    // 不立即写 SQLite：若 same-upstream / failover 恢复成功，不应出现中间 504 行。
    let deferred_log = Some(format!(
        "provider={provider}; class=timeout; recovery={}; status=504; message={message}",
        request_recovery.as_str(),
    ));
    let _ = (inbound_path, meta);
    AttemptOutcome::Retryable {
        message,
        response: None,
        is_timeout: true,
        should_cooldown: true,
        deferred_log,
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
    request_recovery: TransportRecovery,
) -> AttemptOutcome {
    let failure = analyze_transport_error(provider, err, request_recovery);
    tracing::warn!(
        provider,
        upstream = %upstream.id,
        transport_class = failure.class.as_str(),
        recovery = failure.recovery.as_str(),
        status = failure.status.as_u16(),
        is_timeout = failure.is_timeout,
        diagnostic = %failure.diagnostic_message,
        "upstream request failed before response headers"
    );

    if failure.recovery == TransportRecovery::Fatal {
        let client_message = format!("Upstream request failed: {}", failure.client_message);
        let diagnostic_message = format!("Upstream request failed: {}", failure.diagnostic_message);
        result::log_upstream_error_if_needed(
            &state.log,
            request_detail,
            meta,
            provider,
            &upstream.id,
            selected_account_id,
            inbound_path,
            failure.status,
            diagnostic_message,
            start_time,
        );
        return AttemptOutcome::Fatal(http::error_response(failure.status, client_message));
    }

    mark_account_retryable_failure(
        state,
        provider,
        selected_account_id,
        Some(failure.client_message.clone()),
        cooldown_scope,
    );
    // Retryable 诊断延后到本请求终态失败再落库；恢复成功则不刷中间 502。
    let _ = (request_detail, start_time, inbound_path, meta);
    AttemptOutcome::Retryable {
        message: failure.client_message,
        response: None,
        is_timeout: failure.is_timeout,
        should_cooldown: true,
        deferred_log: Some(failure.diagnostic_message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::LogLevel;
    use axum::{body::to_bytes, extract::State, response::IntoResponse, routing::any, Router};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use token_proxy_config::{ProxyConfig, UpstreamStrategyRuntime};

    #[derive(Clone, Default)]
    struct CapturedMediaRequest {
        body: Arc<Mutex<Option<axum::body::Bytes>>>,
        content_type: Arc<Mutex<Option<String>>>,
    }

    async fn capture_media_request(
        State(capture): State<CapturedMediaRequest>,
        headers: axum::http::HeaderMap,
        body: axum::body::Body,
    ) -> axum::response::Response {
        let body = to_bytes(body, usize::MAX)
            .await
            .expect("read captured media body");
        *capture.body.lock().expect("captured body lock") = Some(body);
        *capture
            .content_type
            .lock()
            .expect("captured content-type lock") = headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        axum::http::StatusCode::OK.into_response()
    }

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
            same_upstream_retry_count: 1,
            codex_session_scoped_cooldown_enabled: false,
            xai_inject_x_search: false,
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
            billing: Default::default(),
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

    #[test]
    fn codex_proxy_attempts_allow_at_most_one_replay() {
        let direct = build_codex_send_attempts(None);
        let http = build_codex_send_attempts(Some("http://127.0.0.1:8080"));
        let socks5 = build_codex_send_attempts(Some("socks5://127.0.0.1:1080"));
        let socks5h = build_codex_send_attempts(Some("socks5h://127.0.0.1:1080"));

        assert_eq!(direct.len(), 1);
        assert_eq!(http.len(), 2);
        assert_eq!(socks5.len(), 2);
        assert_eq!(socks5h.len(), 2);
        assert_eq!(
            socks5[1].proxy_url.as_deref(),
            Some("socks5h://127.0.0.1:1080")
        );
        assert!(http[1].http1_only);
        assert!(socks5[1].http1_only);
        assert!(socks5h[1].http1_only);
    }

    #[tokio::test]
    async fn xai_image_edits_send_preserves_multipart_boundary_and_body() {
        let capture = CapturedMediaRequest::default();
        let app = Router::new()
            .route("/{*path}", any(capture_media_request))
            .with_state(capture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind media capture upstream");
        let address = listener.local_addr().expect("media capture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("media capture upstream should run");
        });

        let boundary = "token-proxy-xai-boundary";
        let payload = axum::body::Bytes::from_static(
            b"--token-proxy-xai-boundary\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit\r\n--token-proxy-xai-boundary--\r\n",
        );
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let body = crate::proxy::request_body::ReplayableBody::from_bytes(payload.clone());
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_str(&content_type).expect("multipart content type"),
        );
        super::super::prepare::enforce_xai_request_headers(
            "/v1/images/edits",
            &body,
            false,
            None,
            &mut headers,
        );
        let client = Client::builder()
            .no_proxy()
            .build()
            .expect("build media capture client");
        let response = send_request_once(
            client,
            &Method::POST,
            &format!("http://{address}/v1/images/edits"),
            &headers,
            body.to_reqwest_body().await.expect("build request body"),
            Some(Duration::from_secs(5)),
            Instant::now(),
            RequestTimings::default(),
        )
        .await;

        server.abort();
        let response = match response {
            Ok(response) => response,
            Err(_) => panic!("multipart media request should reach local upstream"),
        };
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            capture
                .content_type
                .lock()
                .expect("captured content-type lock")
                .as_deref(),
            Some(content_type.as_str())
        );
        assert_eq!(
            capture.body.lock().expect("captured body lock").as_ref(),
            Some(&payload)
        );
    }
}
