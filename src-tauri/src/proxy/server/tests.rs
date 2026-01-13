use super::*;

use std::collections::HashMap;

use crate::logging::LogLevel;
use crate::proxy::config::{ProviderUpstreams, ProxyConfig, UpstreamStrategy};

fn config_with_providers(
    providers: &[&'static str],
    enable_api_format_conversion: bool,
) -> ProxyConfig {
    let mut upstreams = HashMap::new();
    for provider in providers {
        upstreams.insert((*provider).to_string(), ProviderUpstreams { groups: Vec::new() });
    }
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: None,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 20 * 1024 * 1024,
        enable_api_format_conversion,
        upstream_strategy: UpstreamStrategy::PriorityRoundRobin,
        upstreams,
    }
}

#[test]
fn chat_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[PROVIDER_RESPONSES], false);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert!(error.contains("format conversion is disabled"));

    let config = config_with_providers(&[PROVIDER_RESPONSES], true);
    let plan = resolve_dispatch_plan(&config, CHAT_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToResponses);
    assert_eq!(plan.response_transform, FormatTransform::ResponsesToChat);
}

#[test]
fn responses_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[PROVIDER_CHAT], false);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert!(error.contains("format conversion is disabled"));

    let config = config_with_providers(&[PROVIDER_CHAT], true);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.outbound_path, Some(CHAT_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToChat);
    assert_eq!(plan.response_transform, FormatTransform::ChatToResponses);
}

#[test]
fn gemini_route_requires_gemini_provider() {
    let config = config_with_providers(&[PROVIDER_CHAT], false);
    let error = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn gemini_route_dispatches_to_gemini() {
    let config = config_with_providers(&[PROVIDER_GEMINI], false);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}
