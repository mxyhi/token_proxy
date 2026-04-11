use super::*;
use crate::app_proxy;
use crate::logging::LogLevel;
use crate::paths::TokenProxyPaths;
use rand::random;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn config_with_addr_and_body_limit(
    host: &str,
    port: u16,
    max_request_body_bytes: usize,
) -> ProxyConfig {
    ProxyConfig {
        host: host.to_string(),
        port,
        local_api_key: None,
        model_list_prefix: false,
        log_level: LogLevel::Silent,
        max_request_body_bytes,
        retryable_failure_cooldown: Duration::from_secs(15),
        upstream_no_data_timeout: Duration::from_secs(120),
        upstream_strategy: crate::proxy::config::UpstreamStrategyRuntime::default(),
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
