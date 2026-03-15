use super::*;
use std::collections::HashMap;

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
        api_key: Some("test-key".to_string()),
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
        api_key: Some("test-key".to_string()),
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
