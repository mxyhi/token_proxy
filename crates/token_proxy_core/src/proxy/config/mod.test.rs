use super::*;
use std::collections::HashMap;
use std::time::Duration;

#[test]
fn build_runtime_config_rejects_retryable_failure_cooldown_that_overflows_instant() {
    let mut config = ProxyConfigFile::default();
    config.retryable_failure_cooldown_secs = u64::MAX;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_routes_openai_responses_via_chat_when_enabled() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![UpstreamConfig {
        id: "glm-coding-plan".to_string(),
        providers: vec!["openai-response".to_string()],
        base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
        api_keys: vec!["test-key".to_string()],
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: true,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }];

    let runtime = build_runtime_config(config).expect("runtime config");
    assert!(runtime.provider_upstreams("openai-response").is_none());

    let openai = runtime
        .provider_upstreams("openai")
        .expect("openai runtime upstream");
    let item = openai
        .groups
        .first()
        .and_then(|group| group.items.first())
        .expect("runtime item");

    assert!(item.supports_inbound(InboundApiFormat::OpenaiResponses));
    assert!(!item.supports_inbound(InboundApiFormat::OpenaiChat));
}

#[test]
fn build_runtime_config_keeps_openai_responses_provider_when_chat_compat_disabled() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![UpstreamConfig {
        id: "glm-coding-plan".to_string(),
        providers: vec!["openai-response".to_string()],
        base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
        api_keys: vec!["test-key".to_string()],
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }];

    let runtime = build_runtime_config(config).expect("runtime config");
    assert!(runtime.provider_upstreams("openai").is_none());

    let openai_responses = runtime
        .provider_upstreams("openai-response")
        .expect("openai-response runtime upstream");
    let item = openai_responses
        .groups
        .first()
        .and_then(|group| group.items.first())
        .expect("runtime item");

    assert!(item.supports_inbound(InboundApiFormat::OpenaiResponses));
}

#[test]
fn build_runtime_config_maps_upstream_no_data_timeout_secs() {
    let mut config = ProxyConfigFile::default();
    config.upstream_no_data_timeout_secs = 3;

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(runtime.upstream_no_data_timeout, Duration::from_secs(3));
}

#[test]
fn build_runtime_config_rejects_upstream_no_data_timeout_below_minimum() {
    let mut config = ProxyConfigFile::default();
    config.upstream_no_data_timeout_secs = 2;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_upstream_no_data_timeout_that_overflows_instant() {
    let mut config = ProxyConfigFile::default();
    config.upstream_no_data_timeout_secs = u64::MAX;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_expands_multiple_api_keys_into_multiple_runtime_upstreams() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![UpstreamConfig {
        id: "shared-openai".to_string(),
        providers: vec!["openai".to_string()],
        base_url: "https://api.openai.com".to_string(),
        api_keys: vec!["key-a".to_string(), "key-b".to_string()],
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }];

    let runtime = build_runtime_config(config).expect("runtime config");
    let openai = runtime
        .provider_upstreams("openai")
        .expect("openai runtime upstream");
    let items = &openai.groups[0].items;

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "shared-openai");
    assert_eq!(items[0].selector_key, "shared-openai#1");
    assert_eq!(items[0].api_key.as_deref(), Some("key-a"));
    assert_eq!(items[1].selector_key, "shared-openai#2");
    assert_eq!(items[1].api_key.as_deref(), Some("key-b"));
}

#[test]
fn build_runtime_config_rejects_multiple_api_keys_for_account_based_provider() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![UpstreamConfig {
        id: "codex-account".to_string(),
        providers: vec!["codex".to_string()],
        base_url: String::new(),
        api_keys: vec!["key-a".to_string(), "key-b".to_string()],
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: Some("codex-account.json".to_string()),
        antigravity_account_id: None,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }];

    let result = build_runtime_config(config);

    assert!(result.is_err());
}
