use super::*;
use crate::logging::LogLevel;
use crate::proxy::model_discovery::UpstreamModelProbeStatus;
use axum::{
    body::Bytes,
    extract::State,
    http::{StatusCode, Uri},
    response::IntoResponse,
    routing::{any, post},
    Router,
};
use rand::random;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use token_proxy_account_store::app_proxy;
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_config::{AccountProvider, UpstreamConfig, UpstreamCredential};
use tokio::task::{AbortHandle, JoinHandle};

fn config_with_addr_and_body_limit(
    host: &str,
    port: u16,
    max_request_body_bytes: usize,
) -> ProxyConfig {
    ProxyConfig {
        host: host.to_string(),
        port,
        local_api_key: None,
        cors_enabled: false,
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes,
        retryable_failure_cooldown: Duration::from_secs(15),
        same_upstream_retry_count: 1,
        codex_session_scoped_cooldown_enabled: false,
        xai_inject_x_search: false,
        stream_first_output_timeout: Duration::from_secs(60),
        sync_response_timeout: Duration::from_secs(120),
        upstream_strategy: token_proxy_config::UpstreamStrategyRuntime::default(),
        hot_model_mappings: HashMap::new(),
        upstreams: HashMap::new(),
        kiro_preferred_endpoint: None,
    }
}

#[test]
fn classify_reload_behavior_returns_reload_for_hot_reload_safe_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let next = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);

    let action = classify_reload_behavior(
        Some((current.addr(), current.max_request_body_bytes)),
        &next,
    );

    assert_eq!(action, ProxyConfigApplyBehavior::Reload);
}

#[test]
fn classify_reload_behavior_restarts_when_addr_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let next = config_with_addr_and_body_limit("127.0.0.1", 9300, 1024);

    let action = classify_reload_behavior(
        Some((current.addr(), current.max_request_body_bytes)),
        &next,
    );

    assert_eq!(action, ProxyConfigApplyBehavior::Restart);
}

#[test]
fn classify_reload_behavior_restarts_when_body_limit_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let next = config_with_addr_and_body_limit("127.0.0.1", 9208, 2048);

    let action = classify_reload_behavior(
        Some((current.addr(), current.max_request_body_bytes)),
        &next,
    );

    assert_eq!(action, ProxyConfigApplyBehavior::Restart);
}

#[test]
fn classify_reload_behavior_skips_apply_when_proxy_is_stopped() {
    let next = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);

    let action = classify_reload_behavior(None, &next);

    assert_eq!(action, ProxyConfigApplyBehavior::SavedOnly);
}

#[test]
fn classify_reload_behavior_keeps_reload_for_timeout_only_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let mut next = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    next.sync_response_timeout = Duration::from_secs(7);

    let action = classify_reload_behavior(
        Some((current.addr(), current.max_request_body_bytes)),
        &next,
    );

    assert_eq!(action, ProxyConfigApplyBehavior::Reload);
}

fn run_async(test: impl std::future::Future<Output = ()>) {
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(test);
}

fn test_config_file(port: u16) -> token_proxy_config::ProxyConfigFile {
    token_proxy_config::ProxyConfigFile {
        port,
        ..Default::default()
    }
}

#[derive(Clone)]
struct ModelCatalogProbeState {
    body: Value,
    requests: Arc<Mutex<Vec<String>>>,
}

struct ModelCatalogProbeUpstream {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl ModelCatalogProbeUpstream {
    fn paths(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }

    fn abort(self) {
        self.task.abort();
    }
}

async fn model_catalog_probe_handler(
    State(state): State<Arc<ModelCatalogProbeState>>,
    uri: Uri,
) -> axum::response::Response {
    state
        .requests
        .lock()
        .expect("requests lock")
        .push(uri.path().to_string());
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        state.body.to_string(),
    )
        .into_response()
}

async fn spawn_model_catalog_probe_upstream(body: Value) -> ModelCatalogProbeUpstream {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(ModelCatalogProbeState {
        body,
        requests: requests.clone(),
    });
    let app = Router::new()
        .route("/{*path}", any(model_catalog_probe_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind model catalog probe upstream");
    let addr: SocketAddr = listener.local_addr().expect("model catalog local addr");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("model catalog probe server should run");
    });
    ModelCatalogProbeUpstream {
        base_url: format!("http://{addr}"),
        requests,
        task,
    }
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
                "refresh_token": "service-refreshed-token",
                "id_token": "header.eyJlbWFpbCI6InNlcnZpY2VAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC1zZXJ2aWNlIn19.signature",
                "expires_in": 7200,
            })
            .to_string(),
        )
            .into_response()
    }

    let app = Router::new()
        .route("/oauth/token", post(handler))
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

fn upstream_config(id: &str, provider: &str, base_url: &str) -> UpstreamConfig {
    // Phase A credential 模型：API Key 上游用 ApiKeys；账户型由调用方覆盖 credential。
    let credential = match AccountProvider::from_provider_str(provider) {
        Some(account_provider) => {
            UpstreamCredential::account(account_provider, format!("{provider}-{id}"))
        }
        None => UpstreamCredential::api_keys(["test-key"]),
    };
    UpstreamConfig {
        id: id.to_string(),
        providers: vec![provider.to_string()],
        base_url: base_url.to_string(),
        credential,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: None,
        enabled: true,
        available_models: Vec::new(),
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }
}

fn xai_upstream_config(id: &str, proxy_url: &str) -> UpstreamConfig {
    let mut upstream = upstream_config(id, "xai", token_proxy_account_xai::CLI_BASE_URL);
    // xAI model probe 走 CLI base + 自定义 proxy；credential 仍为固定 account 绑定。
    upstream.proxy_url = Some(proxy_url.to_string());
    upstream
}

fn create_test_context() -> (ProxyContext, std::path::PathBuf) {
    let data_dir =
        std::env::temp_dir().join(format!("token-proxy-service-test-{}", random::<u64>()));
    std::fs::create_dir_all(&data_dir).expect("create test data dir");
    let paths = Arc::new(TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths"));
    let app_proxy = app_proxy::new_state();
    let context = ProxyContext {
        paths: paths.clone(),
        logging: crate::logging::LoggingState::default(),
        request_detail: Arc::new(crate::proxy::request_detail::RequestDetailCapture::default()),
        token_rate: crate::proxy::token_rate::TokenRateTracker::new(),
        kiro_accounts: Arc::new(
            token_proxy_account_kiro::KiroAccountStore::new(paths.as_ref(), app_proxy.clone())
                .expect("kiro store"),
        ),
        codex_accounts: Arc::new(
            token_proxy_account_codex::CodexAccountStore::new(paths.as_ref(), app_proxy.clone())
                .expect("codex store"),
        ),
        xai_accounts: Arc::new(
            token_proxy_account_xai::XaiAccountStore::new(paths.as_ref(), app_proxy)
                .expect("xai store"),
        ),
    };
    (context, data_dir)
}

#[test]
fn apply_saved_config_keeps_proxy_stopped_when_service_is_stopped() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        token_proxy_config::write_config(context.paths.as_ref(), test_config_file(0))
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        let result = service.apply_saved_config(&context).await;

        assert!(matches!(result.status.state, ProxyServiceState::Stopped));
        assert!(result.apply_error.is_none());

        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn apply_saved_config_returns_status_and_error_when_restart_fails() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        token_proxy_config::write_config(context.paths.as_ref(), test_config_file(0))
            .await
            .expect("write initial config");

        let service = ProxyServiceHandle::new();
        let start_status = service.start(&context).await.expect("start proxy");
        assert!(matches!(start_status.state, ProxyServiceState::Running));

        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind blocker");
        let blocked_port = blocker.local_addr().expect("blocker local addr").port();

        token_proxy_config::write_config(context.paths.as_ref(), test_config_file(blocked_port))
            .await
            .expect("write restart config");

        let result = service.apply_saved_config(&context).await;

        assert!(result.apply_error.is_some());
        assert!(matches!(result.status.state, ProxyServiceState::Stopped));
        assert_eq!(result.status.addr, None);
        assert_eq!(result.status.last_error, result.apply_error);

        let _ = service.stop().await;
        drop(blocker);
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn start_refreshes_model_discovery_cache_without_blocking_proxy_start() {
    run_async(async {
        let upstream = spawn_model_catalog_probe_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5.5", "object": "model" },
                { "id": "o4-mini", "object": "model" }
            ]
        }))
        .await;
        let (context, data_dir) = create_test_context();
        let mut config = test_config_file(0);
        config.upstreams = vec![upstream_config(
            "openai-a",
            "openai-response",
            upstream.base_url.as_str(),
        )];
        token_proxy_config::write_config(context.paths.as_ref(), config)
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        let start_status = service.start(&context).await.expect("start proxy");
        assert!(matches!(start_status.state, ProxyServiceState::Running));

        let probes = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let probes = service.model_discovery_snapshot().await;
                if probes
                    .iter()
                    .any(|probe| probe.status == UpstreamModelProbeStatus::Ok)
                {
                    return probes;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("model discovery should complete");

        let probe = probes
            .iter()
            .find(|probe| probe.upstream_id == "openai-a")
            .expect("openai probe");
        assert_eq!(probe.provider, "openai-response");
        assert_eq!(probe.status, UpstreamModelProbeStatus::Ok);
        assert!(probe.models.contains(&"gpt-5.5".to_string()));
        assert!(probe.models.contains(&"o4-mini".to_string()));
        assert_eq!(upstream.paths(), vec!["/v1/models"]);

        let _ = service.stop().await;
        upstream.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn refresh_model_discovery_uses_codex_builtin_catalog_without_models_endpoint() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        let mut config = test_config_file(0);
        config.upstreams = vec![upstream_config("codex-a", "codex", "https://example.com")];
        token_proxy_config::write_config(context.paths.as_ref(), config)
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        service.start(&context).await.expect("start proxy");

        let probes = service.refresh_model_discovery().await;
        let probe = probes
            .iter()
            .find(|probe| probe.upstream_id == "codex-a")
            .expect("codex probe");
        assert_eq!(probe.provider, "codex");
        assert_eq!(probe.status, UpstreamModelProbeStatus::Ok);
        assert_eq!(probe.error, None);
        assert!(probe.models.contains(&"gpt-5.5".to_string()));
        assert!(probe.models.contains(&"gpt-5.3-codex-spark".to_string()));

        let _ = service.stop().await;
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn refresh_model_discovery_xai_failure_keeps_builtin_catalog() {
    run_async(async {
        let upstream = spawn_model_catalog_probe_upstream(json!({
            "object": "list",
            "data": [{ "id": "must-not-be-observed", "object": "model" }]
        }))
        .await;
        let (context, data_dir) = create_test_context();
        let mut config = test_config_file(0);
        config.upstreams = vec![xai_upstream_config("xai-default", &upstream.base_url)];
        token_proxy_config::write_config(context.paths.as_ref(), config)
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        service.start(&context).await.expect("start proxy");

        let probes = service.refresh_model_discovery().await;
        let probe = probes
            .iter()
            .find(|probe| probe.upstream_id == "xai-default")
            .expect("xai probe");
        assert_eq!(probe.provider, "xai");
        assert_eq!(probe.status, UpstreamModelProbeStatus::Failed);
        assert!(probe.error.is_some());
        assert!(probe.models.contains(&"grok-4.5".to_string()));
        assert!(probe.models.contains(&"grok-composer-2.5-fast".to_string()));
        assert!(
            upstream.paths().is_empty(),
            "xAI probe without an account must fail before network access"
        );

        let _ = service.stop().await;
        upstream.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn refresh_model_discovery_updates_cache_on_demand() {
    run_async(async {
        let upstream = spawn_model_catalog_probe_upstream(json!({
            "object": "list",
            "data": [
                { "id": "gpt-5.5", "object": "model" }
            ]
        }))
        .await;
        let (context, data_dir) = create_test_context();
        let mut config = test_config_file(0);
        config.upstreams = vec![upstream_config(
            "openai-a",
            "openai-response",
            upstream.base_url.as_str(),
        )];
        token_proxy_config::write_config(context.paths.as_ref(), config)
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        service.start(&context).await.expect("start proxy");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if upstream.paths().len() == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("startup discovery should complete");

        let probes = service.refresh_model_discovery().await;
        let probe = probes
            .iter()
            .find(|probe| probe.upstream_id == "openai-a")
            .expect("openai probe");
        assert_eq!(probe.status, UpstreamModelProbeStatus::Ok);
        assert!(probe.models.contains(&"gpt-5.5".to_string()));
        assert_eq!(upstream.paths(), vec!["/v1/models", "/v1/models"]);

        let _ = service.stop().await;
        upstream.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn start_spawns_codex_keepalive_without_codex_upstream() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        let (token_url, token_task) = spawn_codex_token_endpoint("service-refreshed-access").await;
        context.codex_accounts.set_test_token_url(&token_url).await;
        let expires_at = (time::OffsetDateTime::now_utc() - time::Duration::hours(1))
            .format(&time::format_description::well_known::Rfc3339)
            .expect("format expires_at");
        context
            .codex_accounts
            .save_record_for_test(
                "codex-service.json".to_string(),
                token_proxy_account_codex::CodexTokenRecord {
                    credential: token_proxy_account_codex::CodexCredential::Oauth {
                        access_token: "service-expired-access".to_string(),
                        refresh_token: "service-refresh-token".to_string(),
                        client_id: Some(
                            token_proxy_account_codex::CodexRefreshTokenClient::Codex
                                .client_id()
                                .to_string(),
                        ),
                        id_token: "header.eyJlbWFpbCI6InNlcnZpY2VAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC1zZXJ2aWNlIn19.signature".to_string(),
                        auto_refresh_enabled: true,
                        openai_device_id: None,
                        expires_at,
                        last_refresh: None,
                    },
                    status: token_proxy_account_codex::CodexAccountStatus::Active,
                    account_id: Some("acct-service".to_string()),
                    user_id: None,
                    email: Some("service@example.com".to_string()),
                    quota: token_proxy_account_codex::CodexQuotaCache::default(),
                },
            )
            .await
            .expect("seed codex account");
        token_proxy_config::write_config(context.paths.as_ref(), test_config_file(0))
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        service.start(&context).await.expect("start proxy");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let record = context
                    .codex_accounts
                    .load_account_for_test("codex-service.json")
                    .await
                    .expect("load codex account");
                if record
                    .oauth()
                    .is_some_and(|oauth| oauth.access_token == "service-refreshed-access")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("codex keepalive should refresh without codex upstream");

        let _ = service.stop().await;
        token_task.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn stop_aborts_xai_account_refresh_task() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        token_proxy_config::write_config(context.paths.as_ref(), test_config_file(0))
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        service.start(&context).await.expect("start proxy");
        let refresh_task = xai_refresh_abort_handle(&service).await;
        assert!(!refresh_task.is_finished());

        service.stop().await.expect("stop proxy");
        wait_for_task_finish(&refresh_task).await;

        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn reload_restarts_xai_account_refresh_task() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        token_proxy_config::write_config(context.paths.as_ref(), test_config_file(0))
            .await
            .expect("write config");

        let service = ProxyServiceHandle::new();
        service.start(&context).await.expect("start proxy");
        let previous_task = xai_refresh_abort_handle(&service).await;

        service.reload(&context).await.expect("reload proxy");
        wait_for_task_finish(&previous_task).await;
        let replacement_task = xai_refresh_abort_handle(&service).await;
        assert_ne!(previous_task.id(), replacement_task.id());
        assert!(!replacement_task.is_finished());

        service.stop().await.expect("stop proxy");
        wait_for_task_finish(&replacement_task).await;
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

async fn xai_refresh_abort_handle(service: &ProxyServiceHandle) -> AbortHandle {
    let inner = service.inner.inner.lock().await;
    inner
        .running
        .as_ref()
        .and_then(|running| running.xai_account_refresh_task.as_ref())
        .expect("xai account refresh task")
        .abort_handle()
}

async fn wait_for_task_finish(task: &AbortHandle) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background task should stop");
}
