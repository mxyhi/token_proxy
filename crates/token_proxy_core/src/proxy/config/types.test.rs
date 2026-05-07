use super::*;

#[test]
fn test_strip_overlapping_prefix() {
    // 标准 OpenAI 兼容格式：base_url 包含 /v1
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/openai/v1", "/v1/chat/completions"),
        "/chat/completions"
    );
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/v1", "/v1/chat/completions"),
        "/chat/completions"
    );

    // 无重叠情况：base_url 不包含路径
    assert_eq!(
        strip_overlapping_prefix("https://api.openai.com", "/v1/chat/completions"),
        "/v1/chat/completions"
    );

    // 无重叠情况：base_url 路径与请求路径无公共后缀
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/openai/", "/v1/chat/completions"),
        "/v1/chat/completions"
    );
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/openai", "/v1/chat/completions"),
        "/v1/chat/completions"
    );

    // 多层路径重叠
    assert_eq!(
        strip_overlapping_prefix("https://example.com/api/openai/v1", "/v1/models"),
        "/models"
    );

    // 完整路径重叠
    assert_eq!(
        strip_overlapping_prefix("https://example.com/openai/v1", "/openai/v1/completions"),
        "/completions"
    );

    // 带尾斜杠的 base_url
    assert_eq!(
        strip_overlapping_prefix("https://example.com/v1/", "/v1/chat/completions"),
        "/chat/completions"
    );

    // 无效 URL 回退
    assert_eq!(
        strip_overlapping_prefix("not-a-valid-url", "/v1/chat/completions"),
        "/v1/chat/completions"
    );
}

fn hot_model_test_upstream(model_mappings: Option<ModelMappingRules>) -> UpstreamRuntime {
    UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.example.com/v1".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    }
}

#[test]
fn hot_model_aliases_normalize_popular_provider_namespaces() {
    let mappings = super::super::hot_model_mappings::default_hot_model_mappings();
    let rules = super::super::model_mapping::compile_model_mappings("test", &mappings)
        .expect("hot model mappings compile");
    let upstream = hot_model_test_upstream(rules);

    assert_eq!(
        upstream.map_model("openai/gpt-5.5").as_deref(),
        Some("gpt-5.5")
    );
    assert_eq!(
        upstream
            .map_model("models/gemini-3.1-pro-preview")
            .as_deref(),
        Some("gemini-3.1-pro-preview")
    );
    assert_eq!(
        upstream.map_model("anthropic/claude-sonnet-4.6").as_deref(),
        Some("claude-sonnet-4.6")
    );
    assert_eq!(
        upstream.map_model("qwen/qwen3.6-plus").as_deref(),
        Some("qwen3.6-plus")
    );
}

#[test]
fn manual_model_mappings_override_hot_aliases() {
    let hot_mappings = super::super::hot_model_mappings::default_hot_model_mappings();
    let upstream_mappings = std::collections::HashMap::from([(
        "openai/gpt-5.5".to_string(),
        "vendor-special-gpt-5.5".to_string(),
    )]);
    let mappings = super::super::hot_model_mappings::merge_hot_model_mappings(
        &hot_mappings,
        &upstream_mappings,
    );
    let rules = super::super::model_mapping::compile_model_mappings("test", &mappings)
        .expect("merged mappings compile");
    let upstream = hot_model_test_upstream(rules);

    assert_eq!(
        upstream.map_model("openai/gpt-5.5").as_deref(),
        Some("vendor-special-gpt-5.5")
    );
}

#[test]
fn proxy_config_file_defaults_include_hot_model_mappings() {
    let config = ProxyConfigFile::default();

    assert_eq!(
        config.hot_model_mappings.get("openai/gpt-5.5"),
        Some(&"gpt-5.5".to_string())
    );
}

#[test]
fn test_upstream_url() {
    // openai provider: /v1/chat/completions
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.example.com/openai/v1".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    assert_eq!(
        upstream.upstream_url("/v1/chat/completions"),
        "https://api.example.com/openai/v1/chat/completions"
    );

    // openai-response provider: /v1/responses
    let upstream_responses = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.example.com/openai/v1".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    assert_eq!(
        upstream_responses.upstream_url("/v1/responses"),
        "https://api.example.com/openai/v1/responses"
    );

    let coding_plan = UpstreamRuntime {
        id: "coding-plan".to_string(),
        selector_key: "coding-plan".to_string(),
        base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    assert_eq!(
        coding_plan.upstream_url("/v1/chat/completions"),
        "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
    );

    // 无路径前缀的 base_url
    let upstream_no_path = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    assert_eq!(
        upstream_no_path.upstream_url("/v1/chat/completions"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        upstream_no_path.upstream_url("/v1/responses"),
        "https://api.openai.com/v1/responses"
    );

    // 带尾斜杠的 base_url
    let upstream_trailing_slash = UpstreamRuntime {
        id: "test".to_string(),
        selector_key: "test".to_string(),
        base_url: "https://api.example.com/openai/v1/".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    };
    // openai: /v1/chat/completions
    assert_eq!(
        upstream_trailing_slash.upstream_url("/v1/chat/completions"),
        "https://api.example.com/openai/v1/chat/completions"
    );
    // openai-response: /v1/responses
    assert_eq!(
        upstream_trailing_slash.upstream_url("/v1/responses"),
        "https://api.example.com/openai/v1/responses"
    );
    // anthropic: /v1/messages
    assert_eq!(
        upstream_trailing_slash.upstream_url("/v1/messages"),
        "https://api.example.com/openai/v1/messages"
    );
}

#[test]
fn proxy_config_file_defaults_retryable_failure_cooldown_to_15_seconds() {
    let config = ProxyConfigFile::default();

    assert_eq!(config.retryable_failure_cooldown_secs, 15);
}

#[test]
fn proxy_config_file_defaults_codex_session_scoped_cooldown_disabled() {
    let config = ProxyConfigFile::default();

    assert!(!config.codex_session_scoped_cooldown_enabled);
}

#[test]
fn proxy_config_file_defaults_upstream_strategy_to_fill_first_serial() {
    let config = ProxyConfigFile::default();

    assert_eq!(
        config.upstream_strategy.order,
        UpstreamOrderStrategy::FillFirst
    );
    assert_eq!(
        config.upstream_strategy.dispatch,
        UpstreamDispatchStrategy::Serial
    );
}
