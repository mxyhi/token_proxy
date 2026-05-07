use super::*;
use crate::app_proxy;
use crate::logging::LogLevel;
use crate::paths::TokenProxyPaths;
use crate::proxy::config::UpstreamConfig;
use crate::proxy::model_discovery::UpstreamModelProbeStatus;
use axum::{
    extract::State,
    http::{StatusCode, Uri},
    response::IntoResponse,
    routing::any,
    Router,
};
use rand::random;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;

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
        codex_session_scoped_cooldown_enabled: false,
        upstream_no_data_timeout: Duration::from_secs(120),
        upstream_strategy: crate::proxy::config::UpstreamStrategyRuntime::default(),
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
    next.upstream_no_data_timeout = Duration::from_secs(7);

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

fn test_config_file(port: u16) -> crate::proxy::config::ProxyConfigFile {
    crate::proxy::config::ProxyConfigFile {
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

fn upstream_config(id: &str, provider: &str, base_url: &str) -> UpstreamConfig {
    UpstreamConfig {
        id: id.to_string(),
        providers: vec![provider.to_string()],
        base_url: base_url.to_string(),
        api_keys: vec!["test-key".to_string()],
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        preferred_endpoint: None,
        proxy_url: None,
        priority: None,
        enabled: true,
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }
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
            crate::kiro::KiroAccountStore::new(paths.as_ref(), app_proxy.clone())
                .expect("kiro store"),
        ),
        codex_accounts: Arc::new(
            crate::codex::CodexAccountStore::new(paths.as_ref(), app_proxy.clone())
                .expect("codex store"),
        ),
    };
    (context, data_dir)
}

#[test]
fn apply_saved_config_keeps_proxy_stopped_when_service_is_stopped() {
    run_async(async {
        let (context, data_dir) = create_test_context();
        crate::proxy::config::write_config(context.paths.as_ref(), test_config_file(0))
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
        crate::proxy::config::write_config(context.paths.as_ref(), test_config_file(0))
            .await
            .expect("write initial config");

        let service = ProxyServiceHandle::new();
        let start_status = service.start(&context).await.expect("start proxy");
        assert!(matches!(start_status.state, ProxyServiceState::Running));

        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind blocker");
        let blocked_port = blocker.local_addr().expect("blocker local addr").port();

        crate::proxy::config::write_config(context.paths.as_ref(), test_config_file(blocked_port))
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
        crate::proxy::config::write_config(context.paths.as_ref(), config)
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
        crate::proxy::config::write_config(context.paths.as_ref(), config)
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
        crate::proxy::config::write_config(context.paths.as_ref(), config)
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
