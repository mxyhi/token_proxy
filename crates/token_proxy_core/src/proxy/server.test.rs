use super::*;

use std::collections::HashMap;

use crate::logging::LogLevel;
use crate::proxy::config::{
    InboundApiFormat, ProviderUpstreams, ProxyConfig, UpstreamGroup, UpstreamRuntime,
    UpstreamStrategy,
};

const FORMATS_ALL: &[InboundApiFormat] = &[
    InboundApiFormat::OpenaiChat,
    InboundApiFormat::OpenaiResponses,
    InboundApiFormat::AnthropicMessages,
    InboundApiFormat::Gemini,
];

const FORMATS_CHAT: &[InboundApiFormat] = &[InboundApiFormat::OpenaiChat];
const FORMATS_RESPONSES: &[InboundApiFormat] = &[InboundApiFormat::OpenaiResponses];
const FORMATS_MESSAGES: &[InboundApiFormat] = &[InboundApiFormat::AnthropicMessages];
const FORMATS_GEMINI: &[InboundApiFormat] = &[InboundApiFormat::Gemini];

const FORMATS_KIRO_NATIVE: &[InboundApiFormat] = &[InboundApiFormat::AnthropicMessages];

fn config_with_providers(providers: &[(&'static str, &'static [InboundApiFormat])]) -> ProxyConfig {
    let upstreams: Vec<(&'static str, i32, &'static str, &'static [InboundApiFormat])> = providers
        .iter()
        .map(|(provider, formats)| (*provider, 0, *provider, *formats))
        .collect();
    config_with_upstreams(&upstreams)
}

fn config_with_upstreams(
    upstreams: &[(&'static str, i32, &'static str, &'static [InboundApiFormat])],
) -> ProxyConfig {
    let mut provider_map: HashMap<String, ProviderUpstreams> = HashMap::new();
    for (provider, priority, id, inbound_formats) in upstreams {
        let mut runtime = UpstreamRuntime {
            id: (*id).to_string(),
            base_url: "https://example.com".to_string(),
            api_key: None,
            filter_prompt_cache_retention: false,
            filter_safety_identifier: false,
            kiro_account_id: None,
            codex_account_id: None,
            antigravity_account_id: None,
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: *priority,
            model_mappings: None,
            header_overrides: None,
            allowed_inbound_formats: Default::default(),
        };
        runtime
            .allowed_inbound_formats
            .extend(inbound_formats.iter().copied());
        let entry = provider_map
            .entry((*provider).to_string())
            .or_insert_with(|| ProviderUpstreams { groups: Vec::new() });
        if let Some(group) = entry.groups.iter_mut().find(|group| group.priority == *priority) {
            group.items.push(runtime);
        } else {
            entry.groups.push(UpstreamGroup {
                priority: *priority,
                items: vec![runtime],
            });
        }
    }
    for upstreams in provider_map.values_mut() {
        upstreams.groups.sort_by(|left, right| right.priority.cmp(&left.priority));
    }
    ProxyConfig {
        host: "127.0.0.1".to_string(),
        port: 9208,
        local_api_key: None,
        log_level: LogLevel::Silent,
        max_request_body_bytes: 20 * 1024 * 1024,
        upstream_strategy: UpstreamStrategy::PriorityRoundRobin,
        upstreams: provider_map,
        kiro_preferred_endpoint: None,
        antigravity_user_agent: None,
    }
}

#[test]
fn chat_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_RESPONSES)]);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, CHAT_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToResponses);
    assert_eq!(plan.response_transform, FormatTransform::ResponsesToChat);
}

#[test]
fn chat_does_not_route_to_kiro() {
    let config = config_with_providers(&[(PROVIDER_KIRO, FORMATS_ALL)]);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn responses_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_CHAT)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.outbound_path, Some(CHAT_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToChat);
    assert_eq!(plan.response_transform, FormatTransform::ChatToResponses);
}

#[test]
fn responses_does_not_route_to_kiro() {
    let config = config_with_providers(&[(PROVIDER_KIRO, FORMATS_ALL)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn chat_to_codex_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let error = resolve_dispatch_plan(&config, CHAT_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, CHAT_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ChatToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToChat);
}

#[test]
fn responses_prefers_codex_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_CODEX, FORMATS_RESPONSES)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_CODEX);
    assert_eq!(plan.outbound_path, Some(CODEX_RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToCodex);
    assert_eq!(plan.response_transform, FormatTransform::CodexToResponses);
}

#[test]
fn responses_same_protocol_preferred_over_priority() {
    let config = config_with_upstreams(
        &[
            (PROVIDER_RESPONSES, 0, "resp", FORMATS_RESPONSES),
            (PROVIDER_CHAT, 10, "chat", FORMATS_ALL),
        ],
    );
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn responses_same_protocol_tiebreaks_by_id() {
    let config = config_with_upstreams(
        &[
            (PROVIDER_RESPONSES, 5, "b-resp", FORMATS_RESPONSES),
            (PROVIDER_KIRO, 5, "a-kiro", FORMATS_KIRO_NATIVE),
        ],
    );
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn anthropic_messages_fallback_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_RESPONSES)]);
    let error = resolve_dispatch_plan(&config, "/v1/messages")
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_RESPONSES, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_RESPONSES);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::AnthropicToResponses);
    assert_eq!(plan.response_transform, FormatTransform::ResponsesToAnthropic);
}

#[test]
fn anthropic_messages_fallbacks_to_kiro_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_KIRO, FORMATS_KIRO_NATIVE)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn anthropic_messages_allows_antigravity_without_conversion() {
    let config = config_with_providers(&[(PROVIDER_ANTIGRAVITY, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_ANTIGRAVITY);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::AnthropicToGemini);
    assert_eq!(plan.response_transform, FormatTransform::GeminiToAnthropic);
}

#[test]
fn anthropic_messages_prefers_kiro_without_conversion() {
    let config = config_with_upstreams(
        &[
            (PROVIDER_RESPONSES, 10, "resp", FORMATS_ALL),
            (PROVIDER_KIRO, 0, "kiro", FORMATS_KIRO_NATIVE),
        ],
    );
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn anthropic_messages_prefers_anthropic_when_priority_higher() {
    let config = config_with_upstreams(
        &[
            (PROVIDER_ANTHROPIC, 5, "anthro", FORMATS_MESSAGES),
            (PROVIDER_KIRO, 1, "kiro", FORMATS_KIRO_NATIVE),
        ],
    );
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}

#[test]
fn anthropic_messages_tiebreaks_by_id_between_anthropic_and_kiro() {
    let config = config_with_upstreams(
        &[
            (PROVIDER_ANTHROPIC, 5, "b-anthro", FORMATS_MESSAGES),
            (PROVIDER_KIRO, 5, "a-kiro", FORMATS_KIRO_NATIVE),
        ],
    );
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_KIRO);
    assert_eq!(plan.outbound_path, Some(RESPONSES_PATH));
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::KiroToAnthropic);
}

#[test]
fn responses_fallback_to_anthropic_requires_format_conversion_enabled() {
    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_MESSAGES)]);
    let error = resolve_dispatch_plan(&config, RESPONSES_PATH)
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");

    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, RESPONSES_PATH).expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
    assert_eq!(plan.outbound_path, Some("/v1/messages"));
    assert_eq!(plan.request_transform, FormatTransform::ResponsesToAnthropic);
    assert_eq!(plan.response_transform, FormatTransform::AnthropicToResponses);
}

#[test]
fn gemini_route_requires_format_conversion_for_fallback() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_CHAT)]);
    let error = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .err()
        .expect("should reject");
    assert_eq!(error, "No available upstream configured.");
}

#[test]
fn gemini_route_fallbacks_to_chat() {
    let config = config_with_providers(&[(PROVIDER_CHAT, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_CHAT);
    assert_eq!(plan.outbound_path, Some(CHAT_PATH));
    assert_eq!(plan.request_transform, FormatTransform::GeminiToChat);
    assert_eq!(plan.response_transform, FormatTransform::ChatToGemini);
}

#[test]
fn gemini_route_fallbacks_to_anthropic() {
    let config = config_with_providers(&[(PROVIDER_ANTHROPIC, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_ANTHROPIC);
    assert_eq!(plan.outbound_path, Some("/v1/messages"));
    assert_eq!(plan.request_transform, FormatTransform::GeminiToAnthropic);
    assert_eq!(plan.response_transform, FormatTransform::AnthropicToGemini);
}

#[test]
fn anthropic_messages_fallbacks_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_ALL)]);
    let plan = resolve_dispatch_plan(&config, "/v1/messages").expect("should fallback");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.outbound_path, None);
    assert_eq!(plan.request_transform, FormatTransform::AnthropicToGemini);
    assert_eq!(plan.response_transform, FormatTransform::GeminiToAnthropic);
}

#[test]
fn gemini_route_dispatches_to_gemini() {
    let config = config_with_providers(&[(PROVIDER_GEMINI, FORMATS_GEMINI)]);
    let plan = resolve_dispatch_plan(&config, "/v1beta/models/gemini-1.5-flash:generateContent")
        .expect("should dispatch");
    assert_eq!(plan.provider, PROVIDER_GEMINI);
    assert_eq!(plan.request_transform, FormatTransform::None);
    assert_eq!(plan.response_transform, FormatTransform::None);
}
