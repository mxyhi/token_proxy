use super::*;

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::any,
    Router,
};
use serde_json::{json, Value};
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
    InboundApiFormat, ProviderUpstreams, ProxyConfig, UpstreamGroup, UpstreamRuntime,
    UpstreamStrategy,
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
            base_url: (*base_url).to_string(),
            api_key: Some("test-key".to_string()),
            filter_prompt_cache_retention: false,
            filter_safety_identifier: false,
            kiro_account_id: None,
            codex_account_id: (*provider == PROVIDER_CODEX).then(|| format!("codex-{id}.json")),
            antigravity_account_id: None,
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
        upstream_strategy: UpstreamStrategy::PriorityRoundRobin,
        upstreams: provider_map,
        kiro_preferred_endpoint: None,
        antigravity_user_agent: None,
    }
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    path: String,
    body: Value,
}

#[derive(Clone)]
struct MockUpstreamState {
    status: StatusCode,
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

async fn mock_upstream_handler(
    State(state): State<Arc<MockUpstreamState>>,
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
        });
    (
        state.status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.body.to_string(),
    )
        .into_response()
}

async fn spawn_mock_upstream(status: StatusCode, body: Value) -> MockUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(MockUpstreamState {
        status,
        body,
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
    let app_proxy = crate::app_proxy::new_state();
    let cursors = build_upstream_cursors(&config);
    let kiro_accounts = Arc::new(
        crate::kiro::KiroAccountStore::new(&paths, app_proxy.clone()).expect("kiro store"),
    );
    let codex_accounts = Arc::new(
        crate::codex::CodexAccountStore::new(&paths, app_proxy.clone()).expect("codex store"),
    );
    let antigravity_accounts = Arc::new(
        crate::antigravity::AntigravityAccountStore::new(&paths, app_proxy)
            .expect("antigravity store"),
    );
    let expires_at = (OffsetDateTime::now_utc() + TimeDuration::days(1))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("format expires_at");
    for upstreams in config.upstreams.values() {
        for group in &upstreams.groups {
            for upstream in &group.items {
                let Some(account_id) = upstream.codex_account_id.as_deref() else {
                    continue;
                };
                let auth_dir = paths.data_dir().join("codex-auth");
                std::fs::create_dir_all(&auth_dir).expect("create codex auth dir");
                let record = json!({
                    "access_token": "codex-access-token",
                    "refresh_token": "codex-refresh-token",
                    "id_token": "codex-id-token",
                    "account_id": "chatgpt-account",
                    "email": "codex@example.com",
                    "expires_at": expires_at,
                    "last_refresh": null
                });
                std::fs::write(
                    auth_dir.join(account_id),
                    serde_json::to_vec_pretty(&record).expect("serialize codex record"),
                )
                .expect("write codex account");
                codex_accounts
                    .list_accounts()
                    .await
                    .expect("refresh codex account cache");
            }
        }
    }
    let state = Arc::new(ProxyState {
        config,
        http_clients: super::super::http_client::ProxyHttpClients::new().expect("http clients"),
        log: Arc::new(super::super::log::LogWriter::new(None)),
        cursors,
        upstream_selector: super::super::upstream_selector::UpstreamSelectorRuntime::new(),
        request_detail: Arc::new(super::super::request_detail::RequestDetailCapture::new(
            None,
        )),
        token_rate: super::super::token_rate::TokenRateTracker::new(),
        kiro_accounts,
        codex_accounts,
        antigravity_accounts,
    });
    Arc::new(RwLock::new(state))
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
        config.upstream_strategy = UpstreamStrategy::PriorityFillFirst;

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
        config.upstream_strategy = UpstreamStrategy::PriorityFillFirst;

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
fn anthropic_messages_allows_antigravity_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_ANTIGRAVITY, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_ANTIGRAVITY);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::AnthropicToGemini);
    assert_eq!(plan.response_transform, FormatTransform::GeminiToAnthropic);
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
