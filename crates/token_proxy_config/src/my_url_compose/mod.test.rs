use super::needs_legacy_version_warn;
use super::EndpointCompose;
use super::UrlComposeConfig;
use crate::types::{ProxyConfigFile, UpstreamConfig};

#[test]
fn url_compose_serializes_kebab_case_keys() {
    let upstream = UpstreamConfig {
        id: "u1".to_string(),
        providers: vec!["openai-response".to_string()],
        base_url: "https://x.com".to_string(),
        credential: crate::types::UpstreamCredential::api_keys(["k"]),
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        available_models: Vec::new(),
        model_mappings: Default::default(),
        convert_from_map: Default::default(),
        overrides: None,
        url_compose: Some(UrlComposeConfig {
            openai_response: Some(EndpointCompose {
                prefix: String::new(),
                suffix: "/v3/responses".to_string(),
            }),
            ..Default::default()
        }),
    };
    let value = serde_json::to_value(&upstream).expect("serialize");
    assert_eq!(value["url_compose"]["openai-response"]["suffix"], "/v3/responses");
    assert!(value["url_compose"].get("openai").is_none());
    assert!(value["url_compose"].get("anthropic").is_none());
}

#[test]
fn legacy_my_conf_fields_are_silently_ignored() {
    let config: ProxyConfigFile = serde_json::from_value(serde_json::json!({
        "host": "127.0.0.1",
        "port": 8118,
        "upstreams": [{
            "id": "u1",
            "providers": ["openai"],
            "base_url": "https://x.com",
            "my_conf_openai": { "openai_compatible_v1_fix": false },
            "my_conf_anthropic": { "anthropic_messages_url_fix": false }
        }]
    }))
    .expect("legacy config with my_conf fields must load");
    assert_eq!(config.upstreams.len(), 1);
    assert!(config.upstreams[0].url_compose.is_none());
}

#[test]
fn url_compose_missing_falls_back_to_identity() {
    let config: ProxyConfigFile = serde_json::from_value(serde_json::json!({
        "host": "127.0.0.1",
        "port": 8118,
        "upstreams": [{
            "id": "u1",
            "providers": ["anthropic"],
            "base_url": "https://x.com",
            "credential": { "type": "api_keys", "api_keys": ["k"] }
        }]
    }))
    .expect("config");
    assert!(config.upstreams[0].url_compose.is_none());
}

#[test]
fn warn_needed_when_fully_unconfigured_and_versioned_base() {
    assert!(needs_legacy_version_warn("https://x.com/v1", None));
    assert!(needs_legacy_version_warn(
        "https://x.com/v1",
        Some(&UrlComposeConfig::default())
    ));
    assert!(needs_legacy_version_warn("https://x.com/v3/", None));
}

#[test]
fn warn_skipped_when_any_family_configured() {
    let compose = UrlComposeConfig {
        openai: Some(EndpointCompose {
            prefix: String::new(),
            suffix: "/v3/chat/completions".to_string(),
        }),
        ..Default::default()
    };
    assert!(!needs_legacy_version_warn("https://x.com/v1", Some(&compose)));
}

#[test]
fn warn_skipped_for_non_version_base() {
    assert!(!needs_legacy_version_warn("https://x.com/api/plan", None));
    assert!(!needs_legacy_version_warn("https://x.com", None));
    assert!(!needs_legacy_version_warn("https://x.com/v", None));
}
