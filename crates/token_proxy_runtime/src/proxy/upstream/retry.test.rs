use super::*;
use token_proxy_config::UpstreamRuntime;

fn sample_upstream(codex_account_id: Option<&str>) -> UpstreamRuntime {
    UpstreamRuntime {
        id: "codex-u".to_string(),
        selector_key: "codex-u".to_string(),
        base_url: "https://example.com".to_string(),
        api_key: None,
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: codex_account_id.map(str::to_string),
        xai_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        available_models: Vec::new(),
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    }
}

#[test]
fn pin_account_if_missing_fills_empty_codex_slot() {
    // Same-Upstream Retry 必须钉住当前 account_id，禁止换号。
    let mut upstream = sample_upstream(None);
    pin_account_if_missing("codex", &mut upstream, Some("codex-a.json"));
    assert_eq!(upstream.codex_account_id.as_deref(), Some("codex-a.json"));
}

#[test]
fn pin_account_if_missing_keeps_existing_binding() {
    // 已固定的 credential account_id 不可被运行时覆盖。
    let mut upstream = sample_upstream(Some("codex-fixed.json"));
    pin_account_if_missing("codex", &mut upstream, Some("codex-other.json"));
    assert_eq!(
        upstream.codex_account_id.as_deref(),
        Some("codex-fixed.json")
    );
}

#[test]
fn pin_account_if_missing_ignores_blank_candidate() {
    let mut upstream = sample_upstream(None);
    pin_account_if_missing("codex", &mut upstream, Some("  "));
    assert!(upstream.codex_account_id.is_none());
}
