#![allow(clippy::field_reassign_with_default)]

use super::*;
use std::collections::HashMap;
use std::time::Duration;

fn sample_upstream(
    id: &str,
    providers: &[&str],
    base_url: &str,
    credential: UpstreamCredential,
) -> UpstreamConfig {
    UpstreamConfig {
        id: id.to_string(),
        providers: providers.iter().map(|value| (*value).to_string()).collect(),
        base_url: base_url.to_string(),
        credential,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        available_models: Vec::new(),
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }
}

#[test]
fn build_runtime_config_adds_new_default_hot_mapping_to_saved_overrides() {
    let mut config = ProxyConfigFile::default();
    // 模拟旧版本已保存的配置：字段存在，但尚未包含后来新增的默认 alias。
    config.hot_model_mappings =
        HashMap::from([("custom/alias".to_string(), "custom-target".to_string())]);

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(
        runtime.hot_model_mappings.get("composer-2.5"),
        Some(&"grok-composer-2.5-fast".to_string())
    );
    assert_eq!(
        runtime.hot_model_mappings.get("custom/alias"),
        Some(&"custom-target".to_string())
    );
}

#[test]
fn build_runtime_config_keeps_user_override_for_default_hot_mapping() {
    let mut config = ProxyConfigFile::default();
    config.hot_model_mappings = HashMap::from([(
        "composer-2.5".to_string(),
        "vendor-composer-2.5".to_string(),
    )]);

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(
        runtime.hot_model_mappings.get("composer-2.5"),
        Some(&"vendor-composer-2.5".to_string())
    );
}

#[test]
fn build_runtime_config_rejects_retryable_failure_cooldown_that_overflows_instant() {
    let mut config = ProxyConfigFile::default();
    config.retryable_failure_cooldown_secs = u64::MAX;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_maps_same_upstream_retry_count() {
    let mut config = ProxyConfigFile::default();
    config.same_upstream_retry_count = 3;

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(runtime.same_upstream_retry_count, 3);
}

#[test]
fn build_runtime_config_rejects_same_upstream_retry_count_above_max() {
    let mut config = ProxyConfigFile::default();
    config.same_upstream_retry_count = 6;

    let result = build_runtime_config(config);

    match result {
        Ok(_) => panic!("same_upstream_retry_count above max should be rejected"),
        Err(message) => assert!(message.contains("same_upstream_retry_count")),
    }
}

#[test]
fn build_runtime_config_routes_openai_responses_via_chat_when_enabled() {
    let mut config = ProxyConfigFile::default();
    let mut upstream = sample_upstream(
        "glm-coding-plan",
        &["openai-response"],
        "https://open.bigmodel.cn/api/coding/paas/v4",
        UpstreamCredential::api_keys(["test-key"]),
    );
    upstream.use_chat_completions_for_responses = true;
    config.upstreams = vec![upstream];

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
    config.upstreams = vec![sample_upstream(
        "glm-coding-plan",
        &["openai-response"],
        "https://open.bigmodel.cn/api/coding/paas/v4",
        UpstreamCredential::api_keys(["test-key"]),
    )];

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
fn build_runtime_config_normalizes_available_models() {
    let mut config = ProxyConfigFile::default();
    let mut upstream = sample_upstream(
        "model-limited",
        &["openai"],
        "https://api.openai.com",
        UpstreamCredential::api_keys(["test-key"]),
    );
    upstream.available_models = vec![
        " gpt-5.4-mini ".to_string(),
        String::new(),
        "gpt-5.4".to_string(),
        "gpt-5.4".to_string(),
    ];
    config.upstreams = vec![upstream];

    let runtime = build_runtime_config(config).expect("runtime config");
    let item = runtime
        .provider_upstreams("openai")
        .and_then(|upstreams| upstreams.groups.first())
        .and_then(|group| group.items.first())
        .expect("runtime item");

    assert_eq!(item.available_models, vec!["gpt-5.4", "gpt-5.4-mini"]);
    assert_eq!(item.advertised_model_ids, item.available_models);
    assert!(item.supports_model(Some("gpt-5.4")));
    assert!(!item.supports_model(Some("gpt-4.1")));
}

#[test]
fn build_runtime_config_codex_accepts_chat_and_responses_by_default() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "codex-account",
        &["codex"],
        "",
        UpstreamCredential::account(AccountProvider::Codex, "codex-acc"),
    )];

    let runtime = build_runtime_config(config).expect("runtime config");
    let codex = runtime
        .provider_upstreams("codex")
        .expect("codex runtime upstream");
    let item = codex
        .groups
        .first()
        .and_then(|group| group.items.first())
        .expect("runtime item");

    assert!(item.supports_inbound(InboundApiFormat::OpenaiChat));
    assert!(item.supports_inbound(InboundApiFormat::OpenaiResponses));
    assert_eq!(item.codex_account_id.as_deref(), Some("codex-acc"));
}

fn xai_upstream(base_url: &str) -> UpstreamConfig {
    sample_upstream(
        "xai-default",
        &["xai"],
        base_url,
        UpstreamCredential::account(AccountProvider::Xai, "xai-user@example.com"),
    )
}

#[test]
fn build_runtime_config_xai_uses_trusted_cli_endpoint_and_all_text_formats() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![xai_upstream("")];

    let runtime = build_runtime_config(config).expect("runtime config");
    let item = runtime
        .provider_upstreams("xai")
        .and_then(|upstreams| upstreams.groups.first())
        .and_then(|group| group.items.first())
        .expect("xai runtime item");

    assert_eq!(item.base_url, token_proxy_account_xai::CLI_BASE_URL);
    assert_eq!(item.xai_account_id.as_deref(), Some("xai-user@example.com"));
    assert!(item.supports_inbound(InboundApiFormat::OpenaiChat));
    assert!(item.supports_inbound(InboundApiFormat::OpenaiResponses));
    assert!(item.supports_inbound(InboundApiFormat::AnthropicMessages));
    assert!(item.supports_inbound(InboundApiFormat::Gemini));
}

#[test]
fn build_runtime_config_rejects_untrusted_xai_base_url() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![xai_upstream("https://example.com/v1")];

    let error = build_runtime_config(config)
        .err()
        .expect("custom xai base URL must fail");

    assert!(error.contains("xAI OAuth base_url"));
}

#[test]
fn build_runtime_config_rejects_api_key_for_xai_oauth_provider() {
    let mut config = ProxyConfigFile::default();
    let mut upstream = xai_upstream("");
    upstream.credential = UpstreamCredential::api_keys(["not-an-oauth-account"]);
    config.upstreams = vec![upstream];

    let error = build_runtime_config(config)
        .err()
        .expect("xai API key must fail");

    assert!(error.contains("does not accept api_keys"));
}

#[test]
fn build_runtime_config_rejects_foreign_account_binding_for_xai() {
    let mut config = ProxyConfigFile::default();
    let mut upstream = xai_upstream("");
    // 旧版可同时写多个 *_account_id；新联合类型用 provider mismatch 表达同类冲突。
    upstream.credential = UpstreamCredential::account(AccountProvider::Codex, "codex-account");
    config.upstreams = vec![upstream];

    let error = build_runtime_config(config)
        .err()
        .expect("foreign account binding must fail");

    assert!(error.contains("codex_account_id requires provider codex"));
}

#[test]
fn build_runtime_config_rejects_xai_account_binding_for_other_provider() {
    let mut config = ProxyConfigFile::default();
    let mut upstream = xai_upstream("https://api.openai.com/v1");
    upstream.providers = vec!["openai".to_string()];
    config.upstreams = vec![upstream];

    let error = build_runtime_config(config)
        .err()
        .expect("xai binding on openai must fail");

    assert!(error.contains("xai_account_id requires provider xai"));
}

#[test]
fn build_runtime_config_projects_kiro_account_credential() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "kiro-bound",
        &["kiro"],
        "",
        UpstreamCredential::account(AccountProvider::Kiro, "kiro-acc"),
    )];

    let runtime = build_runtime_config(config).expect("runtime config");
    let item = runtime
        .provider_upstreams("kiro")
        .and_then(|upstreams| upstreams.groups.first())
        .and_then(|group| group.items.first())
        .expect("kiro runtime item");

    assert_eq!(item.kiro_account_id.as_deref(), Some("kiro-acc"));
    assert!(item.codex_account_id.is_none());
    assert!(item.xai_account_id.is_none());
    assert!(item.api_key.is_none());
}

#[test]
fn build_runtime_config_projects_codex_account_credential() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "codex-bound",
        &["codex"],
        "",
        UpstreamCredential::account(AccountProvider::Codex, "codex-acc"),
    )];

    let runtime = build_runtime_config(config).expect("runtime config");
    let item = runtime
        .provider_upstreams("codex")
        .and_then(|upstreams| upstreams.groups.first())
        .and_then(|group| group.items.first())
        .expect("codex runtime item");

    assert_eq!(item.codex_account_id.as_deref(), Some("codex-acc"));
    assert!(item.kiro_account_id.is_none());
    assert!(item.xai_account_id.is_none());
}

#[test]
fn build_runtime_config_passthrough_credential_has_no_api_key_or_account() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "openai-pass",
        &["openai"],
        "https://api.openai.com",
        UpstreamCredential::Passthrough,
    )];

    let runtime = build_runtime_config(config).expect("runtime config");
    let item = runtime
        .provider_upstreams("openai")
        .and_then(|upstreams| upstreams.groups.first())
        .and_then(|group| group.items.first())
        .expect("openai runtime item");

    assert!(item.api_key.is_none());
    assert!(item.kiro_account_id.is_none());
    assert!(item.codex_account_id.is_none());
    assert!(item.xai_account_id.is_none());
}

#[test]
fn build_runtime_config_rejects_duplicate_account_binding() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![
        sample_upstream(
            "kiro-a",
            &["kiro"],
            "",
            UpstreamCredential::account(AccountProvider::Kiro, "same-acc"),
        ),
        sample_upstream(
            "kiro-b",
            &["kiro"],
            "",
            UpstreamCredential::account(AccountProvider::Kiro, "same-acc"),
        ),
    ];

    let error = build_runtime_config(config)
        .err()
        .expect("duplicate account binding must fail");

    // 错误须含安全标识，不得夹带 api key 等 secret。
    assert!(error.contains("kiro"));
    assert!(error.contains("same-acc"));
    assert!(error.contains("kiro-a"));
    assert!(error.contains("kiro-b"));
    assert!(!error.contains("sk-"));
}

#[test]
fn build_runtime_config_allows_same_account_id_across_different_providers() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![
        sample_upstream(
            "kiro-shared-id",
            &["kiro"],
            "",
            UpstreamCredential::account(AccountProvider::Kiro, "shared"),
        ),
        sample_upstream(
            "codex-shared-id",
            &["codex"],
            "",
            UpstreamCredential::account(AccountProvider::Codex, "shared"),
        ),
    ];

    build_runtime_config(config).expect("same account_id across providers is ok");
}

#[test]
fn build_runtime_config_duplicate_account_binding_does_not_affect_api_key_or_passthrough() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![
        sample_upstream(
            "openai-key",
            &["openai"],
            "https://api.openai.com/v1",
            UpstreamCredential::api_keys(["sk-test"]),
        ),
        sample_upstream(
            "openai-pass",
            &["openai"],
            "https://api.openai.com/v1",
            UpstreamCredential::Passthrough,
        ),
        sample_upstream(
            "kiro-only",
            &["kiro"],
            "",
            UpstreamCredential::account(AccountProvider::Kiro, "kiro-1"),
        ),
    ];

    let runtime = build_runtime_config(config).expect("mixed upstreams ok");
    assert!(runtime.provider_upstreams("openai").is_some());
    assert!(runtime.provider_upstreams("kiro").is_some());
}

#[test]
fn upstream_config_serialization_omits_legacy_flat_credential_fields() {
    let upstream = sample_upstream(
        "ser",
        &["openai"],
        "https://example.com",
        UpstreamCredential::api_keys(["k1"]),
    );
    let value = serde_json::to_value(&upstream).expect("serialize");
    let obj = value.as_object().expect("object");
    assert!(obj.contains_key("credential"));
    assert!(!obj.contains_key("api_keys"));
    assert!(!obj.contains_key("kiro_account_id"));
    assert!(!obj.contains_key("codex_account_id"));
    assert!(!obj.contains_key("xai_account_id"));
}

#[test]
fn build_runtime_config_maps_stream_first_output_timeout_secs() {
    let mut config = ProxyConfigFile::default();
    config.stream_first_output_timeout_secs = 3;

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(runtime.stream_first_output_timeout, Duration::from_secs(3));
}

#[test]
fn build_runtime_config_maps_sync_response_timeout_secs() {
    let mut config = ProxyConfigFile::default();
    config.sync_response_timeout_secs = 30;

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(runtime.sync_response_timeout, Duration::from_secs(30));
}

#[test]
fn build_runtime_config_maps_split_timeout_defaults() {
    let runtime = build_runtime_config(ProxyConfigFile::default()).expect("runtime config");

    assert_eq!(runtime.stream_first_output_timeout, Duration::from_secs(60));
    assert_eq!(runtime.sync_response_timeout, Duration::from_secs(300));
}

#[test]
fn build_runtime_config_maps_codex_session_scoped_cooldown_switch() {
    let mut config = ProxyConfigFile::default();
    config.codex_session_scoped_cooldown_enabled = true;

    let runtime = build_runtime_config(config).expect("runtime config");

    assert!(runtime.codex_session_scoped_cooldown_enabled);
}

#[test]
fn build_runtime_config_maps_xai_x_search_injection_switch() {
    let mut config = ProxyConfigFile::default();
    config.xai_inject_x_search = true;

    let runtime = build_runtime_config(config).expect("runtime config");

    assert!(runtime.xai_inject_x_search);
}

#[test]
fn build_runtime_config_maps_hedged_strategy() {
    let mut config = ProxyConfigFile::default();
    config.upstream_strategy = UpstreamStrategy {
        order: UpstreamOrderStrategy::RoundRobin,
        dispatch: UpstreamDispatchStrategy::Hedged {
            delay_ms: 250,
            max_parallel: 3,
        },
    };

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(
        runtime.upstream_strategy.order,
        UpstreamOrderStrategy::RoundRobin
    );
    assert_eq!(
        runtime.upstream_strategy.dispatch,
        UpstreamDispatchRuntime::Hedged {
            delay: Duration::from_millis(250),
            max_parallel: 3,
        }
    );
}

#[test]
fn build_runtime_config_maps_race_strategy() {
    let mut config = ProxyConfigFile::default();
    config.upstream_strategy = UpstreamStrategy {
        order: UpstreamOrderStrategy::RoundRobin,
        dispatch: UpstreamDispatchStrategy::Race { max_parallel: 4 },
    };

    let runtime = build_runtime_config(config).expect("runtime config");

    assert_eq!(
        runtime.upstream_strategy.order,
        UpstreamOrderStrategy::RoundRobin
    );
    assert_eq!(
        runtime.upstream_strategy.dispatch,
        UpstreamDispatchRuntime::Race { max_parallel: 4 }
    );
}

#[test]
fn build_runtime_config_rejects_hedged_strategy_with_zero_delay() {
    let mut config = ProxyConfigFile::default();
    config.upstream_strategy = UpstreamStrategy {
        order: UpstreamOrderStrategy::FillFirst,
        dispatch: UpstreamDispatchStrategy::Hedged {
            delay_ms: 0,
            max_parallel: 2,
        },
    };

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_hedged_strategy_with_max_parallel_below_two() {
    let mut config = ProxyConfigFile::default();
    config.upstream_strategy = UpstreamStrategy {
        order: UpstreamOrderStrategy::FillFirst,
        dispatch: UpstreamDispatchStrategy::Hedged {
            delay_ms: 250,
            max_parallel: 1,
        },
    };

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_race_strategy_with_max_parallel_below_two() {
    let mut config = ProxyConfigFile::default();
    config.upstream_strategy = UpstreamStrategy {
        order: UpstreamOrderStrategy::FillFirst,
        dispatch: UpstreamDispatchStrategy::Race { max_parallel: 1 },
    };

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_stream_first_output_timeout_below_minimum() {
    let mut config = ProxyConfigFile::default();
    config.stream_first_output_timeout_secs = 0;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_sync_response_timeout_below_minimum() {
    let mut config = ProxyConfigFile::default();
    config.sync_response_timeout_secs = 0;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_stream_first_output_timeout_that_overflows_instant() {
    let mut config = ProxyConfigFile::default();
    config.stream_first_output_timeout_secs = u64::MAX;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_sync_response_timeout_that_overflows_instant() {
    let mut config = ProxyConfigFile::default();
    config.sync_response_timeout_secs = u64::MAX;

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_expands_multiple_api_keys_into_multiple_runtime_upstreams() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "shared-openai",
        &["openai"],
        "https://api.openai.com",
        UpstreamCredential::api_keys(["key-a", "key-b"]),
    )];

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
fn build_runtime_config_rejects_api_key_that_cannot_be_precompiled_as_header() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "bad-openai",
        &["openai"],
        "https://api.openai.com",
        UpstreamCredential::api_keys(["bad\nkey"]),
    )];

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_unsupported_provider() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "removed-provider",
        &["legacy-provider"],
        "",
        UpstreamCredential::Passthrough,
    )];

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_multiple_api_keys_for_account_based_provider() {
    let mut config = ProxyConfigFile::default();
    // 旧 fixture 同时写 api_keys + codex_account_id；新联合改为 api_keys 作用于账户型 provider。
    config.upstreams = vec![sample_upstream(
        "codex-account",
        &["codex"],
        "",
        UpstreamCredential::api_keys(["key-a", "key-b"]),
    )];

    let result = build_runtime_config(config);

    assert!(result.is_err());
}

#[test]
fn build_runtime_config_rejects_passthrough_for_kiro() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "kiro-default",
        &["kiro"],
        "",
        UpstreamCredential::Passthrough,
    )];

    let error = build_runtime_config(config)
        .err()
        .expect("kiro passthrough must fail");
    assert!(error.contains("account-based providers require account credential"));
}

#[test]
fn build_runtime_config_rejects_passthrough_for_codex() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "codex-default",
        &["codex"],
        "",
        UpstreamCredential::Passthrough,
    )];

    let error = build_runtime_config(config)
        .err()
        .expect("codex passthrough must fail");
    assert!(error.contains("account-based providers require account credential"));
}

#[test]
fn build_runtime_config_rejects_passthrough_for_xai() {
    let mut config = ProxyConfigFile::default();
    config.upstreams = vec![sample_upstream(
        "xai-default",
        &["xai"],
        "",
        UpstreamCredential::Passthrough,
    )];

    let error = build_runtime_config(config)
        .err()
        .expect("xai passthrough must fail");
    assert!(error.contains("account-based providers require account credential"));
}
