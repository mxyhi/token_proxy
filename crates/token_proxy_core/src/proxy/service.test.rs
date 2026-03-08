use super::*;
use crate::logging::LogLevel;
use std::collections::HashMap;
use std::time::Duration;

fn config_with_addr_and_body_limit(host: &str, port: u16, max_request_body_bytes: usize) -> ProxyConfig {
    ProxyConfig {
        host: host.to_string(),
        port,
        local_api_key: None,
        log_level: LogLevel::Silent,
        max_request_body_bytes,
        retryable_failure_cooldown: Duration::from_secs(15),
        upstream_strategy: crate::proxy::config::UpstreamStrategy::PriorityFillFirst,
        upstreams: HashMap::new(),
        kiro_preferred_endpoint: None,
        antigravity_user_agent: None,
    }
}

#[test]
fn classify_reload_behavior_returns_reload_for_hot_reload_safe_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let next = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);

    let action = classify_reload_behavior(Some((current.addr(), current.max_request_body_bytes)), &next);

    assert_eq!(action, ProxyConfigApplyBehavior::Reload);
}

#[test]
fn classify_reload_behavior_restarts_when_addr_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let next = config_with_addr_and_body_limit("127.0.0.1", 9300, 1024);

    let action = classify_reload_behavior(Some((current.addr(), current.max_request_body_bytes)), &next);

    assert_eq!(action, ProxyConfigApplyBehavior::Restart);
}

#[test]
fn classify_reload_behavior_restarts_when_body_limit_changes() {
    let current = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);
    let next = config_with_addr_and_body_limit("127.0.0.1", 9208, 2048);

    let action = classify_reload_behavior(Some((current.addr(), current.max_request_body_bytes)), &next);

    assert_eq!(action, ProxyConfigApplyBehavior::Restart);
}

#[test]
fn classify_reload_behavior_skips_apply_when_proxy_is_stopped() {
    let next = config_with_addr_and_body_limit("127.0.0.1", 9208, 1024);

    let action = classify_reload_behavior(None, &next);

    assert_eq!(action, ProxyConfigApplyBehavior::SavedOnly);
}
