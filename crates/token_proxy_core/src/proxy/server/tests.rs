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
const RESPONSES_COMPACT_PATH: &str = "/v1/responses/compact";

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
            api_key_headers: None,
            filter_prompt_cache_retention: false,
            filter_safety_identifier: false,
            rewrite_developer_role_to_system: false,
            kiro_account_id: None,
            codex_account_id: (*provider == PROVIDER_CODEX).then(|| format!("codex-{id}.json")),
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: *priority,
            advertised_model_ids: Vec::new(),
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
        cors_enabled: false,
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 20 * 1024 * 1024,
        retryable_failure_cooldown: std::time::Duration::from_secs(15),
        stream_first_output_timeout: std::time::Duration::from_secs(60),
        sync_response_timeout: std::time::Duration::from_secs(120),
        upstream_strategy: UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::RoundRobin,
            dispatch: UpstreamDispatchRuntime::Serial,
        },
        codex_session_scoped_cooldown_enabled: false,
        hot_model_mappings: HashMap::new(),
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

#[derive(Clone)]
struct MockRawSequenceResponse {
    status: StatusCode,
    body: Bytes,
    content_type: String,
}

#[derive(Clone)]
struct MockRawSequenceUpstreamState {
    responses: Arc<Mutex<Vec<MockRawSequenceResponse>>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

#[derive(Clone)]
struct MockDelayedBodyUpstreamState {
    status: StatusCode,
    body: Bytes,
    content_type: String,
    body_delay_ms: u64,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

#[derive(Clone)]
struct MockModelCatalogState {
    body: Value,
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

#[derive(Clone, Debug)]
struct RecordedMultipartRequest {
    path: String,
    body: Bytes,
    authorization: Option<String>,
    content_type: Option<String>,
}

#[derive(Clone)]
struct MultipartProbeState {
    response_body: Value,
    requests: Arc<Mutex<Vec<RecordedMultipartRequest>>>,
}

struct MultipartProbeUpstream {
    base_url: String,
    requests: Arc<Mutex<Vec<RecordedMultipartRequest>>>,
    task: JoinHandle<()>,
}

impl MultipartProbeUpstream {
    fn requests(&self) -> Vec<RecordedMultipartRequest> {
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
struct MockCodexRefreshRetryState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

struct MockCodexEmptyChatSwitchState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
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
        .header(
            axum::http::header::CONTENT_TYPE,
            state.content_type.as_str(),
        )
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

async fn mock_raw_sequence_upstream_handler(
    State(state): State<Arc<MockRawSequenceUpstreamState>>,
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
    let response = {
        let mut responses = state.responses.lock().expect("responses lock");
        if responses.len() > 1 {
            responses.remove(0)
        } else {
            responses
                .first()
                .cloned()
                .expect("sequence mock must keep fallback response")
        }
    };
    axum::response::Response::builder()
        .status(response.status)
        .header(
            axum::http::header::CONTENT_TYPE,
            response.content_type.as_str(),
        )
        .body(Body::from(response.body))
        .expect("build raw sequence mock response")
}

async fn spawn_mock_raw_sequence_upstream(
    responses: Vec<(StatusCode, Bytes, &'static str)>,
) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let responses = responses
        .into_iter()
        .map(|(status, body, content_type)| MockRawSequenceResponse {
            status,
            body,
            content_type: content_type.to_string(),
        })
        .collect::<Vec<_>>();
    let state = Arc::new(MockRawSequenceUpstreamState {
        responses: Arc::new(Mutex::new(responses)),
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(mock_raw_sequence_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw sequence mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("raw sequence mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn multipart_probe_upstream_handler(
    State(state): State<Arc<MultipartProbeState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> axum::response::Response {
    let bytes = to_bytes(body, usize::MAX)
        .await
        .expect("read multipart probe body");
    state
        .requests
        .lock()
        .expect("requests lock")
        .push(RecordedMultipartRequest {
            path: uri.path().to_string(),
            body: bytes,
            authorization: headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            content_type: headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        });
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.response_body.to_string(),
    )
        .into_response()
}

async fn spawn_multipart_probe_upstream(response_body: Value) -> MultipartProbeUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MultipartProbeState {
        response_body,
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(multipart_probe_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind multipart probe upstream");
    let addr: SocketAddr = listener.local_addr().expect("multipart probe local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("multipart probe server should run");
    });
    MultipartProbeUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn mock_delayed_body_upstream_handler(
    State(state): State<Arc<MockDelayedBodyUpstreamState>>,
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
    let response_body = state.body.clone();
    let body_delay_ms = state.body_delay_ms;
    let stream = futures_util::stream::once(async move {
        tokio::time::sleep(std::time::Duration::from_millis(body_delay_ms)).await;
        Ok::<Bytes, std::convert::Infallible>(response_body)
    });
    axum::response::Response::builder()
        .status(state.status)
        .header(
            axum::http::header::CONTENT_TYPE,
            state.content_type.as_str(),
        )
        .body(Body::from_stream(stream))
        .expect("build delayed body mock response")
}

async fn spawn_mock_upstream_with_delayed_body(
    status: StatusCode,
    body: Bytes,
    content_type: &str,
    body_delay_ms: u64,
) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockDelayedBodyUpstreamState {
        status,
        body,
        content_type: content_type.to_string(),
        body_delay_ms,
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(mock_delayed_body_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed body mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("delayed body mock upstream server should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn mock_model_catalog_handler(
    State(state): State<Arc<MockModelCatalogState>>,
    uri: Uri,
) -> axum::response::Response {
    state
        .requests
        .lock()
        .expect("requests lock")
        .push(RecordedRequest {
            path: uri.path().to_string(),
            body: Value::Null,
            authorization: None,
            chatgpt_account_id: None,
        });
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.body.to_string(),
    )
        .into_response()
}

async fn spawn_model_catalog_upstream(body: Value) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockModelCatalogState {
        body,
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(mock_model_catalog_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind model catalog mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("model catalog mock upstream server should run");
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

async fn codex_refresh_retry_upstream_handler(
    State(state): State<Arc<MockCodexRefreshRetryState>>,
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
            chatgpt_account_id,
        });

    let (status, body) = match authorization.as_deref() {
        Some("Bearer codex-access-old") => (
            StatusCode::UNAUTHORIZED,
            json!({
                "error": {
                    "message": "Your authentication token has been invalidated.",
                    "type": "invalid_request_error",
                    "code": "token_invalidated",
                    "param": null
                }
            }),
        ),
        Some("Bearer codex-access-new") => (
            StatusCode::OK,
            json!({
                "id": "resp_codex_refreshed",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_refreshed",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from refreshed codex account" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        ),
        Some("Bearer codex-access-b") => (
            StatusCode::OK,
            json!({
                "id": "resp_unexpected_failover",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5-codex",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_failover",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "unexpected failover" }
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

async fn spawn_codex_refresh_retry_mock_upstream() -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockCodexRefreshRetryState {
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(codex_refresh_retry_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind codex refresh retry mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("codex refresh retry mock upstream should run");
    });
    MockUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
}

async fn codex_empty_chat_switch_upstream_handler(
    State(state): State<Arc<MockCodexEmptyChatSwitchState>>,
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

    let body = match authorization.as_deref() {
        Some("Bearer codex-access-a") => json!({
            "id": "resp_codex_empty_chat",
            "object": "response",
            "created_at": 123,
            "model": "gpt-5-codex",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "id": "msg_empty",
                    "status": "completed",
                    "role": "assistant",
                    "content": []
                }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 0, "total_tokens": 1 }
        }),
        Some("Bearer codex-access-b") => json!({
            "id": "resp_codex_failover_after_empty",
            "object": "response",
            "created_at": 123,
            "model": "gpt-5-codex",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "id": "msg_ok",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "from codex failover after empty chat" }
                    ]
                }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 5, "total_tokens": 6 }
        }),
        _ => json!({
            "error": {
                "message": "unexpected account",
                "type": "invalid_request_error",
                "code": "token_invalidated",
                "param": null
            }
        }),
    };

    (
        StatusCode::OK,
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

async fn spawn_codex_empty_chat_switch_mock_upstream() -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockCodexEmptyChatSwitchState {
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(codex_empty_chat_switch_upstream_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind empty chat switch mock upstream");
    let addr: SocketAddr = listener.local_addr().expect("mock local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("empty chat switch mock upstream server should run");
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
                            client_id: Some(
                                crate::codex::CodexRefreshTokenClient::Codex
                                    .client_id()
                                    .to_string(),
                            ),
                            id_token: "codex-id-token".to_string(),
                            auto_refresh_enabled: true,
                            status: crate::codex::CodexAccountStatus::Active,
                            account_id: Some("chatgpt-account".to_string()),
                            user_id: None,
                            openai_device_id: None,
                            email: Some("codex@example.com".to_string()),
                            expires_at: expires_at.clone(),
                            last_refresh: None,
                            proxy_url: None,
                            priority: 0,
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
        account_selector: super::super::account_selector::AccountSelectorRuntime::new_with_cooldown(
            retryable_failure_cooldown,
        ),
        request_detail: Arc::new(super::super::request_detail::RequestDetailCapture::new(
            None,
        )),
        token_rate: super::super::token_rate::TokenRateTracker::new(),
        model_discovery: Arc::new(
            super::super::model_discovery::UpstreamModelDiscoveryCache::new(),
        ),
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
    seed_codex_account_with_refresh_token(
        state,
        storage_account_id,
        access_token,
        "codex-refresh-token",
        chatgpt_account_id,
        expires_at,
    )
    .await;
}

async fn seed_codex_account_with_device_id(
    state: &ProxyStateHandle,
    storage_account_id: &str,
    access_token: &str,
    chatgpt_account_id: &str,
    openai_device_id: &str,
    expires_at: &str,
) {
    seed_codex_account_with_options(
        state,
        storage_account_id,
        access_token,
        "codex-refresh-token",
        chatgpt_account_id,
        Some(openai_device_id),
        expires_at,
    )
    .await;
}

async fn seed_codex_account_with_refresh_token(
    state: &ProxyStateHandle,
    storage_account_id: &str,
    access_token: &str,
    refresh_token: &str,
    chatgpt_account_id: &str,
    expires_at: &str,
) {
    seed_codex_account_with_options(
        state,
        storage_account_id,
        access_token,
        refresh_token,
        chatgpt_account_id,
        None,
        expires_at,
    )
    .await;
}

async fn seed_codex_account_with_options(
    state: &ProxyStateHandle,
    storage_account_id: &str,
    access_token: &str,
    refresh_token: &str,
    chatgpt_account_id: &str,
    openai_device_id: Option<&str>,
    expires_at: &str,
) {
    let state_guard = state.read().await;
    state_guard
        .codex_accounts
        .save_record(
            storage_account_id.to_string(),
            crate::codex::CodexTokenRecord {
                access_token: access_token.to_string(),
                refresh_token: refresh_token.to_string(),
                client_id: Some(
                    crate::codex::CodexRefreshTokenClient::Codex
                        .client_id()
                        .to_string(),
                ),
                id_token: "codex-id-token".to_string(),
                auto_refresh_enabled: true,
                status: crate::codex::CodexAccountStatus::Active,
                account_id: Some(chatgpt_account_id.to_string()),
                user_id: None,
                openai_device_id: openai_device_id.map(ToOwned::to_owned),
                email: Some(format!("{storage_account_id}@example.com")),
                expires_at: expires_at.to_string(),
                last_refresh: None,
                proxy_url: None,
                priority: 0,
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
                priority: 0,
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

async fn assert_responses_stream_retry_fallback_from_codex_prelude_error() {
    let primary = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"error\",\"error\":{\"message\":\"primary codex stream failed before first output\"}}\n\n",
        ),
        "text/event-stream",
    )
    .await;
    let fallback = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"from responses fallback\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"from responses fallback\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
    )
    .await;

    let config = config_with_runtime_upstreams(&[
        (
            PROVIDER_CODEX,
            10,
            "codex-primary-stream",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            5,
            "responses-fallback-stream",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    let data_dir = next_test_data_dir("responses_codex_stream_prelude_fallback");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "input": "hi",
                "stream": true
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_headers = response.headers().clone();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy stream response bytes");
    let response_text = String::from_utf8(response_bytes.to_vec()).expect("response text");

    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert_eq!(
        response_headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(response_text.contains("from responses fallback"));
    assert!(!response_text.contains("primary codex stream failed"));
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
    assert_eq!(fallback_requests.len(), 1);
    assert_eq!(fallback_requests[0].path, RESPONSES_PATH);
}

async fn assert_responses_stream_fallbacks_after_pre_header_timeout() {
    let primary = spawn_mock_upstream_with_delay(
        StatusCode::OK,
        json!({
            "id": "resp_too_late",
            "object": "response",
            "status": "completed",
            "output": []
        }),
        80,
    )
    .await;
    let fallback = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"from responses fallback\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"from responses fallback\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
    )
    .await;

    let mut config = config_with_runtime_upstreams(&[
        (
            PROVIDER_CODEX,
            10,
            "codex-primary-slow-headers",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            5,
            "responses-fallback-after-header-timeout",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    config.stream_first_output_timeout = std::time::Duration::from_millis(20);
    let data_dir = next_test_data_dir("responses_stream_header_timeout_fallback");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "stream": true,
                "input": "hi"
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy stream body");
    let response_text = String::from_utf8_lossy(&response_bytes);
    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert!(response_text.contains("from responses fallback"));
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(fallback_requests.len(), 1);
    assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
    assert_eq!(fallback_requests[0].path, RESPONSES_PATH);
}

async fn assert_responses_stream_fallbacks_after_attempt_first_output_deadline() {
    let primary = spawn_mock_upstream_with_delayed_body(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"primary late output\"}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
        80,
    )
    .await;
    let fallback = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"from responses fallback\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"from responses fallback\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
    )
    .await;

    let mut config = config_with_runtime_upstreams(&[
        (
            PROVIDER_CODEX,
            10,
            "codex-primary-slow-body",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            5,
            "responses-fallback-after-body-deadline",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    config.stream_first_output_timeout = std::time::Duration::from_millis(40);
    let data_dir = next_test_data_dir("responses_stream_first_output_deadline_fallback");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "stream": true,
                "input": "hi"
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy stream body");
    let response_text = String::from_utf8_lossy(&response_bytes);
    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert!(response_text.contains("from responses fallback"));
    assert!(!response_text.contains("primary late output"));
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(fallback_requests.len(), 1);
    assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
    assert_eq!(fallback_requests[0].path, RESPONSES_PATH);
}

async fn assert_responses_stream_retry_fallback_from_codex_created_then_failed_before_output() {
    let primary = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_created\",\"model\":\"gpt-5\"}}\n\n\
data: {\"type\":\"response.in_progress\",\"response\":{\"id\":\"resp_created\",\"model\":\"gpt-5\",\"status\":\"in_progress\"}}\n\n\
data: {\"type\":\"response.failed\",\"error\":{\"message\":\"primary codex stream failed after created\"}}\n\n",
        ),
        "text/event-stream",
    )
    .await;
    let fallback = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"from responses fallback\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"from responses fallback\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
    )
    .await;

    let config = config_with_runtime_upstreams(&[
        (
            PROVIDER_CODEX,
            10,
            "codex-primary-stream-created-then-failed",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            5,
            "responses-fallback-stream-created-then-failed",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    let data_dir = next_test_data_dir("responses_codex_stream_created_then_failed_fallback");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "input": "hi",
                "stream": true
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_headers = response.headers().clone();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy stream response bytes");
    let response_text = String::from_utf8(response_bytes.to_vec()).expect("response text");

    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert_eq!(
        response_headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(response_text.contains("from responses fallback"));
    assert!(!response_text.contains("primary codex stream failed after created"));
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
    assert_eq!(fallback_requests.len(), 1);
    assert_eq!(fallback_requests[0].path, RESPONSES_PATH);
}

async fn assert_responses_request_retries_same_upstream_once_after_capacity_error() {
    let primary = spawn_mock_raw_sequence_upstream(vec![
        (
            StatusCode::TOO_MANY_REQUESTS,
            Bytes::from(
                json!({
                    "error": {
                        "message": "Selected model is at capacity. Please try a different model."
                    }
                })
                .to_string(),
            ),
            "application/json",
        ),
        (
            StatusCode::OK,
            Bytes::from(
                json!({
                    "id": "resp_after_capacity_retry",
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
                                { "type": "output_text", "text": "same upstream recovered" }
                            ]
                        }
                    ],
                    "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
                })
                .to_string(),
            ),
            "application/json",
        ),
    ])
    .await;
    let mut config = config_with_runtime_upstreams(&[(
        PROVIDER_RESPONSES,
        10,
        "responses-capacity-primary",
        primary.base_url.as_str(),
        FORMATS_RESPONSES,
    )]);
    config.upstream_strategy = UpstreamStrategyRuntime {
        order: UpstreamOrderStrategy::FillFirst,
        dispatch: UpstreamDispatchRuntime::Serial,
    };
    let data_dir = next_test_data_dir("responses_capacity_same_upstream_retry");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let (status, json) = send_responses_request(state).await;
    let primary_requests = primary.requests();

    primary.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["output"][0]["content"][0]["text"].as_str(),
        Some("same upstream recovered")
    );
    assert_eq!(
        primary_requests.len(),
        2,
        "capacity response should retry the same upstream once"
    );
    for request in primary_requests {
        assert_eq!(
            request.body["model"].as_str(),
            Some("gpt-5"),
            "capacity retry must keep the caller-requested model"
        );
    }
}

async fn assert_responses_stream_does_not_fallback_after_first_codex_output() {
    let primary = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial before failure\"}\n\n\
data: {\"type\":\"error\",\"error\":{\"message\":\"primary codex stream failed after first output\"}}\n\n",
        ),
        "text/event-stream",
    )
    .await;
    let fallback = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"from responses fallback\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"from responses fallback\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
    )
    .await;

    let config = config_with_runtime_upstreams(&[
        (
            PROVIDER_CODEX,
            10,
            "codex-primary-stream-no-fallback",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            5,
            "responses-fallback-should-not-run",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    let data_dir = next_test_data_dir("responses_codex_stream_after_output_no_fallback");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "input": "hi",
                "stream": true
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_headers = response.headers().clone();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy stream response bytes");
    let response_text = String::from_utf8(response_bytes.to_vec()).expect("response text");

    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert_eq!(
        response_headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(response_text.contains("partial before failure"));
    assert!(response_text.contains("primary codex stream failed after first output"));
    assert!(!response_text.contains("from responses fallback"));
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
    assert_eq!(fallback_requests.len(), 0);
}

async fn assert_responses_stream_does_not_retry_same_upstream_after_first_output_capacity_error() {
    let primary = spawn_mock_raw_sequence_upstream(vec![
        (
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial before capacity\"}\n\n\
data: {\"type\":\"error\",\"error\":{\"message\":\"Selected model is at capacity. Please try a different model.\"}}\n\n",
            ),
            "text/event-stream",
        ),
        (
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"second attempt should not run\"}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        ),
    ])
    .await;
    let fallback = spawn_mock_raw_upstream(
        StatusCode::OK,
        Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"fallback should not run\"}\n\n\
data: [DONE]\n\n",
        ),
        "text/event-stream",
    )
    .await;

    let config = config_with_runtime_upstreams(&[
        (
            PROVIDER_CODEX,
            10,
            "codex-capacity-after-output",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            5,
            "responses-capacity-after-output-fallback",
            fallback.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    let data_dir = next_test_data_dir("responses_capacity_after_output_no_same_retry");
    let state = build_test_state_handle(config, data_dir.clone()).await;

    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "input": "hi",
                "stream": true
            })
            .to_string(),
        ),
    )
    .await;

    let response_status = response.status();
    let response_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("proxy stream response bytes");
    let response_text = String::from_utf8(response_bytes.to_vec()).expect("response text");
    let primary_requests = primary.requests();
    let fallback_requests = fallback.requests();

    primary.abort();
    fallback.abort();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(response_status, StatusCode::OK);
    assert!(response_text.contains("partial before capacity"));
    assert!(response_text.contains("Selected model is at capacity"));
    assert!(!response_text.contains("second attempt should not run"));
    assert!(!response_text.contains("fallback should not run"));
    assert_eq!(primary_requests.len(), 1);
    assert!(fallback_requests.is_empty());
}

async fn send_responses_request(state: ProxyStateHandle) -> (StatusCode, Value) {
    send_responses_request_with_headers(state, axum::http::HeaderMap::new()).await
}

async fn send_responses_request_with_session(
    state: ProxyStateHandle,
    session_id: &str,
) -> (StatusCode, Value) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "session_id",
        HeaderValue::from_str(session_id).expect("session header"),
    );
    send_responses_request_with_headers(state, headers).await
}

async fn send_responses_request_with_headers(
    state: ProxyStateHandle,
    headers: axum::http::HeaderMap,
) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        headers,
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

async fn spawn_codex_token_endpoint(
    access_token: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        State(access_token): State<&'static str>,
        body: Bytes,
    ) -> axum::response::Response {
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("grant_type=refresh_token"),
            "refresh grant missing: {body}"
        );
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json!({
                "access_token": access_token,
                "refresh_token": "codex-refresh-token-new",
                "id_token": build_codex_id_token("refreshed@example.com", "chatgpt-a"),
                "expires_in": 7200,
            })
            .to_string(),
        )
            .into_response()
    }

    let app = Router::new()
        .route("/oauth/token", axum::routing::post(handler))
        .with_state(access_token);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind codex token endpoint");
    let addr = listener.local_addr().expect("token endpoint addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("codex token endpoint should run");
    });
    (format!("http://{addr}/oauth/token"), task)
}

fn build_codex_id_token(email: &str, account_id: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let payload = json!({
        "email": email,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
        },
    });
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
    format!("header.{encoded}.signature")
}

async fn send_chat_request(state: ProxyStateHandle) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static("/v1/chat/completions"),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": "gpt-5",
                "messages": [
                    {
                        "role": "user",
                        "content": "hi"
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

async fn send_responses_request_with_model(
    state: ProxyStateHandle,
    model: &str,
) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        axum::http::HeaderMap::new(),
        Body::from(
            json!({
                "model": model,
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

async fn send_models_request(state: ProxyStateHandle) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::GET,
        Uri::from_static("/v1/models"),
        axum::http::HeaderMap::new(),
        Body::empty(),
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
fn models_index_aggregates_unique_ids_when_prefix_disabled() {
    run_async(async {
        let upstream_a = spawn_model_catalog_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5", "object": "model" },
                { "id": "gpt-4.1", "object": "model" }
            ]
        }))
        .await;
        let upstream_b = spawn_model_catalog_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5", "object": "model" },
                { "id": "o3", "object": "model" }
            ]
        }))
        .await;
        let data_dir =
            next_test_data_dir("models_index_aggregates_unique_ids_when_prefix_disabled");
        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                0,
                "alpha",
                upstream_a.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                0,
                "beta",
                upstream_b.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        let state = build_test_state_handle(config, data_dir).await;

        let (status, body) = send_models_request(state).await;
        assert_eq!(status, StatusCode::OK);
        let mut ids = body["data"]
            .as_array()
            .expect("models data")
            .iter()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["gpt-4.1", "gpt-5", "o3"]);
        let alpha_requests = upstream_a.requests();
        let beta_requests = upstream_b.requests();
        assert_eq!(alpha_requests.len(), 1);
        assert_eq!(alpha_requests[0].path, "/v1/models");
        assert_eq!(beta_requests.len(), 1);
        assert_eq!(beta_requests[0].path, "/v1/models");

        upstream_a.abort();
        upstream_b.abort();
    });
}

#[test]
fn models_index_allows_missing_local_key_when_local_auth_enabled() {
    run_async(async {
        let upstream = spawn_model_catalog_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5", "object": "model" }
            ]
        }))
        .await;
        let data_dir =
            next_test_data_dir("models_index_allows_missing_local_key_when_local_auth_enabled");
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "alpha",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        config.local_api_key = Some("local-key".to_string());
        let state = build_test_state_handle(config, data_dir).await;

        let (status, body) = send_models_request(state).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"][0]["id"].as_str(), Some("gpt-5"));
        let requests = upstream.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/models");

        upstream.abort();
    });
}

#[test]
fn models_index_adds_prefixed_entries_and_duplicate_alias_when_enabled() {
    run_async(async {
        let upstream_a = spawn_model_catalog_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5", "object": "model" },
                { "id": "gpt-4.1", "object": "model" }
            ]
        }))
        .await;
        let upstream_b = spawn_model_catalog_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5", "object": "model" },
                { "id": "o3", "object": "model" }
            ]
        }))
        .await;
        let data_dir = next_test_data_dir(
            "models_index_adds_prefixed_entries_and_duplicate_alias_when_enabled",
        );
        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                0,
                "alpha",
                upstream_a.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                0,
                "beta",
                upstream_b.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.model_list_prefix = true;
        let state = build_test_state_handle(config, data_dir).await;

        let (status, body) = send_models_request(state).await;
        assert_eq!(status, StatusCode::OK);
        let mut ids = body["data"]
            .as_array()
            .expect("models data")
            .iter()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "alpha/gpt-4.1",
                "alpha/gpt-5",
                "beta/gpt-5",
                "beta/o3",
                "gpt-5",
            ]
        );

        upstream_a.abort();
        upstream_b.abort();
    });
}

#[test]
fn models_index_includes_exact_model_mapping_keys_when_catalog_is_empty() {
    run_async(async {
        let upstream = spawn_model_catalog_upstream(json!({
            "object": "list",
            "data": []
        }))
        .await;
        let data_dir = next_test_data_dir(
            "models_index_includes_exact_model_mapping_keys_when_catalog_is_empty",
        );
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "alpha",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        config
            .upstreams
            .get_mut(PROVIDER_RESPONSES)
            .expect("responses upstreams")
            .groups[0]
            .items[0]
            .advertised_model_ids = vec!["alias-gpt-5".to_string(), "alias-gpt-4.1".to_string()];
        let state = build_test_state_handle(config, data_dir).await;

        let (status, body) = send_models_request(state).await;
        assert_eq!(status, StatusCode::OK);
        let mut ids = body["data"]
            .as_array()
            .expect("models data")
            .iter()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["alias-gpt-4.1", "alias-gpt-5"]);

        upstream.abort();
    });
}

#[test]
fn prefixed_model_routes_to_target_upstream_and_strips_prefix_before_forwarding() {
    run_async(async {
        let upstream_a = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_alpha",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [],
                "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
            }),
        )
        .await;
        let upstream_b = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_beta",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [],
                "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
            }),
        )
        .await;
        let data_dir = next_test_data_dir(
            "prefixed_model_routes_to_target_upstream_and_strips_prefix_before_forwarding",
        );
        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                0,
                "alpha",
                upstream_a.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                0,
                "beta",
                upstream_b.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        let state = build_test_state_handle(config, data_dir).await;

        let (status, body) = send_responses_request_with_model(state, "beta/gpt-5").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["model"].as_str(), Some("beta/gpt-5"));
        assert!(upstream_a.requests().is_empty());
        let beta_requests = upstream_b.requests();
        assert_eq!(beta_requests.len(), 1);
        assert_eq!(beta_requests[0].body["model"].as_str(), Some("gpt-5"));

        upstream_a.abort();
        upstream_b.abort();
    });
}

#[test]
fn prefixed_responses_reasoning_model_strips_sampling_params_before_forwarding() {
    run_async(async {
        let upstream = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_beta",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [],
                "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
            }),
        )
        .await;
        let data_dir = next_test_data_dir(
            "prefixed_responses_reasoning_model_strips_sampling_params_before_forwarding",
        );
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "beta",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static(RESPONSES_PATH),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "beta/gpt-5",
                    "input": "hi",
                    "temperature": 0.7,
                    "top_p": 0.9
                })
                .to_string(),
            ),
        )
        .await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let json: Value = serde_json::from_slice(&body).expect("proxy response json");
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["model"].as_str(), Some("beta/gpt-5"));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body["model"].as_str(), Some("gpt-5"));
        assert!(requests[0].body.get("temperature").is_none());
        assert!(requests[0].body.get("top_p").is_none());
    });
}

async fn wait_for_logged_account_id(pool: &sqlx::SqlitePool) -> Option<String> {
    for _ in 0..50 {
        let row = sqlx::query("SELECT account_id FROM request_logs ORDER BY id DESC LIMIT 1;")
            .fetch_optional(pool)
            .await
            .expect("query request logs");
        if let Some(row) = row {
            return row
                .try_get::<Option<String>, _>("account_id")
                .ok()
                .flatten();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

async fn wait_for_logged_client_ip(pool: &sqlx::SqlitePool) -> Option<String> {
    for _ in 0..50 {
        let row = sqlx::query("SELECT client_ip FROM request_logs ORDER BY id DESC LIMIT 1;")
            .fetch_optional(pool)
            .await
            .expect("query request logs");
        if let Some(row) = row {
            return row.try_get::<Option<String>, _>("client_ip").ok().flatten();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

async fn send_responses_request_through_router(
    state: ProxyStateHandle,
) -> (StatusCode, Value, JoinHandle<()>) {
    let app = build_router(state.clone(), 20 * 1024 * 1024).with_state::<()>(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy router");
    let addr = listener.local_addr().expect("proxy router addr");
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("proxy router should run");
    });
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{RESPONSES_PATH}"))
        .json(&json!({
            "model": "gpt-5",
            "input": "hi"
        }))
        .send()
        .await
        .expect("send proxy router request");
    let status = response.status();
    let json = response
        .json::<Value>()
        .await
        .expect("proxy router response json");
    (status, json, task)
}

async fn wait_for_request_log_count(pool: &sqlx::SqlitePool, expected_min: i64) -> i64 {
    for _ in 0..50 {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM request_logs;")
            .fetch_one(pool)
            .await
            .expect("count request logs");
        let count = row.try_get::<i64, _>("count").unwrap_or_default();
        if count >= expected_min {
            return count;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("request log count");
}

async fn wait_for_latest_request_log_status_and_error(
    pool: &sqlx::SqlitePool,
) -> (i64, Option<String>) {
    for _ in 0..50 {
        let row = sqlx::query(
            "SELECT status, response_error FROM request_logs ORDER BY id DESC LIMIT 1;",
        )
        .fetch_optional(pool)
        .await
        .expect("query latest request log");
        if let Some(row) = row {
            return (
                row.try_get::<i64, _>("status").unwrap_or_default(),
                row.try_get::<Option<String>, _>("response_error")
                    .ok()
                    .flatten(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("latest request log status and error");
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
        seed_codex_account(
            &state,
            "codex-z.json",
            "codex-access-z",
            "chatgpt-z",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;

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
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(requests[0].chatgpt_account_id.as_deref(), Some("chatgpt-a"));
    });
}

#[test]
fn responses_request_injects_codex_installation_id_from_selected_account() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_codex_installation",
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
                            { "type": "output_text", "text": "from codex installation" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-installation",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("responses_codex_installation");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account_with_device_id(
            &state,
            "codex-codex-installation.json",
            "codex-access-install",
            "chatgpt-install",
            "device-install-123",
            &expires_at,
        )
        .await;

        let (status, json) = send_responses_request_with_model(state, "gpt-5-codex").await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex installation")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].body["client_metadata"]["x-codex-installation-id"].as_str(),
            Some("device-install-123")
        );
    });
}

#[test]
fn responses_request_refreshes_codex_account_after_unauthorized_before_failover() {
    run_async(async {
        let codex = spawn_codex_refresh_retry_mock_upstream().await;
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-refresh-retry",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("responses_codex_refresh_retry");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        let (token_url, token_task) = spawn_codex_token_endpoint("codex-access-new").await;
        {
            let state_guard = state.read().await;
            state_guard
                .codex_accounts
                .set_test_token_url(&token_url)
                .await;
        }
        seed_codex_account_with_refresh_token(
            &state,
            "codex-a.json",
            "codex-access-old",
            "codex-refresh-token-old",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account_with_refresh_token(
            &state,
            "codex-b.json",
            "codex-access-b",
            "codex-refresh-token-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (status, json) = send_responses_request(state).await;
        let requests = codex.requests();

        token_task.abort();
        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from refreshed codex account")
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-old")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-new")
        );
    });
}

#[test]
fn responses_request_refreshes_pinned_codex_account_after_unauthorized() {
    run_async(async {
        let codex = spawn_codex_refresh_retry_mock_upstream().await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-refresh-pinned",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);

        let data_dir = next_test_data_dir("responses_codex_refresh_pinned");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        let (token_url, token_task) = spawn_codex_token_endpoint("codex-access-new").await;
        {
            let state_guard = state.read().await;
            state_guard
                .codex_accounts
                .set_test_token_url(&token_url)
                .await;
        }
        seed_codex_account_with_refresh_token(
            &state,
            "codex-codex-refresh-pinned.json",
            "codex-access-old",
            "codex-refresh-token-old",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account_with_refresh_token(
            &state,
            "codex-b.json",
            "codex-access-b",
            "codex-refresh-token-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (status, json) = send_responses_request(state).await;
        let requests = codex.requests();

        token_task.abort();
        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from refreshed codex account")
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-old")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-new")
        );
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
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
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
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(requests[0].chatgpt_account_id.as_deref(), Some("chatgpt-a"));
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
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
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
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
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
        assert_eq!(requests[0].chatgpt_account_id.as_deref(), Some("chatgpt-b"));
    });
}

#[test]
fn chat_request_failovers_to_next_codex_account_after_empty_2xx_response() {
    run_async(async {
        let codex = spawn_codex_empty_chat_switch_mock_upstream().await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-auto-empty-chat-failover",
            codex.base_url.as_str(),
            FORMATS_CHAT,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("chat_codex_account_empty_response_failover");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (status, json) = send_chat_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["choices"][0]["message"]["content"].as_str(),
            Some("from codex failover after empty chat")
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
    });
}

#[test]
fn chat_request_retries_empty_choices_event_stream_on_responses_provider() {
    run_async(async {
        let primary = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"id\":\"\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"gpt-5.5\",\"system_fingerprint\":\"\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":0,\"total_tokens\":12}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;
        let fallback = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"from responses fallback\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"from responses fallback\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;
        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CHAT,
                10,
                "airouter-chat-empty",
                primary.base_url.as_str(),
                FORMATS_CHAT,
            ),
            (
                PROVIDER_RESPONSES,
                5,
                "airouter-responses-fallback",
                fallback.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        let data_dir = next_test_data_dir("chat_empty_choices_stream_responses_fallback");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (status, json) = send_chat_request(state).await;
        let primary_requests = primary.requests();
        let fallback_requests = fallback.requests();

        primary.abort();
        fallback.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["choices"][0]["message"]["content"].as_str(),
            Some("from responses fallback")
        );
        assert_eq!(primary_requests.len(), 1);
        assert_eq!(primary_requests[0].path, "/v1/chat/completions");
        assert_eq!(fallback_requests.len(), 1);
        assert_eq!(fallback_requests[0].path, RESPONSES_PATH);
        assert_eq!(fallback_requests[0].body["model"].as_str(), Some("gpt-5"));
        assert_eq!(
            fallback_requests[0].body["input"][0]["content"][0]["text"].as_str(),
            Some("hi")
        );
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
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
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
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
        assert_eq!(
            requests[2].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
    });
}

#[test]
fn responses_codex_session_scoped_cooldown_clears_after_successful_failover() {
    run_async(async {
        let codex =
            spawn_auth_switch_mock_upstream_with_primary_status(StatusCode::UNAUTHORIZED).await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-session-cooldown-success",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;
        config.codex_session_scoped_cooldown_enabled = true;
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_codex_session_cooldown_success");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (first_status, first_json) =
            send_responses_request_with_session(state.clone(), "session-a").await;
        let (second_status, second_json) =
            send_responses_request_with_session(state, "session-a").await;
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
            "successful session turns should not keep a failed same-turn account cooling"
        );
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
        assert_eq!(
            requests[2].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[3].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
    });
}

#[test]
fn responses_codex_session_scoped_cooldown_isolates_failed_sessions() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": {
                    "message": "codex account unauthorized",
                    "type": "invalid_request_error",
                    "code": "token_invalidated"
                }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-session-cooldown-failed",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;
        config.codex_session_scoped_cooldown_enabled = true;
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_codex_session_cooldown_isolated");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (first_status, _) =
            send_responses_request_with_session(state.clone(), "session-a").await;
        let (second_status, _) = send_responses_request_with_session(state, "session-b").await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::UNAUTHORIZED);
        assert_eq!(second_status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            requests.len(),
            4,
            "session-a cooldown must not skip accounts for session-b"
        );
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
        assert_eq!(
            requests[2].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[3].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
    });
}

#[test]
fn responses_codex_session_scoped_cooldown_does_not_share_missing_session() {
    run_async(async {
        let codex = spawn_mock_upstream(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": {
                    "message": "codex account unauthorized",
                    "type": "invalid_request_error",
                    "code": "token_invalidated"
                }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-session-cooldown-missing-session",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;
        config.codex_session_scoped_cooldown_enabled = true;
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_codex_missing_session_cooldown");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (first_status, _) = send_responses_request(state.clone()).await;
        let (second_status, _) = send_responses_request(state).await;
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::UNAUTHORIZED);
        assert_eq!(second_status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            requests.len(),
            4,
            "requests without session_id must not share cooldown state"
        );
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
        assert_eq!(
            requests[2].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[3].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
    });
}

#[test]
fn responses_request_falls_back_to_responses_provider_when_all_codex_accounts_are_cooling() {
    run_async(async {
        let codex =
            spawn_auth_switch_mock_upstream_with_primary_status(StatusCode::UNAUTHORIZED).await;
        let fallback = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_fallback_after_codex_cooling",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5.4",
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "id": "msg_1",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "from responses fallback after cooling" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CODEX,
                10,
                "codex-primary",
                codex.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                0,
                "responses-fallback",
                fallback.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;
        config.retryable_failure_cooldown = std::time::Duration::from_secs(15);

        let data_dir = next_test_data_dir("responses_codex_cooling_cross_provider_fallback");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;

        let (first_status, first_json) = send_responses_request(state.clone()).await;
        let (second_status, second_json) = send_responses_request(state).await;
        let codex_requests = codex.requests();
        let fallback_requests = fallback.requests();

        codex.abort();
        fallback.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(
            first_json["output"][0]["content"][0]["text"].as_str(),
            Some("from responses fallback after cooling")
        );
        assert_eq!(
            second_json["output"][0]["content"][0]["text"].as_str(),
            Some("from responses fallback after cooling")
        );
        assert_eq!(
            codex_requests.len(),
            1,
            "第二次请求应因账号 cooling 而跳过 codex，不再命中主 upstream"
        );
        assert_eq!(fallback_requests.len(), 2);
    });
}

#[test]
fn responses_request_does_not_cooldown_same_codex_account_after_400() {
    run_async(async {
        let codex =
            spawn_auth_switch_mock_upstream_with_primary_status(StatusCode::BAD_REQUEST).await;

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
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
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
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
        assert_eq!(
            requests[2].authorization.as_deref(),
            Some("Bearer codex-access-a")
        );
        assert_eq!(
            requests[3].authorization.as_deref(),
            Some("Bearer codex-access-b")
        );
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
        assert_eq!(
            json["content"][0]["text"].as_str(),
            Some("from kiro failover")
        );
        assert_eq!(kiro_requests.len(), 2);
        assert_eq!(
            kiro_requests[0].authorization.as_deref(),
            Some("Bearer kiro-access-a")
        );
        assert_eq!(
            kiro_requests[1].authorization.as_deref(),
            Some("Bearer kiro-access-b")
        );
        assert!(
            fallback_requests.is_empty(),
            "kiro should exhaust same-upstream accounts before falling back to next upstream"
        );
    });
}

#[test]
fn messages_request_stops_after_all_kiro_accounts_fail_and_does_not_retry_next_upstream() {
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
        let next_upstream = spawn_mock_raw_upstream(
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
                next_upstream.base_url.as_str(),
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
        let next_upstream_requests = next_upstream.requests();

        kiro.abort();
        next_upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            json["error"]["message"].as_str(),
            Some("Kiro account is not configured.")
        );
        assert_eq!(kiro_requests.len(), 2);
        assert_eq!(
            kiro_requests[0].authorization.as_deref(),
            Some("Bearer kiro-access-a")
        );
        assert_eq!(
            kiro_requests[1].authorization.as_deref(),
            Some("Bearer kiro-access-b")
        );
        assert_eq!(
            next_upstream_requests.len(),
            0,
            "same provider account cooldown now blocks reusing a later kiro upstream in the same request"
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
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
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

#[test]
fn responses_request_skips_localhost_client_ip_in_request_logs() {
    run_async(async {
        let upstream = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_client_ip",
                "object": "response",
                "created_at": 123,
                "model": "gpt-5",
                "status": "completed",
                "output": [],
                "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
            }),
        )
        .await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "responses-client-ip",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("responses_client_ip");
        let (state, pool) = build_test_state_handle_with_sqlite_log(config, data_dir.clone()).await;

        let (status, json, proxy_task) = send_responses_request_through_router(state).await;
        let logged_client_ip = wait_for_logged_client_ip(&pool).await;

        proxy_task.abort();
        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"].as_str(), Some("resp_client_ip"));
        assert_eq!(logged_client_ip.as_deref(), None);
    });
}

#[test]
fn responses_request_logs_each_codex_account_failover_attempt() {
    run_async(async {
        let codex = spawn_auth_switch_mock_upstream().await;

        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "codex-account-log-all-attempts",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let provider_upstreams = config
            .upstreams
            .get_mut(PROVIDER_CODEX)
            .expect("codex upstreams");
        provider_upstreams.groups[0].items[0].codex_account_id = None;

        let data_dir = next_test_data_dir("responses_codex_logs_all_attempts");
        let (state, pool) = build_test_state_handle_with_sqlite_log(config, data_dir.clone()).await;
        let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        seed_codex_account(
            &state,
            "codex-a.json",
            "codex-access-a",
            "chatgpt-a",
            &expires_at,
        )
        .await;
        seed_codex_account(
            &state,
            "codex-b.json",
            "codex-access-b",
            "chatgpt-b",
            &expires_at,
        )
        .await;

        let (status, json) = send_responses_request(state).await;
        let logged_count = wait_for_request_log_count(&pool, 2).await;
        let logged_rows =
            sqlx::query("SELECT account_id, status FROM request_logs ORDER BY id ASC;")
                .fetch_all(&pool)
                .await
                .expect("query request logs");

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["output"][0]["content"][0]["text"].as_str(),
            Some("from codex failover")
        );
        assert_eq!(logged_count, 2);
        assert_eq!(
            logged_rows[0]
                .try_get::<Option<String>, _>("account_id")
                .ok()
                .flatten()
                .as_deref(),
            Some("codex-a.json")
        );
        assert_eq!(
            logged_rows[0]
                .try_get::<i64, _>("status")
                .unwrap_or_default(),
            401
        );
        assert_eq!(
            logged_rows[1]
                .try_get::<Option<String>, _>("account_id")
                .ok()
                .flatten()
                .as_deref(),
            Some("codex-b.json")
        );
        assert_eq!(
            logged_rows[1]
                .try_get::<i64, _>("status")
                .unwrap_or_default(),
            200
        );
    });
}

#[test]
fn responses_stream_request_falls_back_from_codex_when_first_sse_event_is_error() {
    run_async(async {
        assert_responses_stream_retry_fallback_from_codex_prelude_error().await;
    });
}

#[test]
fn responses_stream_request_falls_back_from_codex_when_created_then_failed_before_output() {
    run_async(async {
        assert_responses_stream_retry_fallback_from_codex_created_then_failed_before_output().await;
    });
}

#[test]
fn responses_request_retries_same_upstream_once_after_capacity_error() {
    run_async(async {
        assert_responses_request_retries_same_upstream_once_after_capacity_error().await;
    });
}

#[test]
fn responses_stream_request_falls_back_from_codex_when_headers_timeout_before_output() {
    run_async(async {
        assert_responses_stream_fallbacks_after_pre_header_timeout().await;
    });
}

#[test]
fn responses_stream_request_falls_back_when_first_output_deadline_expires_after_headers() {
    run_async(async {
        assert_responses_stream_fallbacks_after_attempt_first_output_deadline().await;
    });
}

#[test]
fn responses_stream_request_does_not_fallback_after_first_codex_output() {
    run_async(async {
        assert_responses_stream_does_not_fallback_after_first_codex_output().await;
    });
}

#[test]
fn responses_stream_request_does_not_retry_same_upstream_after_first_output_capacity_error() {
    run_async(async {
        assert_responses_stream_does_not_retry_same_upstream_after_first_output_capacity_error()
            .await;
    });
}

#[test]
fn responses_stream_request_emits_compatible_terminal_event_when_codex_upstream_ends_early() {
    run_async(async {
        let primary = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_early\",\"created_at\":123,\"model\":\"gpt-5-codex\"}}\n\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial output\"}\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-primary-stream-ends-early",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("responses_codex_stream_ends_early");
        let (state, pool) = build_test_state_handle_with_sqlite_log(config, data_dir.clone()).await;
        {
            let state_guard = state.read().await;
            state_guard.request_detail.arm();
        }

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static(RESPONSES_PATH),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-5",
                    "input": "hi",
                    "stream": true
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_headers = response.headers().clone();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy stream response bytes");
        let response_text = String::from_utf8(response_bytes.to_vec()).expect("response text");
        let logged_count = wait_for_request_log_count(&pool, 1).await;
        let (logged_status, logged_error) =
            wait_for_latest_request_log_status_and_error(&pool).await;
        let primary_requests = primary.requests();

        primary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        assert!(
            response_text.contains("partial output"),
            "chunks: {response_text}"
        );
        assert!(
            response_text.contains("\"type\":\"response.completed\""),
            "chunks: {response_text}"
        );
        assert!(
            response_text.contains("\"status\":\"incomplete\""),
            "chunks: {response_text}"
        );
        assert!(
            response_text.contains("\"incomplete_details\":{\"reason\":\"error\"}"),
            "chunks: {response_text}"
        );
        assert!(
            response_text.contains("data: [DONE]"),
            "chunks: {response_text}"
        );
        assert_eq!(logged_count, 1);
        assert_eq!(logged_status, 200);
        assert_eq!(
            logged_error.as_deref(),
            Some("Codex upstream stream disconnected before response.completed")
        );
        assert_eq!(primary_requests.len(), 1);
        assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
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

async fn send_anthropic_count_tokens_request(
    state: ProxyStateHandle,
    headers: HeaderMap,
) -> (StatusCode, Value) {
    send_anthropic_count_tokens_request_with_body(
        state,
        headers,
        json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "hi from claude" }
                    ]
                }
            ]
        }),
    )
    .await
}

async fn send_anthropic_count_tokens_request_with_body(
    state: ProxyStateHandle,
    headers: HeaderMap,
    body: Value,
) -> (StatusCode, Value) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static("/v1/messages/count_tokens"),
        headers,
        Body::from(body.to_string()),
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
fn responses_request_with_gpt_image_2_preserves_native_payload_for_openai_response() {
    run_async(async {
        let responses = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "resp_img_1",
                "object": "response",
                "created_at": 123,
                "model": "gpt-image-2",
                "status": "completed",
                "output": [
                    {
                        "type": "image_generation_call",
                        "id": "ig_1",
                        "result": "ZmFrZS1pbWFnZS1iNjQ="
                    }
                ]
            }),
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            10,
            "responses-gpt-image-2",
            responses.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("responses_gpt_image_2_native_passthrough");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static(RESPONSES_PATH),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "input": [
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "input_text",
                                    "text": "draw small red fox wearing sunglasses"
                                }
                            ]
                        }
                    ],
                    "tools": [
                        {
                            "type": "image_generation",
                            "size": "1024x1024",
                            "quality": "high"
                        }
                    ]
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
        let requests = responses.requests();

        responses.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(response_json["model"].as_str(), Some("gpt-image-2"));
        assert_eq!(
            response_json["output"][0]["type"].as_str(),
            Some("image_generation_call")
        );
        assert_eq!(
            response_json["output"][0]["result"].as_str(),
            Some("ZmFrZS1pbWFnZS1iNjQ=")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, RESPONSES_PATH);
        assert_eq!(requests[0].body["model"].as_str(), Some("gpt-image-2"));
        assert_eq!(
            requests[0].body["input"][0]["content"][0]["type"].as_str(),
            Some("input_text")
        );
        assert_eq!(
            requests[0].body["input"][0]["content"][0]["text"].as_str(),
            Some("draw small red fox wearing sunglasses")
        );
        assert_eq!(
            requests[0].body["tools"][0]["type"].as_str(),
            Some("image_generation")
        );
        assert_eq!(
            requests[0].body["tools"][0]["size"].as_str(),
            Some("1024x1024")
        );
        assert_eq!(
            requests[0].body["tools"][0]["quality"].as_str(),
            Some("high")
        );
    });
}

#[test]
fn openai_image_generation_falls_back_to_codex_responses_bridge() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_codex\",\"object\":\"response\",\"created_at\":1710000000,\"model\":\"gpt-5.4-mini\",\"tools\":[{\"type\":\"image_generation\",\"model\":\"gpt-image-2\",\"size\":\"1024x1024\",\"quality\":\"high\",\"output_format\":\"png\"}]}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_codex\",\"object\":\"response\",\"created_at\":1710000000,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"aGVsbG8=\",\"revised_prompt\":\"draw a cat\",\"output_format\":\"png\",\"quality\":\"high\",\"size\":\"1024x1024\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_codex_bridge");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw a cat",
                    "size": "1024x1024",
                    "quality": "high",
                    "output_format": "png",
                    "response_format": "b64_json"
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
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(response_json["created"].as_i64(), Some(1710000000));
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("aGVsbG8=")
        );
        assert_eq!(
            response_json["data"][0]["revised_prompt"].as_str(),
            Some("draw a cat")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, CODEX_RESPONSES_PATH);
        assert_eq!(requests[0].body["model"].as_str(), Some("gpt-5.4-mini"));
        assert_eq!(
            requests[0].body["input"][0]["content"][0]["text"].as_str(),
            Some("draw a cat")
        );
        assert_eq!(
            requests[0].body["tools"][0]["type"].as_str(),
            Some("image_generation")
        );
        assert_eq!(
            requests[0].body["tools"][0]["action"].as_str(),
            Some("generate")
        );
        assert_eq!(
            requests[0].body["tools"][0]["model"].as_str(),
            Some("gpt-image-2")
        );
        assert_eq!(
            requests[0].body["tools"][0]["size"].as_str(),
            Some("1024x1024")
        );
        assert_eq!(
            requests[0].body["tools"][0]["quality"].as_str(),
            Some("high")
        );
        assert_eq!(
            requests[0].body["tool_choice"]["type"].as_str(),
            Some("image_generation")
        );
    });
}

#[test]
fn openai_image_generation_retries_native_failure_to_codex_bridge() {
    run_async(async {
        let openai = spawn_mock_upstream(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": { "message": "native image upstream unavailable" }
            }),
        )
        .await;
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_retry\",\"object\":\"response\",\"created_at\":1710000004,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"cmV0cnk=\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CHAT,
                10,
                "openai-images-primary",
                openai.base_url.as_str(),
                FORMATS_CHAT,
            ),
            (
                PROVIDER_CODEX,
                0,
                "codex-images-fallback",
                codex.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        let data_dir = next_test_data_dir("openai_images_generation_codex_retry_bridge");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw after native failure"
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
        let openai_requests = openai.requests();
        let codex_requests = codex.requests();

        openai.abort();
        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("cmV0cnk=")
        );
        assert_eq!(openai_requests.len(), 1);
        assert_eq!(openai_requests[0].path, "/v1/images/generations");
        assert_eq!(codex_requests.len(), 1);
        assert_eq!(codex_requests[0].path, CODEX_RESPONSES_PATH);
        assert_eq!(
            codex_requests[0].body["input"][0]["content"][0]["text"].as_str(),
            Some("draw after native failure")
        );
        assert_eq!(
            codex_requests[0].body["tools"][0]["type"].as_str(),
            Some("image_generation")
        );
    });
}

#[test]
fn openai_image_generation_retry_skips_codex_without_responses_format() {
    run_async(async {
        let openai = spawn_mock_upstream(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": { "message": "native image upstream unavailable" }
            }),
        )
        .await;
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_unused\",\"output\":[{\"type\":\"image_generation_call\",\"result\":\"dW51c2Vk\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CHAT,
                10,
                "openai-images-primary",
                openai.base_url.as_str(),
                FORMATS_CHAT,
            ),
            (
                PROVIDER_CODEX,
                0,
                "codex-chat-only",
                codex.base_url.as_str(),
                FORMATS_CHAT,
            ),
        ]);
        let data_dir = next_test_data_dir("openai_images_generation_codex_wrong_format");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw after native failure"
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
        let openai_requests = openai.requests();
        let codex_requests = codex.requests();

        openai.abort();
        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            response_json["error"]["message"].as_str(),
            Some("native image upstream unavailable")
        );
        assert_eq!(openai_requests.len(), 1);
        assert!(codex_requests.is_empty());
    });
}

#[test]
fn openai_image_generation_trailing_slash_falls_back_to_codex_bridge() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_slash\",\"object\":\"response\",\"created_at\":1710000018,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"c2xhc2g=\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_trailing_slash");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations/"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw trailing slash"
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
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("c2xhc2g=")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, CODEX_RESPONSES_PATH);
    });
}

#[test]
fn openai_image_generation_codex_bridge_rejects_multi_image_n() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"image_generation_call\",\"result\":\"dW51c2Vk\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_reject_multi_n");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw two",
                    "n": 2
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_text = String::from_utf8_lossy(&response_bytes);
        let requests = codex.requests();

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::BAD_REQUEST);
        assert!(response_text.contains("Codex image bridge currently supports n=1 only."));
        assert!(requests.is_empty());
    });
}

#[test]
fn openai_image_generation_codex_bridge_uses_output_item_done_fallback() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_item\",\"object\":\"response\",\"created_at\":1710000010,\"model\":\"gpt-5.4-mini\",\"tools\":[{\"type\":\"image_generation\",\"model\":\"gpt-image-2\",\"output_format\":\"png\"}]}}\n\n\
data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"aXRlbQ==\",\"revised_prompt\":\"draw item fallback\",\"output_format\":\"png\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_item\",\"object\":\"response\",\"created_at\":1710000010,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_item_done_fallback");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw item fallback"
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

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("aXRlbQ==")
        );
        assert_eq!(
            response_json["data"][0]["revised_prompt"].as_str(),
            Some("draw item fallback")
        );
    });
}

#[test]
fn openai_image_generation_codex_bridge_honors_url_response_format() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_url\",\"object\":\"response\",\"created_at\":1710000011,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"dXJs\",\"output_format\":\"webp\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_url_response_format");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw url",
                    "response_format": "url"
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

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["url"].as_str(),
            Some("data:image/webp;base64,dXJs")
        );
        assert_eq!(response_json["data"][0]["b64_json"].as_str(), Some("dXJs"));
    });
}

#[test]
fn openai_image_generation_codex_bridge_preserves_usage_and_metadata() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_usage\",\"object\":\"response\",\"created_at\":1710000015,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"usage\":{\"input_tokens\":5,\"output_tokens\":9,\"total_tokens\":14,\"input_tokens_details\":{\"text_tokens\":5,\"image_tokens\":0},\"output_tokens_details\":{\"image_tokens\":9}},\"tool_usage\":{\"image_gen\":{\"images\":1}},\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"dXNhZ2U=\",\"output_format\":\"webp\",\"quality\":\"high\",\"size\":\"1024x1024\",\"background\":\"transparent\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_usage_metadata");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw usage"
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

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("dXNhZ2U=")
        );
        assert_eq!(response_json["usage"]["total_tokens"].as_i64(), Some(14));
        assert!(response_json["usage"].get("images").is_none());
        assert_eq!(response_json["output_format"].as_str(), Some("webp"));
        assert_eq!(response_json["quality"].as_str(), Some("high"));
        assert_eq!(response_json["size"].as_str(), Some("1024x1024"));
        assert_eq!(response_json["background"].as_str(), Some("transparent"));
    });
}

#[test]
fn openai_image_generation_codex_bridge_retries_incomplete_to_next_upstream() {
    run_async(async {
        let primary = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_img_incomplete\",\"object\":\"response\",\"created_at\":1710000020,\"model\":\"gpt-5.4-mini\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"output\":[]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;
        let fallback = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_after_incomplete\",\"object\":\"response\",\"created_at\":1710000021,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"ZmFpbG92ZXI=\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CODEX,
                10,
                "codex-images-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_CODEX,
                10,
                "codex-images-fallback",
                fallback.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };
        let data_dir = next_test_data_dir("openai_images_generation_incomplete_retry");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw after incomplete"
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
            response_json["data"][0]["b64_json"].as_str(),
            Some("ZmFpbG92ZXI=")
        );
        assert_eq!(primary_requests.len(), 1);
        assert_eq!(fallback_requests.len(), 1);
        assert_eq!(primary_requests[0].path, CODEX_RESPONSES_PATH);
        assert_eq!(fallback_requests[0].path, CODEX_RESPONSES_PATH);
    });
}

#[test]
fn openai_image_generation_codex_bridge_retries_empty_output_to_next_upstream() {
    run_async(async {
        let primary = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_empty_retry\",\"object\":\"response\",\"created_at\":1710000024,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;
        let fallback = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_after_empty\",\"object\":\"response\",\"created_at\":1710000025,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"ZW1wdHktZmFpbG92ZXI=\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CODEX,
                10,
                "codex-images-empty-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_CODEX,
                10,
                "codex-images-empty-fallback",
                fallback.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };
        let data_dir = next_test_data_dir("openai_images_generation_empty_output_retry");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw after empty output"
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
            response_json["data"][0]["b64_json"].as_str(),
            Some("ZW1wdHktZmFpbG92ZXI=")
        );
        assert_eq!(primary_requests.len(), 1);
        assert_eq!(fallback_requests.len(), 1);
    });
}

#[test]
fn openai_image_generation_codex_bridge_content_filter_incomplete_does_not_retry() {
    run_async(async {
        let primary = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_img_filtered\",\"object\":\"response\",\"created_at\":1710000022,\"model\":\"gpt-5.4-mini\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"content_filter\"},\"output\":[]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;
        let fallback = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_unused\",\"object\":\"response\",\"created_at\":1710000023,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"dW51c2Vk\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_CODEX,
                10,
                "codex-images-primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_CODEX,
                10,
                "codex-images-fallback",
                fallback.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };
        let data_dir = next_test_data_dir("openai_images_generation_content_filter_no_retry");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw filtered"
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_text = String::from_utf8_lossy(&response_bytes);
        let primary_requests = primary.requests();
        let fallback_requests = fallback.requests();

        primary.abort();
        fallback.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::BAD_REQUEST);
        assert!(response_text.contains("content_filter"));
        assert_eq!(primary_requests.len(), 1);
        assert!(fallback_requests.is_empty());
    });
}

#[test]
fn openai_image_generation_codex_bridge_falls_back_to_tool_usage() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_tool_usage\",\"object\":\"response\",\"created_at\":1710000017,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"tool_usage\":{\"image_gen\":{\"images\":1}},\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"dG9vbHVzYWdl\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_tool_usage");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw tool usage"
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

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(response_json["usage"]["images"].as_i64(), Some(1));
    });
}

#[test]
fn openai_image_generation_codex_bridge_merges_lifecycle_metadata() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_meta\",\"object\":\"response\",\"created_at\":1710000016,\"model\":\"gpt-5.4-mini\",\"tools\":[{\"type\":\"image_generation\",\"model\":\"gpt-image-2\",\"output_format\":\"webp\",\"quality\":\"high\",\"size\":\"1024x1024\",\"background\":\"transparent\"}]}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_meta\",\"object\":\"response\",\"created_at\":1710000016,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"bWV0YQ==\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_lifecycle_metadata");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw metadata"
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

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("bWV0YQ==")
        );
        assert_eq!(response_json["output_format"].as_str(), Some("webp"));
        assert_eq!(response_json["quality"].as_str(), Some("high"));
        assert_eq!(response_json["size"].as_str(), Some("1024x1024"));
        assert_eq!(response_json["background"].as_str(), Some("transparent"));
        assert_eq!(response_json["model"].as_str(), Some("gpt-image-2"));
    });
}

#[test]
fn openai_image_generation_codex_bridge_streams_images_events() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_stream\",\"object\":\"response\",\"created_at\":1710000012,\"model\":\"gpt-5.4-mini\",\"tools\":[{\"type\":\"image_generation\",\"model\":\"gpt-image-2\",\"output_format\":\"png\"}]}}\n\n\
data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_b64\":\"cGFydA==\",\"partial_image_index\":0,\"output_format\":\"png\"}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_stream\",\"object\":\"response\",\"created_at\":1710000012,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"ZmluYWw=\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_stream_events");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw stream",
                    "stream": true,
                    "response_format": "url"
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_text = String::from_utf8_lossy(&response_bytes);

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert!(response_text.contains("event: image_generation.partial_image"));
        assert!(response_text.contains("event: image_generation.completed"));
        assert!(response_text.contains("\"type\":\"image_generation.completed\""));
        assert!(response_text.contains("\"url\":\"data:image/png;base64,ZmluYWw=\""));
        assert!(response_text.contains("\"b64_json\":\"ZmluYWw=\""));
        assert!(!response_text.contains("\"type\":\"response.completed\""));
    });
}

#[test]
fn openai_image_generation_codex_bridge_streams_output_item_done_with_usage() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"ZmFsbGJhY2s=\",\"revised_prompt\":\"draw fallback stream\",\"output_format\":\"png\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_stream_item\",\"object\":\"response\",\"created_at\":1710000013,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"usage\":{\"input_tokens\":6,\"output_tokens\":10,\"total_tokens\":16,\"output_tokens_details\":{\"image_tokens\":10}},\"tool_usage\":{\"image_gen\":{\"images\":1}},\"output\":[]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_stream_item_done_usage");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw fallback stream",
                    "stream": true,
                    "response_format": "url"
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_text = String::from_utf8_lossy(&response_bytes);

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert!(response_text.contains("event: image_generation.completed"));
        assert!(response_text.contains("\"url\":\"data:image/png;base64,ZmFsbGJhY2s=\""));
        assert!(response_text.contains("\"b64_json\":\"ZmFsbGJhY2s=\""));
        assert!(response_text.contains("\"revised_prompt\":\"draw fallback stream\""));
        assert!(response_text.contains("\"usage\":{\"input_tokens\":6"));
        assert!(!response_text.contains("\"usage\":{\"images\":1}"));
    });
}

#[test]
fn openai_image_generation_codex_bridge_stream_errors_without_image_output() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_empty\",\"object\":\"response\",\"created_at\":1710000014,\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_stream_empty_output");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw empty stream",
                    "stream": true
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_text = String::from_utf8_lossy(&response_bytes);

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert!(response_text.contains("event: error"));
        assert!(response_text.contains("upstream did not return image output"));
        assert!(!response_text.contains("event: image_generation.completed"));
    });
}

#[test]
fn openai_image_generation_codex_bridge_stream_fills_missing_created_at() {
    run_async(async {
        let codex = spawn_mock_raw_upstream(
            StatusCode::OK,
            Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_no_created\",\"object\":\"response\",\"model\":\"gpt-5.4-mini\",\"status\":\"completed\",\"output\":[{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"result\":\"bm93\",\"output_format\":\"png\"}]}}\n\n\
data: [DONE]\n\n",
            ),
            "text/event-stream",
        )
        .await;

        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            10,
            "codex-images",
            codex.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        let data_dir = next_test_data_dir("openai_images_generation_stream_missing_created");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/generations"),
            axum::http::HeaderMap::new(),
            Body::from(
                json!({
                    "model": "gpt-image-2",
                    "prompt": "draw without created",
                    "stream": true
                })
                .to_string(),
            ),
        )
        .await;

        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_text = String::from_utf8_lossy(&response_bytes);

        codex.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        let created_at = response_text
            .split("\"created_at\":")
            .nth(1)
            .and_then(|tail| tail.split([',', '}']).next())
            .and_then(|value| value.parse::<i64>().ok())
            .expect("stream created_at");

        assert_eq!(response_status, StatusCode::OK);
        assert!(created_at > 0);
    });
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
        config.codex_session_scoped_cooldown_enabled = true;

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
fn responses_context_window_error_does_not_failover_same_request() {
    run_async(async {
        let primary = spawn_mock_upstream(
            StatusCode::BAD_GATEWAY,
            json!({
                "error": {
                    "message": "Your input exceeds the context window of this model. Please adjust your input and try again.",
                    "type": "upstream_error"
                }
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

        let data_dir = next_test_data_dir("responses_context_window_no_failover");
        let state = build_test_state_handle(config, data_dir.clone()).await;

        let (status, body) = send_responses_request(state).await;
        let primary_requests = primary.requests();
        let secondary_requests = secondary.requests();

        primary.abort();
        secondary.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body["error"]["message"].as_str(),
            Some(
                "Your input exceeds the context window of this model. Please adjust your input and try again."
            )
        );
        assert_eq!(primary_requests.len(), 1);
        assert!(
            secondary_requests.is_empty(),
            "context-window semantic errors must not switch accounts"
        );
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
fn responses_compact_requires_responses_family_provider() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_ALL)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_COMPACT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn responses_compact_prefers_responses_provider_and_preserves_path() {
    let config = config_with_upstreams(&[
        (PROVIDER_CHAT, 10, "chat", FORMATS_ALL),
        (PROVIDER_RESPONSES, 0, "responses", FORMATS_RESPONSES),
    ]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_COMPACT_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);

    let outbound_path = resolve_outbound_path(
        RESPONSES_COMPACT_PATH,
        &plan,
        &RequestMeta {
            client_ip: None,
            stream: false,
            original_model: Some("gpt-5.4".to_string()),
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound_path, RESPONSES_COMPACT_PATH);
}

#[test]
fn responses_compact_can_route_to_codex_and_preserve_compact_suffix() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_COMPACT_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(
        plan.request_transform,
        FormatTransform::ResponsesCompactToCodex
    );
    assert_eq!(plan.response_transform, FormatTransform::CodexToResponses);

    let outbound_path = resolve_outbound_path(
        RESPONSES_COMPACT_PATH,
        &plan,
        &RequestMeta {
            client_ip: None,
            stream: false,
            original_model: Some("gpt-5.4".to_string()),
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound_path, "/responses/compact");
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
fn openai_compatible_responses_to_codex_uses_conversion() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToResponses);
}

#[test]
fn native_codex_responses_request_passthroughs_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert(
        "user-agent",
        HeaderValue::from_static("codex_cli_rs/0.135.0 (Mac OS 15.5.0; arm64) codex-cli"),
    );

    let plan = resolve_dispatch_plan_with_request(&config, RESPONSES_PATH, &headers, None)
        .expect("should dispatch native codex request");

    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
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
fn retry_fallback_plan_switches_compact_responses_to_codex() {
    let config = config_with_providers(&[
        (PROVIDER_RESPONSES, FORMATS_RESPONSES),
        (PROVIDER_CODEX, FORMATS_RESPONSES),
    ]);
    let plan = resolve_retry_fallback_plan(&config, RESPONSES_COMPACT_PATH, PROVIDER_RESPONSES)
        .expect("should fallback to codex");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(
        plan.request_transform,
        FormatTransform::ResponsesCompactToCodex
    );
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
fn retry_fallback_plan_allows_openai_response_chat_to_native_codex_provider() {
    let config = config_with_providers(&[
        (PROVIDER_RESPONSES, FORMATS_RESPONSES),
        (PROVIDER_CODEX, FORMATS_RESPONSES),
    ]);
    let plan = resolve_retry_fallback_plan(&config, CHAT_PATH, PROVIDER_RESPONSES)
        .expect("should fallback to native codex provider");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToChat);
}

#[test]
fn retry_fallback_plan_switches_chat_from_openai_to_responses() {
    let config = config_with_providers(&[
        (PROVIDER_CHAT, FORMATS_CHAT),
        (PROVIDER_RESPONSES, FORMATS_ALL),
    ]);
    let plan = resolve_retry_fallback_plan(&config, CHAT_PATH, PROVIDER_CHAT)
        .expect("should fallback to openai responses");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToResponses);
    assert_eq!(plan.response_transform, FormatTransform::ResponsesToChat);
}

#[test]
fn retry_fallback_plan_allows_openai_to_native_responses_provider() {
    let config = config_with_providers(&[
        (PROVIDER_CHAT, FORMATS_CHAT),
        (PROVIDER_RESPONSES, FORMATS_RESPONSES),
    ]);
    let plan = resolve_retry_fallback_plan(&config, CHAT_PATH, PROVIDER_CHAT)
        .expect("should fallback to native responses provider");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToResponses);
    assert_eq!(plan.response_transform, FormatTransform::ResponsesToChat);
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
            client_ip: None,
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
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
            client_ip: None,
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
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
fn gemini_count_tokens_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:countTokens")
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn anthropic_count_tokens_bridges_to_responses_input_tokens() {
    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages/count_tokens").expect("should bridge");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some("/v1/responses/input_tokens"));
    assert_eq!(
        plan.request_transform,
        FormatTransform::AnthropicCountTokensToResponsesInputTokens
    );
    assert_eq!(
        plan.response_transform,
        FormatTransform::ResponsesInputTokensToAnthropicCountTokens
    );
}

#[test]
fn anthropic_count_tokens_preserves_authorization_header_name_for_upstream() {
    run_async(async {
        let upstream = spawn_mock_upstream(StatusCode::OK, json!({ "input_tokens": 12 })).await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_ANTHROPIC,
            0,
            "anthropic-auth-relay",
            upstream.base_url.as_str(),
            FORMATS_MESSAGES,
        )]);
        let data_dir = next_test_data_dir("anthropic_count_tokens_authorization_header");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer local-debug-key"),
        );
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let (status, body) = send_anthropic_count_tokens_request(state, headers).await;
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["input_tokens"].as_u64(), Some(12));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/messages/count_tokens");
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer test-key")
        );
    });
}

#[test]
fn anthropic_count_tokens_request_bridges_to_responses_input_tokens_upstream() {
    run_async(async {
        let upstream = spawn_mock_upstream(StatusCode::OK, json!({ "input_tokens": 12 })).await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "responses-count-tokens",
            upstream.base_url.as_str(),
            FORMATS_ALL,
        )]);
        let data_dir = next_test_data_dir("anthropic_count_tokens_responses_bridge");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let (status, body) = send_anthropic_count_tokens_request_with_body(
            state,
            headers,
            json!({
                "model": "gpt-5.4",
                "messages": [
                    {
                        "role": "user",
                        "content": [{ "type": "text", "text": "hi from claude" }]
                    }
                ],
                "system": "count only",
                "temperature": 0.7,
                "stream": true
            }),
        )
        .await;
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["input_tokens"].as_u64(), Some(12));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/responses/input_tokens");
        assert_eq!(requests[0].body["model"], json!("gpt-5.4"));
        assert_eq!(requests[0].body["instructions"], json!("count only"));
        assert_eq!(requests[0].body["input"][0]["role"], json!("user"));
        assert!(requests[0].body.get("stream").is_none());
        assert!(requests[0].body.get("temperature").is_none());
    });
}

#[test]
fn anthropic_count_tokens_filters_generation_only_fields_for_upstream() {
    run_async(async {
        let upstream = spawn_mock_upstream(StatusCode::OK, json!({ "input_tokens": 12 })).await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_ANTHROPIC,
            0,
            "anthropic-count-tokens-filter",
            upstream.base_url.as_str(),
            FORMATS_MESSAGES,
        )]);
        let data_dir = next_test_data_dir("anthropic_count_tokens_filters_generation_fields");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let (status, _body) = send_anthropic_count_tokens_request_with_body(
            state,
            headers,
            json!({
                "model": "claude-sonnet-4-5",
                "messages": [
                    {
                        "role": "user",
                        "content": [{ "type": "text", "text": "hi from claude" }]
                    }
                ],
                "temperature": 0.7,
                "top_p": 0.9,
                "top_k": 40,
                "stream": true,
                "stop_sequences": ["END"],
                "stop": ["legacy"],
                "metadata": { "trace": "debug" }
            }),
        )
        .await;
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(requests.len(), 1);
        let body = &requests[0].body;
        assert_eq!(body["model"], json!("claude-sonnet-4-5"));
        assert!(body.get("messages").is_some());
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("top_k").is_none());
        assert!(body.get("stream").is_none());
        assert!(body.get("stop_sequences").is_none());
        assert!(body.get("stop").is_none());
        assert!(body.get("metadata").is_none());
    });
}

#[test]
fn anthropic_messages_preserves_generation_fields_for_upstream() {
    run_async(async {
        let upstream = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-5",
                "content": [{ "type": "text", "text": "ok" }],
                "usage": { "input_tokens": 1, "output_tokens": 1 }
            }),
        )
        .await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_ANTHROPIC,
            0,
            "anthropic-messages-preserve",
            upstream.base_url.as_str(),
            FORMATS_MESSAGES,
        )]);
        let data_dir = next_test_data_dir("anthropic_messages_preserves_generation_fields");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let request_body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 64,
            "messages": [
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hi from claude" }]
                }
            ],
            "temperature": 0.7,
            "top_p": 0.9,
            "top_k": 40,
            "stream": false,
            "stop_sequences": ["END"],
            "metadata": { "trace": "debug" }
        });

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/messages"),
            HeaderMap::new(),
            Body::from(request_body.to_string()),
        )
        .await;
        let status = response.status();
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/messages");
        assert_eq!(requests[0].body["temperature"], json!(0.7));
        assert_eq!(requests[0].body["top_p"], json!(0.9));
        assert_eq!(requests[0].body["top_k"], json!(40));
        assert_eq!(requests[0].body["stream"], json!(false));
        assert_eq!(requests[0].body["stop_sequences"], json!(["END"]));
        assert_eq!(requests[0].body["metadata"], json!({ "trace": "debug" }));
    });
}

#[test]
fn gemini_embed_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/text-embedding-004:embedContent")
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn gemini_non_generate_route_does_not_fallback_to_formatless_provider() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_CHAT)]);
    let error = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:countTokens")
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn gemini_upload_files_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan = resolve_dispatch_plan(&config, "/upload/v1beta/files").expect("should dispatch");
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
            client_ip: None,
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
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
            client_ip: None,
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
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
            client_ip: None,
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
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
            client_ip: None,
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
            estimated_input_tokens: None,
        },
    );
    assert_eq!(outbound, "/v1/models/gpt-5");
}

#[test]
fn openai_files_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/files").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn openai_uploads_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/uploads").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_batches_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/batches").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_vector_store_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan =
        resolve_dispatch_plan(&config, "/v1/vector_stores/vs_123/files").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_images_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/images/generations").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_image_edits_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/images/edits").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_image_edits_request_preserves_multipart_passthrough() {
    run_async(async {
        let upstream = spawn_multipart_probe_upstream(json!({
            "created": 123,
            "data": [
                {
                    "b64_json": "ZmFrZS1pbWFnZS1kYXRh"
                }
            ]
        }))
        .await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CHAT,
            0,
            "chat-image-edits",
            upstream.base_url.as_str(),
            FORMATS_CHAT,
        )]);
        let data_dir = next_test_data_dir("openai_image_edits_multipart_passthrough");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let boundary = "tp-boundary-123";
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let request_body = format!(
            "--{boundary}\r\n\
Content-Disposition: form-data; name=\"model\"\r\n\r\n\
gpt-image-2\r\n\
--{boundary}\r\n\
Content-Disposition: form-data; name=\"prompt\"\r\n\r\n\
add red hat\r\n\
--{boundary}\r\n\
Content-Disposition: form-data; name=\"image\"; filename=\"input.png\"\r\n\
Content-Type: image/png\r\n\r\n\
fake-png-bytes\r\n\
--{boundary}--\r\n"
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type).expect("multipart content-type"),
        );

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/images/edits"),
            headers,
            Body::from(request_body.clone()),
        )
        .await;
        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_json: Value =
            serde_json::from_slice(&response_bytes).expect("proxy response json");
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(
            response_json["data"][0]["b64_json"].as_str(),
            Some("ZmFrZS1pbWFnZS1kYXRh")
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/images/edits");
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer test-key")
        );
        assert_eq!(
            requests[0].content_type.as_deref(),
            Some(content_type.as_str())
        );
        assert_eq!(requests[0].body, Bytes::from(request_body));
    });
}

#[test]
fn openai_audio_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/audio/transcriptions").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_embeddings_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/embeddings").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_embeddings_request_passthroughs_to_openai_provider() {
    run_async(async {
        let upstream = spawn_mock_upstream(
            StatusCode::OK,
            json!({
                "object": "list",
                "data": [
                    {
                        "object": "embedding",
                        "embedding": [0.1, 0.2],
                        "index": 0
                    }
                ],
                "model": "text-embedding-3-small",
                "usage": { "prompt_tokens": 2, "total_tokens": 2 }
            }),
        )
        .await;
        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_ANTHROPIC,
                10,
                "anthropic-high-priority",
                "https://api.anthropic.com",
                FORMATS_MESSAGES,
            ),
            (
                PROVIDER_CHAT,
                0,
                "openai-embeddings",
                upstream.base_url.as_str(),
                FORMATS_CHAT,
            ),
        ]);
        let data_dir = next_test_data_dir("openai_embeddings_passthrough");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let request_body = json!({
            "model": "text-embedding-3-small",
            "input": ["hello", "world"],
            "encoding_format": "float"
        });

        let response = proxy_request(
            State(state),
            Method::POST,
            Uri::from_static("/v1/embeddings"),
            HeaderMap::new(),
            Body::from(request_body.to_string()),
        )
        .await;
        let response_status = response.status();
        let response_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("proxy response bytes");
        let response_json: Value =
            serde_json::from_slice(&response_bytes).expect("proxy response json");
        let requests = upstream.requests();

        upstream.abort();
        let _ = std::fs::remove_dir_all(&data_dir);

        assert_eq!(response_status, StatusCode::OK);
        assert_eq!(response_json["model"], json!("text-embedding-3-small"));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/embeddings");
        assert_eq!(requests[0].body, request_body);
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer test-key")
        );
    });
}

#[test]
fn openai_moderations_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/moderations").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_completions_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/completions").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_fine_tuning_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/fine_tuning/jobs").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_chat_completion_resource_route_prefers_openai_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_CHAT, 0, "chat", FORMATS_CHAT),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/chat/completions/cmpt_123/messages")
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_responses_resource_route_prefers_openai_responses_provider_over_anthropic_priority() {
    let config = config_with_upstreams(&[
        (PROVIDER_ANTHROPIC, 10, "anthropic", FORMATS_MESSAGES),
        (PROVIDER_RESPONSES, 0, "responses", FORMATS_RESPONSES),
    ]);
    let plan = resolve_dispatch_plan(&config, "/v1/responses/resp_123").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
}

#[test]
fn openai_responses_resource_route_falls_back_to_openai_provider_when_responses_missing() {
    let config = config_with_upstreams(&[(PROVIDER_CHAT, 0, "chat", FORMATS_CHAT)]);
    let plan = resolve_dispatch_plan(&config, "/v1/responses/resp_123").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CHAT);
}

#[test]
fn openai_files_route_falls_back_to_responses_provider_when_openai_missing() {
    let config = config_with_upstreams(&[(PROVIDER_RESPONSES, 0, "responses", FORMATS_RESPONSES)]);
    let plan = resolve_dispatch_plan(&config, "/v1/files/file_123").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
}
