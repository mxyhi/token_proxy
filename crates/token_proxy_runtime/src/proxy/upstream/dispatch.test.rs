use super::*;

fn runtime(id: &str, available_models: &[&str]) -> UpstreamRuntime {
    UpstreamRuntime {
        id: id.to_string(),
        selector_key: id.to_string(),
        base_url: "https://example.com".to_string(),
        api_key: Some("test-key".to_string()),
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        xai_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        available_models: available_models
            .iter()
            .map(|model| (*model).to_string())
            .collect(),
        advertised_model_ids: Vec::new(),
        model_capabilities: Default::default(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    }
}

#[test]
fn empty_available_models_keeps_upstream_eligible() {
    let items = vec![runtime("all", &[])];

    let eligible = filter_eligible_upstreams(vec![0], &items, None, None, Some("unlisted-model"));

    assert_eq!(eligible, vec![0]);
}

#[test]
fn available_models_filter_upstreams_before_dispatch() {
    let items = vec![
        runtime("gpt", &["gpt-5.4"]),
        runtime("claude", &["claude-sonnet-4.6"]),
    ];

    let eligible =
        filter_eligible_upstreams(vec![0, 1], &items, None, None, Some("claude-sonnet-4.6"));

    assert_eq!(eligible, vec![1]);
}

#[test]
fn prefixed_model_matches_target_upstream_available_models() {
    let items = vec![
        runtime("alpha", &["gpt-5.4"]),
        runtime("beta", &["gpt-5.4"]),
    ];

    let eligible =
        filter_eligible_upstreams(vec![0, 1], &items, None, Some("beta"), Some("beta/gpt-5.4"));

    assert_eq!(eligible, vec![1]);
}
