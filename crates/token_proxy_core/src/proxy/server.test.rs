use super::*;

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::any,
    Router,
};
use serde_json::{json, Value};
use sqlx::Row;
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::{runtime::Runtime, sync::RwLock, task::JoinHandle};

use crate::logging::LogLevel;
use crate::paths::TokenProxyPaths;
use crate::proxy::config::{
    InboundApiFormat, ProviderUpstreams, ProxyConfig, UpstreamDispatchRuntime, UpstreamGroup,
    UpstreamOrderStrategy, UpstreamRuntime, UpstreamStrategyRuntime,
};

const FORMATS_ALL: &[InboundApiFormat] = &[
    InboundApiFormat::OpenaiChat,
    InboundApiFormat::OpenaiResponses,
    InboundApiFormat::AnthropicMessages,
    InboundApiFormat::Gemini,
];

const FORMATS_CHAT: &[InboundApiFormat] = &[InboundApiFormat::OpenaiChat];
const FORMATS_RESPONSES: &[InboundApiFormat] = &[InboundApiFormat::OpenaiResponses];
const FORMATS_MESSAGES: &[InboundApiFormat] = &[InboundApiFormat::AnthropicMessages];
const FORMATS_GEMINI: &[InboundApiFormat] = &[InboundApiFormat::Gemini];

const FORMATS_KIRO_NATIVE: &[InboundApiFormat] = &[InboundApiFormat::AnthropicMessages];

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    Runtime::new()
        .expect("create tokio runtime")
        .block_on(future)
}

fn config_with_providers(providers: &[(&'static str, &'static [InboundApiFormat])]) -> ProxyConfig {
    let upstreams: Vec<(&'static str, i32, &'static str, &'static [InboundApiFormat])> = providers
        .iter()
        .map(|(provider, formats)| (*provider, 0, *provider, *formats))
        .collect();
    config_with_upstreams(&upstreams)
}

fn config_with_upstreams(
    upstreams: &[(&'static str, i32, &'static str, &'static [InboundApiFormat])],
) -> ProxyConfig {
    let upstreams_with_urls: Vec<(&str, i32, &str, &str, &[InboundApiFormat])> = upstreams
        .iter()
        .map(|(provider, priority, id, inbound_formats)| {
            (
                *provider,
                *priority,
                *id,
                "https://example.com",
                *inbound_formats,
            )
        })
        .collect();
    config_with_runtime_upstreams(&upstreams_with_urls)
}

fn config_with_runtime_upstreams(
    upstreams: &[(&str, i32, &str, &str, &[InboundApiFormat])],
) -> ProxyConfig {
    let mut provider_map: HashMap<String, ProviderUpstreams> = HashMap::new();
    for (provider, priority, id, base_url, inbound_formats) in upstreams {
        let mut runtime = UpstreamRuntime {
            id: (*id).to_string(),
            selector_key: (*id).to_string(),
            base_url: (*base_url).to_string(),
            api_key: Some("test-key".to_string()),
            filter_prompt_cache_retention: false,
            filter_safety_identifier: false,
            rewrite_developer_role_to_system: false,
            kiro_account_id: None,
            codex_account_id: (*provider == PROVIDER_CODEX).then(|| format!("codex-{id}.json")),
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: *priority,
            model_mappings: None,
            header_overrides: None,
            allowed_inbound_formats: Default::default(),
        };
        runtime
            .allowed_inbound_formats
            .extend(inbound_formats.iter().copied());
        let entry = provider_map
            .entry((*provider).to_string())
            .or_insert_with(|| ProviderUpstreams { groups: Vec::new() });
        if let Some(group) = entry
            .groups
            .iter_mut()
            .find(|group| group.priority == *priority)
        {
            group.items.push(runtime);
        } else {
            entry.groups.push(UpstreamGroup {
                priority: *priority,
                items: vec![runtime],
            });
        }
    }
    for upstreams in provider_map.values_mut() {
        upstreams
            .groups
            .sort_by(|left, right| right.priority.cmp(&left.priority));
    }
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: None,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 20 * 1024 * 1024,
        retryable_failure_cooldown: std::time::Duration::from_secs(15),
        upstream_no_data_timeout: std::time::Duration::from_secs(120),
        upstream_strategy: UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::RoundRobin,
            dispatch: UpstreamDispatchRuntime::Serial,
        },
        upstreams: provider_map,
        kiro_preferred_endpoint: None,
    }
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    path: String,
    body: Value,
    authorization: Option<String>,
    chatgpt_account_id: Option<String>,
}

#[derive(Clone)]
struct MockUpstreamState {
    status: StatusCode,
    body: Value,
    delay_ms: u64,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

#[derive(Clone)]
struct MockRawUpstreamState {
    status: StatusCode,
    body: Bytes,
    content_type: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

struct MockUpstream {
    base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: JoinHandle<()>,
}

impl MockUpstream {
    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    fn abort(self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct MockAuthSwitchState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    primary_status: StatusCode,
}

#[derive(Clone)]
struct MockKiroAuthSwitchState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    primary_status: StatusCode,
}

async fn mock_upstream_handler(
    State(state): State<Arc<MockUpstreamState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> axum::response::Response {
    let bytes = to_bytes(body, usize::MAX).await.expect("read mock body");
    let json_body = serde_json::from_slice::<Value>(&bytes).expect("mock request json");
    state
        .requests
        .lock()
        .expect("requests lock")
        .push(RecordedRequest {
            path: uri.path().to_string(),
            body: json_body,
            authorization: headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            chatgpt_account_id: headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        });
    if state.delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(state.delay_ms)).await;
    }
    (
        state.status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.body.to_string(),
    )
        .into_response()
}

async fn spawn_mock_upstream(status: StatusCode, body: Value) -> MockUpstream {
    spawn_mock_upstream_with_delay(status, body, 0).await
}

async fn spawn_mock_upstream_with_delay(
    status: StatusCode,
    body: Value,
    delay_ms: u64,
) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockUpstreamState {
        status,
        body,
        delay_ms,
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(mock_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn mock_raw_upstream_handler(
    State(state): State<Arc<MockRawUpstreamState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> axum::response::Response {
    let bytes = to_bytes(body, usize::MAX).await.expect("read mock body");
    let json_body = serde_json::from_slice::<Value>(&bytes).expect("mock request json");
    state
        .requests
        .lock()
        .expect("requests lock")
        .push(RecordedRequest {
            path: uri.path().to_string(),
            body: json_body,
            authorization: headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            chatgpt_account_id: headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        });
    axum::response::Response::builder()
        .status(state.status)
        .header(axum::http::header::CONTENT_TYPE, state.content_type.as_str())
        .body(Body::from(state.body.clone()))
        .expect("build raw mock response")
}

async fn spawn_mock_raw_upstream(
    status: StatusCode,
    body: Bytes,
    content_type: &str,
) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockRawUpstreamState {
        status,
        body,
        content_type: content_type.to_string(),
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(mock_raw_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("raw mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn auth_switch_upstream_handler(
    State(state): State<Arc<MockAuthSwitchState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> axum::response::Response {
    let bytes = to_bytes(body, usize::MAX).await.expect("read mock body");
    let json_body = serde_json::from_slice::<Value>(&bytes).expect("mock request json");
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let chatgpt_account_id = headers
        .get("chatgpt-account-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    state
        .requests
        .lock()
        .expect("requests lock")
        .push(RecordedRequest {
            path: uri.path().to_string(),
            body: json_body,
            authorization: authorization.clone(),
            chatgpt_account_id: chatgpt_account_id.clone(),
        });

    let (status, body) = match authorization.as_deref() {
        Some("Bearer codex-access-a") => (
            state.primary_status,
            if state.primary_status == StatusCode::UNAUTHORIZED {
                json!({
                    "error": {
                        "message": "Your authentication token has been invalidated. Please try signing in again.",
                        "type": "invalid_request_error",
                        "code": "token_invalidated",
                        "param": null
                    }
                })
            } else {
                json!({
                    "error": {
                        "message": format!("primary failed: {}", state.primary_status.as_u16()),
                        "type": "invalid_request_error",
                        "code": "bad_request",
                        "param": null
                    }
                })
            },
        ),
        Some("Bearer codex-access-b") => (
            StatusCode::OK,
            json!({
                "id": "resp_codex_failover",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from codex failover" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        ),
        _ => (
            StatusCode::UNAUTHORIZED,
            json!({
                "error": {
                    "message": "unexpected account",
                    "type": "invalid_request_error",
                    "code": "token_invalidated",
                    "param": null
                }
            }),
        ),
    };

    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn spawn_auth_switch_mock_upstream() -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockAuthSwitchState {
        requests: requests.clone(),
        primary_status: StatusCode::UNAUTHORIZED,
    });
    let app = Router::new()
        .route("/{*path}", any(auth_switch_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind auth switch mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("auth switch mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn spawn_auth_switch_mock_upstream_with_primary_status(
    primary_status: StatusCode,
) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockAuthSwitchState {
        requests: requests.clone(),
        primary_status,
    });
    let app = Router::new()
        .route("/{*path}", any(auth_switch_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind auth switch mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("auth switch mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn kiro_auth_switch_upstream_handler(
    State(state): State<Arc<MockKiroAuthSwitchState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> axum::response::Response {
    let bytes = to_bytes(body, usize::MAX).await.expect("read mock body");
    let json_body = serde_json::from_slice::<Value>(&bytes).expect("mock request json");
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    state
        .requests
        .lock()
        .expect("requests lock")
        .push(RecordedRequest {
            path: uri.path().to_string(),
            body: json_body,
            authorization: authorization.clone(),
            chatgpt_account_id: None,
        });

    let (status, content_type, body) = match authorization.as_deref() {
        Some("Bearer kiro-access-a") => (
            state.primary_status,
            "application/json",
            (json!({
                "error": {
                    "message": format!("primary failed: {}", state.primary_status.as_u16())
                }
            }))
            .to_string()
            .into_bytes(),
        ),
        Some("Bearer kiro-access-b") => (
            StatusCode::OK,
            "application/vnd.amazon.eventstream",
            build_kiro_event_stream("from kiro failover").to_vec(),
        ),
        _ => (
            StatusCode::UNAUTHORIZED,
            "application/json",
            (json!({
                "error": {
                    "message": "unexpected account"
                }
            }))
            .to_string()
            .into_bytes(),
        ),
    };

    axum::response::Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .expect("build kiro auth switch response")
}

async fn spawn_kiro_auth_switch_mock_upstream(primary_status: StatusCode) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockKiroAuthSwitchState {
        requests: requests.clone(),
        primary_status,
    });
    let app = Router::new()
        .route("/{*path}", any(kiro_auth_switch_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind kiro auth switch mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("kiro auth switch mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

#[test]
fn responses_request_hedged_delay_prefers_faster_same_priority_upstream() {
    run_async(async {
        let slow_primary = spawn_mock_upstream_with_delay(
            StatusCode::OK,
            json!({
                "id": "resp_from_slow_primary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from slow primary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
            300,
        )
        .await;
        let fast_secondary = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_fast_secondary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from fast secondary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                slow_primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "responses-secondary",
                fast_secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Hedged {
                delay: std::time::Duration::from_millis(50),
                max_parallel: 2,
            },
        };

        let data_dir = next_test_data_dir("responses_hedged_request");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (status, json) = send_responses_request(state).await;
        let primary_requests = slow_primary.requests();
        let secondary_requests = fast_secondary.requests();

        slow_primary.abort();
        fast_secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from fast secondary")
        );
        assert_eq!(primary_requests.len(), 1);
        assert_eq!(secondary_requests.len(), 1);
    });
}

#[test]
fn responses_request_race_prefers_faster_same_priority_upstream() {
    run_async(async {
        let slow_primary = spawn_mock_upstream_with_delay(
            StatusCode::OK,
            json!({
                "id": "resp_from_slow_primary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from slow primary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
            300,
        )
        .await;
        let fast_secondary = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_fast_secondary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from fast secondary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                slow_primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "responses-secondary",
                fast_secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::RoundRobin,
            dispatch: UpstreamDispatchRuntime::Race { max_parallel: 2 },
        };

        let data_dir = next_test_data_dir("responses_race_request");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (status, json) = send_responses_request(state).await;
        let primary_requests = slow_primary.requests();
        let secondary_requests = fast_secondary.requests();

        slow_primary.abort();
        fast_secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from fast secondary")
        );
        assert_eq!(primary_requests.len(), 1);
        assert_eq!(secondary_requests.len(), 1);
    });
}

fn next_test_data_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("token_proxy_server_test_{label}_{stamp}"))
}

async fn build_test_state_handle(config: ProxyConfig, data_dir: PathBuf) -> ProxyStateHandle {
    std::fs::create_dir_all(&data_dir).expect("create test data dir");
    let paths = TokenProxyPaths::from_app_data_dir(data_dir).expect("test paths");
    build_test_state_handle_with_paths(config, paths, None).await
}

async fn build_test_state_handle_with_sqlite_log(
    config: ProxyConfig,
    data_dir: PathBuf,
) -> (ProxyStateHandle, sqlx::SqlitePool) {
    std::fs::create_dir_all(&data_dir).expect("create test data dir");
    let paths = TokenProxyPaths::from_app_data_dir(data_dir).expect("test paths");
    let pool = crate::proxy::sqlite::open_write_pool(&paths)
        .await
        .expect("open sqlite pool");
    let state = build_test_state_handle_with_paths(config, paths, Some(pool.clone())).await;
    (state, pool)
}

async fn build_test_state_handle_with_paths(
    config: ProxyConfig,
    paths: TokenProxyPaths,
    log_pool: Option<sqlx::SqlitePool>,
) -> ProxyStateHandle {
    let app_proxy = crate::app_proxy::new_state();
    let cursors = build_upstream_cursors(&config);
    let kiro_accounts = Arc::new(
        crate::kiro::KiroAccountStore::new(&paths, app_proxy.clone()).expect("kiro store"),
    );
    let codex_accounts = Arc::new(
        crate::codex::CodexAccountStore::new(&paths, app_proxy.clone()).expect("codex store"),
    );
    let _ = app_proxy;
    let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format expires_at");
    for upstreams in config.upstreams.values() {
        for group in &upstreams.groups {
            for upstream in &group.items {
                let Some(account_id) = upstream.codex_account_id.as_deref() else {
                    continue;
                };
                codex_accounts
                    .save_record(
                        account_id.to_string(),
                        crate::codex::CodexTokenRecord {
                            access_token: "codex-access-token".to_string(),
                            refresh_token: "codex-refresh-token".to_string(),
                            id_token: "codex-id-token".to_string(),
                            auto_refresh_enabled: true,
                            status: crate::codex::CodexAccountStatus::Active,
                            account_id: Some("chatgpt-account".to_string()),
                            email: Some("codex@example.com".to_string()),
                            expires_at: expires_at.clone(),
                            last_refresh: None,
                            proxy_url: None,
                            quota: crate::codex::CodexQuotaCache::default(),
                        },
                    )
                    .await
                    .expect("seed codex account");
                codex_accounts
                    .list_accounts()
                    .await
                    .expect("refresh codex account cache");
            }
        }
    }
    let retryable_failure_cooldown = config.retryable_failure_cooldown;
    let state = Arc::new(ProxyState {
        config,
        http_clients: super::super::http_client::ProxyHttpClients::new().expect("http clients"),
        log: Arc::new(super::super::log::LogWriter::new(log_pool)),
        cursors,
        upstream_selector:
            super::super::upstream_selector::UpstreamSelectorRuntime::new_with_cooldown(
                retryable_failure_cooldown,
            ),
        account_selector:
            super::super::account_selector::AccountSelectorRuntime::new_with_cooldown(
                retryable_failure_cooldown,
            ),
        request_detail: Arc::new(super::super::request_detail::RequestDetailCapture::new(
            None,
        )),
        token_rate: super::super::token_rate::TokenRateTracker::new(),
        kiro_accounts,
        codex_accounts,
    });
    Arc::new(RwLock::new(state))
}

async fn seed_codex_account(
    state: &ProxyStateHandle,
    storage_account_id: &str,
    access_token: &str,
    chatgpt_account_id: &str,
    expires_at: &str,
) {
    let state_guard = state.read().await;
    state_guard
        .codex_accounts
        .save_record(
            storage_account_id.to_string(),
            crate::codex::CodexTokenRecord {
                access_token: access_token.to_string(),
                refresh_token: "codex-refresh-token".to_string(),
                id_token: "codex-id-token".to_string(),
                auto_refresh_enabled: true,
                status: crate::codex::CodexAccountStatus::Active,
                account_id: Some(chatgpt_account_id.to_string()),
                email: Some(format!("{storage_account_id}@example.com")),
                expires_at: expires_at.to_string(),
                last_refresh: None,
                proxy_url: None,
                quota: crate::codex::CodexQuotaCache::default(),
            },
        )
        .await
        .expect("seed codex account");
}

async fn seed_kiro_account(
    state: &ProxyStateHandle,
    storage_account_id: &str,
    access_token: &str,
    expires_at: &str,
) {
    let state_guard = state.read().await;
    state_guard
        .kiro_accounts
        .save_record(
            storage_account_id.to_string(),
            crate::kiro::KiroTokenRecord {
                provider: "kiro".to_string(),
                auth_method: "social".to_string(),
                access_token: access_token.to_string(),
                refresh_token: "kiro-refresh-token".to_string(),
                client_id: None,
                client_secret: None,
                email: Some(format!("{storage_account_id}@example.com")),
                expires_at: expires_at.to_string(),
                last_refresh: None,
                profile_arn: None,
                start_url: None,
                region: None,
                proxy_url: None,
                status: crate::kiro::KiroAccountStatus::Active,
                quota: crate::kiro::KiroQuotaCache::default(),
            },
        )
        .await
        .expect("seed kiro account");
}

fn build_kiro_event_stream(text: &str) -> Bytes {
    let mut payload = Vec::new();
    payload.extend(encode_kiro_event_frame(
        json!({
            "assistantResponseEvent": {
                "content": text
            }
        })
        .to_string()
        .as_bytes(),
    ));
    payload.extend(encode_kiro_event_frame(
        json!({
            "messageStopEvent": {
                "stopReason": "end_turn"
            }
        })
        .to_string()
        .as_bytes(),
    ));
    Bytes::from(payload)
}

fn encode_kiro_event_frame(payload: &[u8]) -> Vec<u8> {
    let total_len = (16 + payload.len()) as u32;
    let mut frame = Vec::with_capacity(total_len as usize);
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(payload);
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame
}

async fn assert_responses_retry_fallback_status(status: StatusCode) {
    let primary = spawn_mock_upstream(
        status,
        json!({
            "error": { "message": format!("primary failed: {}", status.as_u16()) }
        }),
    )
    .await;
    let fallback = spawn_mock_upstream(
        StatusCode::OK,
        json!({
            "id": "resp_from_codex",
            "object": "response",
            "created_at": 123,
            "model": "gpt-5-codex",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "from codex fallback" }
                    ]
                }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
        }),
    )
    .await;

    // 这里直接调用 `proxy_request`，只把真实网络留给 upstream mock；
    // 这样能精确覆盖 dispatch / retry / fallback，而不额外引入完整服务生命周期噪音。
    let config = config_with_runtime_upstreams(&[
        (
            PROVIDER_RESPONSES,
            10,
            "responses-primary",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_CODEX,
            5,
            "codex-fallback",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    let data_dir = next_test_data_dir("responses_codex_fallback");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "input": "hi"
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy response bytes");
    let response_json: Value =
        serde_json::from_slice(&response_bytes).expect("proxy response json");

    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert_eq!(
        response_json["output"][0]["content"][0]["text"].as_str(),
        Some("from codex fallback")
    );
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(primary_requests[0].path, RESPONSES_PATH);
    assert_eq!(fallback_requests.len(), 1);
    assert_eq!(fallback_requests[0].path, CODEX_RESPONSES_PATH);
    assert_eq!(fallback_requests[0].body["model"].as_str(), Some("gpt-5"));
    assert_eq!(
        fallback_requests[0].body["input"][0]["content"][0]["text"].as_str(),
        Some("hi")
    );
}

async fn send_responses_request(state: ProxyStateHandle) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "input": "hi"
            })
            .to_string(),
        ),
    )
    .await;

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy response bytes");
    let json = serde_json::from_slice(&body).expect("proxy response json");
    (status, json)
}

async fn send_messages_request(state: ProxyStateHandle) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static("/v1/messages"),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "claude-sonnet-4.5",
                "messages": [
                    {
                        "role": "user",
                        "content": "hi"
                    }
                ],
                "max_tokens": 64
            })
            .to_string(),
        ),
    )
    .await;

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy response bytes");
    let json = serde_json::from_slice(&body).expect("proxy response json");
    (status, json)
}

async fn wait_for_logged_account_id(pool: &sqlx::SqlitePool) -> Option<String> {
    for _ in 0..50 {
        let row = sqlx::query(
            "SELECT account_id FROM request_logs ORDER BY id DESC LIMIT 1;",
        )
        .fetch_optional(pool)
        .await
        .expect("query request logs");
        if let Some(row) = row {
            return row.try_get::<Option<String>, _>("account_id").ok().flatten();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

#[test]
fn responses_request_auto_selects_first_available_codex_account_when_unbound() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_auto_codex",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from auto codex" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-auto",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("responses_codex_auto_select");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(&state, "codex-z.json", "codex-access-z", "chatgpt-z", &expires_at).await;
        seed_codex_account(&state, "codex-a.json", "codex-access-a", "chatgpt-a", &expires_at).await;

        let (status, json) = send_responses_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from auto codex")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer codex-access-a"));
        assert_eq!(requests[0].chatgpt_account_id.as_deref(), Some("chatgpt-a"));
    });
}

#[test]
fn responses_request_failovers_to_next_codex_account_after_invalidated_token() {
    run_async(async {
        let codex = spawn_auth_switch_mock_upstream().await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-auto-failover",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("responses_codex_account_failover");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(&state, "codex-a.json", "codex-access-a", "chatgpt-a", &expires_at)
            .await;
        seed_codex_account(&state, "codex-b.json", "codex-access-b", "chatgpt-b", &expires_at)
            .await;

        let (status, json) = send_responses_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex failover")
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer codex-access-a"));
        assert_eq!(requests[0].chatgpt_account_id.as_deref(), Some("chatgpt-a"));
        assert_eq!(requests[1].authorization.as_deref(), Some("Bearer codex-access-b"));
        assert_eq!(requests[1].chatgpt_account_id.as_deref(), Some("chatgpt-b"));
    });
}

#[test]
fn responses_request_failovers_to_next_codex_account_after_proxy_error() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_codex_proxy_failover",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from codex proxy failover" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-auto-proxy-failover",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("responses_codex_account_proxy_failover");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(&state, "codex-a.json", "codex-access-a", "chatgpt-a", &expires_at)
            .await;
        seed_codex_account(&state, "codex-b.json", "codex-access-b", "chatgpt-b", &expires_at)
            .await;
        {
            let state_guard = state.read().await;
            state_guard
                .codex_accounts
                .set_proxy_url("codex-a.json", Some("http://127.0.0.1:9"))
                .await
                .expect("set broken proxy");
        }

        let (status, json) = send_responses_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex proxy failover")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer codex-access-b"));
        assert_eq!(requests[0].chatgpt_account_id.as_deref(), Some("chatgpt-b"));
    });
}

#[test]
fn responses_request_cooldowns_same_codex_account_after_401() {
    run_async(async {
        let codex =
            spawn_auth_switch_mock_upstream_with_primary_status(StatusCode::UNAUTHORIZED).await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-account-cooldown-401",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_codex_account_cooldown_401");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(&state, "codex-a.json", "codex-access-a", "chatgpt-a", &expires_at)
            .await;
        seed_codex_account(&state, "codex-b.json", "codex-access-b", "chatgpt-b", &expires_at)
            .await;

        let (first_status, first_json) = send_responses_request(state.clone()).await;
        let (second_status, second_json) = send_responses_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            first_json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex failover")
        );
        assert_eq!(
            second_json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex failover")
        );
        assert_eq!(
            requests.len(),
            3,
            "401 should temporarily cool down the failed account across requests"
        );
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer codex-access-a"));
        assert_eq!(requests[1].authorization.as_deref(), Some("Bearer codex-access-b"));
        assert_eq!(requests[2].authorization.as_deref(), Some("Bearer codex-access-b"));
    });
}

#[test]
fn responses_request_does_not_cooldown_same_codex_account_after_400() {
    run_async(async {
        let codex = spawn_auth_switch_mock_upstream_with_primary_status(StatusCode::BAD_REQUEST)
            .await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-account-cooldown-400",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_codex_account_no_cooldown_400");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(&state, "codex-a.json", "codex-access-a", "chatgpt-a", &expires_at)
            .await;
        seed_codex_account(&state, "codex-b.json", "codex-access-b", "chatgpt-b", &expires_at)
            .await;

        let (first_status, first_json) = send_responses_request(state.clone()).await;
        let (second_status, second_json) = send_responses_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            first_json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex failover")
        );
        assert_eq!(
            second_json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex failover")
        );
        assert_eq!(
            requests.len(),
            4,
            "400 should remain same-request retryable, but must not cool down the account across requests"
        );
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer codex-access-a"));
        assert_eq!(requests[1].authorization.as_deref(), Some("Bearer codex-access-b"));
        assert_eq!(requests[2].authorization.as_deref(), Some("Bearer codex-access-a"));
        assert_eq!(requests[3].authorization.as_deref(), Some("Bearer codex-access-b"));
    });
}

#[test]
fn messages_request_failovers_to_next_kiro_account_before_next_upstream() {
    run_async(async {
        let kiro = spawn_kiro_auth_switch_mock_upstream(StatusCode::FORBIDDEN).await;
        let fallback = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "msg_fallback_upstream",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4.5",
                "content": [
                    { "type": "text", "text": "from fallback upstream" }
                ],
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2
                }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_KIRO,
                0,
                "kiro-auto-failover",
                kiro.base_url.as_str(),
                FORMATS_MESSAGES,
            ),
            (
                PROVIDER_KIRO,
                0,
                "kiro-fallback-upstream",
                fallback.base_url.as_str(),
                FORMATS_MESSAGES,
            ),
        ]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_KIRO)
            .expect("kiro upstreams");
        provider_upstreams.groups[0].items[0].kiro_account_id = None;
        provider_upstreams.groups[0].items[1].kiro_account_id = None;
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };

        let data_dir = next_test_data_dir("messages_kiro_account_failover");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_kiro_account(&state, "kiro-a.json", "kiro-access-a", &expires_at).await;
        seed_kiro_account(&state, "kiro-b.json", "kiro-access-b", &expires_at).await;

        let (status, json) = send_messages_request(state).await;
        let kiro_requests = kiro.requests();
        let fallback_requests = fallback.requests();

        kiro.abort();
        fallback.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["content"][0]["text"].as_str(), Some("from kiro failover"));
        assert_eq!(kiro_requests.len(), 2);
        assert_eq!(kiro_requests[0].authorization.as_deref(), Some("Bearer kiro-access-a"));
        assert_eq!(kiro_requests[1].authorization.as_deref(), Some("Bearer kiro-access-b"));
        assert!(
            fallback_requests.is_empty(),
            "kiro should exhaust same-upstream accounts before falling back to next upstream"
        );
    });
}

#[test]
fn messages_request_falls_back_to_next_kiro_upstream_after_all_kiro_accounts_fail() {
    run_async(async {
        let kiro = spawn_mock_upstream(
            StatusCode::FORBIDDEN,
            json!({
                "error": {
                    "message": "all kiro accounts failed with forbidden"
                }
            }),
        )
        .await;
        let fallback = spawn_mock_raw_upstream(
            StatusCode::OK,
            build_kiro_event_stream("from downstream fallback"),
            "application/vnd.amazon.eventstream",
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_KIRO,
                0,
                "kiro-all-accounts-fail",
                kiro.base_url.as_str(),
                FORMATS_MESSAGES,
            ),
            (
                PROVIDER_KIRO,
                0,
                "kiro-fallback-upstream",
                fallback.base_url.as_str(),
                FORMATS_MESSAGES,
            ),
        ]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_KIRO)
            .expect("kiro upstreams");
        provider_upstreams.groups[0].items[0].kiro_account_id = None;
        provider_upstreams.groups[0].items[1].kiro_account_id = None;
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };

        let data_dir = next_test_data_dir("messages_kiro_accounts_then_upstream");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_kiro_account(&state, "kiro-a.json", "kiro-access-a", &expires_at).await;
        seed_kiro_account(&state, "kiro-b.json", "kiro-access-b", &expires_at).await;

        let (status, json) = send_messages_request(state).await;
        let kiro_requests = kiro.requests();
        let fallback_requests = fallback.requests();

        kiro.abort();
        fallback.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["content"][0]["text"].as_str(),
            Some("from downstream fallback")
        );
        assert_eq!(kiro_requests.len(), 2);
        assert_eq!(kiro_requests[0].authorization.as_deref(), Some("Bearer kiro-access-a"));
        assert_eq!(kiro_requests[1].authorization.as_deref(), Some("Bearer kiro-access-b"));
        assert_eq!(
            fallback_requests.len(),
            1,
            "next upstream should only run after same-upstream kiro accounts are exhausted"
        );
    });
}

#[test]
fn responses_request_logs_selected_codex_account_id() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_codex_logged_account",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from codex logged account" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-auto-logged-account",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("responses_codex_logged_account");
        let (state, pool) = build_test_state_handle_with_sqlite_log(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(&state, "codex-a.json", "codex-access-a", "chatgpt-a", &expires_at)
            .await;

        let (status, json) = send_responses_request(state).await;
        let logged_account_id = wait_for_logged_account_id(&pool).await;

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex logged account")
        );
        assert_eq!(logged_account_id.as_deref(), Some("codex-a.json"));
    });
}

async fn send_anthropic_messages_request(
    state: ProxyStateHandle,
    stream: bool,
) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static("/v1/messages"),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 64,
                "stream": stream,
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            { "type": "text", "text": "hi from claude" }
                        ]
                    }
                ]
            })
            .to_string(),
        ),
    )
    .await;

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy response bytes");
    let json = serde_json::from_slice(&body).expect("proxy response json");
    (status, json)
}

#[test]
fn responses_request_uses_chat_compat_for_coding_plan_runtime_upstream() {
    run_async(async {
        let coding_plan = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "created": 123,
                "model": "glm-4.7",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "from coding plan"
                        },
                        "finish_reason": "stop"
                    }
                ],
                "usage": {
                    "prompt_tokens": 2,
                    "completion_tokens": 3,
                    "total_tokens": 5
                }
            }),
        )
        .await;

        let coding_plan_base_url = format!("{}/api/coding/paas/v4", coding_plan.base_url);
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CHAT,
            10,
            "bigmodel-coding-plan",
            coding_plan_base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("responses_coding_plan_chat_compat_runtime");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static(RESPONSES_PATH),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "glm-4.7",
                    "input": "hi"
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_json: Value =
            serde_json::from_slice(&response_bytes).expect("proxy response json");
        let requests = coding_plan.requests();

        coding_plan.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["output"][0]["content"][0]["text"].as_str(),
            Some("from coding plan")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/coding/paas/v4/chat/completions");
        assert_eq!(
            requests[0].body["messages"][0]["role"].as_str(),
            Some("user")
        );
        assert_eq!(
            requests[0].body["messages"][0]["content"].as_str(),
            Some("hi")
        );
        assert!(requests[0].body.get("input").is_none());
    });
}

#[test]
fn responses_request_falls_back_from_400_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::BAD_REQUEST,
    ));
}

#[test]
fn responses_request_falls_back_from_403_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::FORBIDDEN,
    ));
}

#[test]
fn responses_request_falls_back_from_401_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::UNAUTHORIZED,
    ));
}

#[test]
fn responses_request_falls_back_from_404_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::NOT_FOUND,
    ));
}

#[test]
fn responses_request_falls_back_from_408_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::REQUEST_TIMEOUT,
    ));
}

#[test]
fn responses_request_falls_back_from_422_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::UNPROCESSABLE_ENTITY,
    ));
}

#[test]
fn responses_request_falls_back_from_504_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::GATEWAY_TIMEOUT,
    ));
}

#[test]
fn responses_request_falls_back_from_524_to_codex() {
    run_async(assert_responses_retry_fallback_status(
        StatusCode::from_u16(524).expect("524"),
    ));
}

#[test]
fn anthropic_messages_request_routes_to_codex() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_codex",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from codex for claude" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-primary",
            codex.base_url.as_str(),
            FORMATS_ALL,
        )]);
        let data_dir = next_test_data_dir("anthropic_messages_codex_direct");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (status, json) = send_anthropic_messages_request(state, false).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["type"], json!("message"));
        assert_eq!(json["role"], json!("assistant"));
        assert_eq!(json["content"][0]["type"], json!("text"));
        assert_eq!(json["content"][0]["text"], json!("from codex for claude"));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, CODEX_RESPONSES_PATH);
        assert_eq!(requests[0].body["input"][0]["role"].as_str(), Some("user"));
        assert_eq!(
            requests[0].body["input"][0]["content"][0]["type"].as_str(),
            Some("input_text")
        );
        assert_eq!(
            requests[0].body["input"][0]["content"][0]["text"].as_str(),
            Some("hi from claude")
        );
    });
}

#[test]
fn anthropic_messages_request_falls_back_from_responses_to_codex() {
    run_async(async {
        let responses = spawn_mock_upstream(
            StatusCode::BAD_REQUEST,
            json!({
                "error": { "message": "responses upstream rejected request" }
            }),
        )
        .await;
        let codex = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_codex",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "fallback from codex for claude" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                responses.base_url.as_str(),
                FORMATS_ALL,
            ),
            (
                PROVIDER_CODEX,
                5,
                "codex-fallback",
                codex.base_url.as_str(),
                FORMATS_ALL,
            ),
        ]);
        let data_dir = next_test_data_dir("anthropic_messages_responses_to_codex_fallback");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (status, json) = send_anthropic_messages_request(state, false).await;
        let responses_requests = responses.requests();
        let codex_requests = codex.requests();

        responses.abort();
        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["content"][0]["text"].as_str(),
            Some("fallback from codex for claude")
        );
        assert_eq!(responses_requests.len(), 1);
        assert_eq!(responses_requests[0].path, RESPONSES_PATH);
        assert_eq!(codex_requests.len(), 1);
        assert_eq!(codex_requests[0].path, CODEX_RESPONSES_PATH);
    });
}

#[test]
fn responses_request_skips_recently_failed_same_provider_upstream() {
    run_async(async {
        let primary = spawn_mock_upstream(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": { "message": "primary unavailable" }
            }),
        )
        .await;
        let secondary = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_secondary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from secondary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "responses-secondary",
                secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };

        let data_dir = next_test_data_dir("responses_same_provider_cooldown");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (first_status, first_json) = send_responses_request(state.clone()).await;
        let (second_status, second_json) = send_responses_request(state).await;

        let primary_requests = primary.requests();
        let secondary_requests = secondary.requests();

        primary.abort();
        secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            first_json["output"][0]["content"][0]["text"].as_str(),
            Some("from secondary")
        );
        assert_eq!(
            second_json["output"][0]["content"][0]["text"].as_str(),
            Some("from secondary")
        );
        assert_eq!(
            primary_requests.len(),
            1,
            "primary upstream should be cooled down after the first retryable failure"
        );
        assert_eq!(secondary_requests.len(), 2);
    });
}

#[test]
fn responses_request_cooldowns_same_provider_upstream_after_401() {
    run_async(async {
        let primary = spawn_mock_upstream(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": { "message": "primary unauthorized" }
            }),
        )
        .await;
        let secondary = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_secondary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from secondary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "responses-secondary",
                secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };

        let data_dir = next_test_data_dir("responses_same_provider_cooldown_401");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (first_status, first_json) = send_responses_request(state.clone()).await;
        let (second_status, second_json) = send_responses_request(state).await;

        let primary_requests = primary.requests();
        let secondary_requests = secondary.requests();

        primary.abort();
        secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            first_json["output"][0]["content"][0]["text"].as_str(),
            Some("from secondary")
        );
        assert_eq!(
            second_json["output"][0]["content"][0]["text"].as_str(),
            Some("from secondary")
        );
        assert_eq!(
            primary_requests.len(),
            1,
            "401 should cool down the upstream to avoid repeatedly hitting the same invalid account"
        );
        assert_eq!(secondary_requests.len(), 2);
    });
}

#[test]
fn responses_request_does_not_cooldown_same_provider_upstream_after_400() {
    run_async(async {
        let primary = spawn_mock_upstream(
            StatusCode::BAD_REQUEST,
            json!({
                "error": { "message": "primary bad request" }
            }),
        )
        .await;
        let secondary = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_secondary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from secondary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "responses-secondary",
                secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };

        let data_dir = next_test_data_dir("responses_same_provider_no_cooldown_400");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (first_status, first_json) = send_responses_request(state.clone()).await;
        let (second_status, second_json) = send_responses_request(state).await;

        let primary_requests = primary.requests();
        let secondary_requests = secondary.requests();

        primary.abort();
        secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            first_json["output"][0]["content"][0]["text"].as_str(),
            Some("from secondary")
        );
        assert_eq!(
            second_json["output"][0]["content"][0]["text"].as_str(),
            Some("from secondary")
        );
        assert_eq!(
            primary_requests.len(),
            2,
            "400 should stay retryable for same-request fallback, but must not cool down the upstream"
        );
        assert_eq!(secondary_requests.len(), 2);
    });
}

#[test]
fn responses_request_reload_resets_existing_cooldown_and_applies_new_duration() {
    run_async(async {
        let primary = spawn_mock_upstream(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": { "message": "primary unauthorized" }
            }),
        )
        .await;
        let secondary = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_from_secondary",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from secondary" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "responses-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "responses-secondary",
                secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_same_provider_reload_resets_cooldown");
        let state = build_test_state_handle(config.clone(), data_dir.clone()).await;

        let _ = send_responses_request(state.clone()).await;
        let _ = send_responses_request(state.clone()).await;

        let primary_requests_before_reload = primary.requests();
        assert_eq!(
            primary_requests_before_reload.len(),
            1,
            "pre-reload second request should skip cooled-down upstream"
        );

        let mut reloaded_config = config;
        reloaded_config.retryable_failure_cooldown = std::time::Duration::ZERO;
        let reloaded_state_handle =
            build_test_state_handle(reloaded_config, data_dir.clone()).await;
        let reloaded_state = {
            let guard = reloaded_state_handle.read().await;
            guard.clone()
        };
        {
            let mut guard = state.write().await;
            *guard = reloaded_state;
        }

        let _ = send_responses_request(state.clone()).await;
        let _ = send_responses_request(state).await;

        let primary_requests = primary.requests();
        let secondary_requests = secondary.requests();

        primary.abort();
        secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(
            primary_requests.len(),
            3,
            "reload should clear old cooldowns, and zero cooldown should allow primary to be retried on every later request"
        );
        assert_eq!(secondary_requests.len(), 4);
    });
}

#[test]
fn chat_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_RESPONSES)]);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, CHAT_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToResponses);
    assert_eq!(plan.response_transform, FormatTransform::ResponsesToChat);
}

#[test]
fn chat_does_not_route_to_kiro() {
    let config = config_with_providers(&[(PROVIDER_KIRO, FORMATS_ALL)]);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn responses_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_CHAT)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.outbound_path, Some(CHAT_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToChat);
    assert_eq!(plan.response_transform, FormatTransform::ChatToResponses);
}

#[test]
fn responses_does_not_route_to_kiro() {
    let config = config_with_providers(&[(PROVIDER_KIRO, FORMATS_ALL)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn chat_to_codex_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, CHAT_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToChat);
}

#[test]
fn responses_prefers_codex_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToResponses);
}

#[test]
fn retry_fallback_plan_switches_responses_to_codex() {
    let config = config_with_providers(&[
        (PROVIDER_RESPONSES, FORMATS_RESPONSES),
        (PROVIDER_CODEX, FORMATS_RESPONSES),
    ]);
    let plan = resolve_retry_fallback_plan(&config, RESPONSES_PATH, PROVIDER_RESPONSES)
        .expect("should fallback to codex");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToResponses);
}

#[test]
fn retry_fallback_plan_switches_codex_to_responses() {
    let config = config_with_providers(&[
        (PROVIDER_RESPONSES, FORMATS_RESPONSES),
        (PROVIDER_CODEX, FORMATS_RESPONSES),
    ]);
    let plan = resolve_retry_fallback_plan(&config, RESPONSES_PATH, PROVIDER_CODEX)
        .expect("should fallback to openai responses");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn retry_fallback_plan_switches_chat_between_responses_family_providers() {
    let config = config_with_providers(&[
        (PROVIDER_RESPONSES, FORMATS_ALL),
        (PROVIDER_CODEX, FORMATS_ALL),
    ]);
    let plan = resolve_retry_fallback_plan(&config, CHAT_PATH, PROVIDER_RESPONSES)
        .expect("should fallback to codex");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToChat);
}

#[test]
fn retry_fallback_plan_keeps_messages_pairing() {
    let config = config_with_providers(&[
        (PROVIDER_ANTHROPIC, FORMATS_MESSAGES),
        (PROVIDER_KIRO, FORMATS_KIRO_NATIVE),
    ]);
    let plan = resolve_retry_fallback_plan(&config, "/v1/messages", PROVIDER_ANTHROPIC)
        .expect("should fallback to kiro");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn responses_same_protocol_preferred_over_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_RESPONSES, 0, "resp", FORMATS_RESPONSES),
        (PROVIDER_CHAT, 10, "chat", FORMATS_ALL),
    ]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn responses_same_protocol_tiebreaks_by_id() {
    let config = config_with_upstreams(&[
        (PROVIDER_RESPONSES, 5, "b-resp", FORMATS_RESPONSES),
        (PROVIDER_KIRO, 5, "a-kiro", FORMATS_KIRO_NATIVE),
    ]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn anthropic_messages_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_RESPONSES)]);
    let error = resolve_dispatch_plan(&config, "/v1/messages")
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(
        plan.request_transform,
        FormatTransform::AnthropicToResponses
    );
    assert_eq!(
        plan.response_transform,
        FormatTransform::ResponsesToAnthropic
    );
}

#[test]
fn anthropic_messages_fallbacks_to_kiro_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_KIRO, FORMATS_KIRO_NATIVE)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn anthropic_beta_query_is_not_forwarded_to_responses_fallback() {
    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    let outbound = resolve_outbound_path(
        "/v1/messages",
        &plan,
        &RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        },
    );
    let uri = Uri::from_static("/v1/messages?beta=true");
    let outbound_with_query = build_outbound_path_with_query(&outbound, &uri);
    assert_eq!(outbound_with_query, RESPONSES_PATH);
}

#[test]
fn anthropic_beta_query_is_preserved_for_native_anthropic() {
    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_MESSAGES)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should dispatch");
    let outbound = resolve_outbound_path(
        "/v1/messages",
        &plan,
        &RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        },
    );
    let uri = Uri::from_static("/v1/messages?beta=true");
    let outbound_with_query = build_outbound_path_with_query(&outbound, &uri);
    assert_eq!(outbound_with_query, "/v1/messages?beta=true");
}

#[test]
fn anthropic_messages_prefers_kiro_without_conversion() {
    let config = config_with_upstreams(&[
        (PROVIDER_RESPONSES, 10, "resp", FORMATS_ALL),
        (PROVIDER_KIRO, 0, "kiro", FORMATS_KIRO_NATIVE),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn anthropic_messages_prefers_anthropic_when_priority_higher() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 5, "anthro", FORMATS_MESSAGES),
        (PROVIDER_KIRO, 1, "kiro", FORMATS_KIRO_NATIVE),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn anthropic_messages_tiebreaks_by_id_between_anthropic_and_kiro() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 5, "b-anthro", FORMATS_MESSAGES),
        (PROVIDER_KIRO, 5, "a-kiro", FORMATS_KIRO_NATIVE),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn responses_fallback_to_anthropic_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_MESSAGES)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
    assert_eq!(plan.outbound_path, Some("/v1/messages"));
    assert_eq!(
        plan.request_transform,
        FormatTransform::ResponsesToAnthropic
    );
    assert_eq!(
        plan.response_transform,
        FormatTransform::AnthropicToResponses
    );
}

#[test]
fn gemini_route_requires_format_conversion_for_fallback() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_CHAT)]);
    let error = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn gemini_route_fallbacks_to_chat() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.outbound_path, Some(CHAT_PATH));
    assert_eq!(plan.request_transform, FormatTransform::GeminiToChat);
    assert_eq!(plan.response_transform, FormatTransform::ChatToGemini);
}

#[test]
fn gemini_route_fallbacks_to_anthropic() {
    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
    assert_eq!(plan.outbound_path, Some("/v1/messages"));
    assert_eq!(plan.request_transform, FormatTransform::GeminiToAnthropic);
    assert_eq!(plan.response_transform, FormatTransform::AnthropicToGemini);
}

#[test]
fn anthropic_messages_fallbacks_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::AnthropicToGemini);
    assert_eq!(plan.response_transform, FormatTransform::GeminiToAnthropic);
}

#[test]
fn gemini_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn openai_models_route_prefers_openai_compatible_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/models").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn openai_model_detail_route_prefers_openai_compatible_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_RESPONSES, 0, "responses", FORMATS_RESPONSES),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/models/gpt-5").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn gemini_models_index_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn gemini_model_detail_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan =
        resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn openai_models_route_with_anthropic_headers_dispatches_to_anthropic() {
    let config = config_with_upstreams(&[
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
        (PROVIDER_ANTHROPIC, 0, "anthropic", FORMATS_MESSAGES),
    ]);
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert("x-api-key", HeaderValue::from_static("anthropic-key"));
    let plan = resolve_dispatch_plan_with_request(&config, "/v1/models", &headers, None)
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
}

#[test]
fn openai_models_route_with_anthropic_authorization_dispatches_to_anthropic() {
    let config = config_with_upstreams(&[
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
        (PROVIDER_ANTHROPIC, 0, "anthropic", FORMATS_MESSAGES),
    ]);
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer anthropic-key"),
    );
    let plan = resolve_dispatch_plan_with_request(&config, "/v1/models", &headers, None)
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
}

#[test]
fn openai_models_route_with_explicit_anthropic_api_key_dispatches_to_anthropic() {
    let config = config_with_upstreams(&[
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
        (PROVIDER_ANTHROPIC, 0, "anthropic", FORMATS_MESSAGES),
    ]);
    let mut headers = HeaderMap::new();
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert(
        "x-anthropic-api-key",
        HeaderValue::from_static("anthropic-key"),
    );
    let plan = resolve_dispatch_plan_with_request(&config, "/v1/models", &headers, None)
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
}

#[test]
fn openai_models_route_with_gemini_query_dispatches_to_gemini_and_rewrites_path() {
    let config = config_with_upstreams(&[
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
        (PROVIDER_GEMINI, 0, "gemini", FORMATS_GEMINI),
    ]);
    let headers = HeaderMap::new();
    let plan =
        resolve_dispatch_plan_with_request(&config, "/v1/models", &headers, Some("key=test"))
            .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    let outbound = resolve_outbound_path(
        "/v1/models",
        &plan,
        &RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound, "/v1beta/models");
}

#[test]
fn openai_model_detail_route_with_gemini_header_rewrites_to_gemini_model_detail() {
    let config = config_with_upstreams(&[
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
        (PROVIDER_GEMINI, 0, "gemini", FORMATS_GEMINI),
    ]);
    let mut headers = HeaderMap::new();
    headers.insert("x-goog-api-key", HeaderValue::from_static("gemini-key"));
    let plan =
        resolve_dispatch_plan_with_request(&config, "/v1/models/gemini-1.5-flash", &headers, None)
            .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    let outbound = resolve_outbound_path(
        "/v1/models/gemini-1.5-flash",
        &plan,
        &RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound, "/v1beta/models/gemini-1.5-flash");
}

#[test]
fn openai_compatible_models_index_route_prefers_openai_provider_and_rewrites_path() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_RESPONSES, 0, "responses", FORMATS_RESPONSES),
    ]);
    let headers = HeaderMap::new();
    let plan = resolve_dispatch_plan_with_request(&config, "/v1beta/openai/models", &headers, None)
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    let outbound = resolve_outbound_path(
        "/v1beta/openai/models",
        &plan,
        &RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound, "/v1/models");
}

#[test]
fn openai_compatible_model_detail_route_rewrites_to_openai_models_detail() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let headers = HeaderMap::new();
    let plan =
        resolve_dispatch_plan_with_request(&config, "/v1beta/openai/models/gpt-5", &headers, None)
            .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    let outbound = resolve_outbound_path(
        "/v1beta/openai/models/gpt-5",
        &plan,
        &RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound, "/v1/models/gpt-5");
}
