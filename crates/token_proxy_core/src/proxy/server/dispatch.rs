use axum::http::HeaderMap;
use url::form_urlencoded;

use crate::proxy::server_helpers::is_anthropic_path;

use super::{
    super::{
        config::{InboundApiFormat, ProxyConfig},
        gemini,
        inbound::detect_inbound_api_format,
        openai,
        openai_compat::{
            FormatTransform, CHAT_PATH, PROVIDER_CHAT, PROVIDER_RESPONSES, RESPONSES_PATH,
        },
        RequestMeta,
    },
    CODEX_RESPONSES_COMPACT_PATH, CODEX_RESPONSES_PATH, ERROR_NO_UPSTREAM, PROVIDER_ANTHROPIC,
    PROVIDER_CODEX, PROVIDER_GEMINI, PROVIDER_KIRO,
};

#[derive(Clone, Copy)]
pub(super) struct DispatchPlan {
    pub(super) provider: &'static str,
    pub(super) outbound_path: Option<&'static str>,
    pub(super) request_transform: FormatTransform,
    pub(super) response_transform: FormatTransform,
}

struct ProviderRank {
    priority: i32,
    min_id: String,
}

const OPENAI_MODELS_PATH: &str = "/v1/models";
const OPENAI_COMPATIBLE_MODELS_PATH: &str = "/v1beta/openai/models";

fn base_plan(provider: &'static str) -> DispatchPlan {
    DispatchPlan {
        provider,
        outbound_path: None,
        request_transform: FormatTransform::None,
        response_transform: FormatTransform::None,
    }
}

fn provider_rank(config: &ProxyConfig, provider: &str) -> Option<ProviderRank> {
    let upstreams = config.provider_upstreams(provider)?;
    let (priority, min_id) = match upstreams.groups.first() {
        Some(group) => {
            let min_id = group
                .items
                .iter()
                .map(|item| item.id.as_str())
                .min()
                .unwrap_or(provider);
            (group.priority, min_id)
        }
        None => (0, provider),
    };
    Some(ProviderRank {
        priority,
        min_id: min_id.to_string(),
    })
}

fn provider_rank_for_inbound(
    config: &ProxyConfig,
    provider: &str,
    inbound_format: Option<InboundApiFormat>,
) -> Option<ProviderRank> {
    let upstreams = config.provider_upstreams(provider)?;
    let Some(inbound_format) = inbound_format else {
        return provider_rank(config, provider);
    };

    for group in &upstreams.groups {
        let mut min_id: Option<&str> = None;
        let mut has_candidate = false;
        for item in &group.items {
            if !item.supports_inbound(inbound_format) {
                continue;
            }
            has_candidate = true;
            min_id = match min_id {
                None => Some(item.id.as_str()),
                Some(current) => Some(std::cmp::min(current, item.id.as_str())),
            };
        }
        if has_candidate {
            return Some(ProviderRank {
                priority: group.priority,
                min_id: min_id.unwrap_or(provider).to_string(),
            });
        }
    }

    None
}

fn choose_provider_by_priority(
    config: &ProxyConfig,
    inbound_format: Option<InboundApiFormat>,
    candidates: &[&'static str],
) -> Option<&'static str> {
    let mut selected: Option<(&'static str, ProviderRank)> = None;
    for candidate in candidates {
        let Some(rank) = provider_rank_for_inbound(config, candidate, inbound_format) else {
            continue;
        };
        match &selected {
            None => selected = Some((*candidate, rank)),
            Some((_, best)) => {
                if rank.priority > best.priority
                    || (rank.priority == best.priority && rank.min_id < best.min_id)
                {
                    selected = Some((*candidate, rank));
                }
            }
        }
    }
    selected.map(|(provider, _)| provider)
}

fn resolve_gemini_plan(config: &ProxyConfig, path: &str) -> Option<Result<DispatchPlan, String>> {
    if !gemini::is_gemini_path(path) {
        return None;
    }
    let inbound_format = Some(InboundApiFormat::Gemini);
    if let Some(selected) = choose_provider_by_priority(config, inbound_format, &[PROVIDER_GEMINI])
    {
        return Some(Ok(base_plan(selected)));
    }
    let fallback = choose_provider_by_priority(
        config,
        inbound_format,
        &[PROVIDER_RESPONSES, PROVIDER_CHAT, PROVIDER_ANTHROPIC],
    );
    let Some(fallback) = fallback else {
        return Some(Err(ERROR_NO_UPSTREAM.to_string()));
    };
    Some(Ok(match fallback {
        PROVIDER_RESPONSES => DispatchPlan {
            provider: PROVIDER_RESPONSES,
            outbound_path: Some(RESPONSES_PATH),
            request_transform: FormatTransform::GeminiToResponses,
            response_transform: FormatTransform::ResponsesToGemini,
        },
        PROVIDER_CHAT => DispatchPlan {
            provider: PROVIDER_CHAT,
            outbound_path: Some(CHAT_PATH),
            request_transform: FormatTransform::GeminiToChat,
            response_transform: FormatTransform::ChatToGemini,
        },
        PROVIDER_ANTHROPIC => DispatchPlan {
            provider: PROVIDER_ANTHROPIC,
            outbound_path: Some("/v1/messages"),
            request_transform: FormatTransform::GeminiToAnthropic,
            response_transform: FormatTransform::AnthropicToGemini,
        },
        _ => base_plan(PROVIDER_RESPONSES),
    }))
}

fn resolve_gemini_native_plan(
    config: &ProxyConfig,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    if !gemini::is_gemini_native_path(path) || gemini::is_gemini_path(path) {
        return None;
    }
    let provider = choose_provider_by_priority(config, None, &[PROVIDER_GEMINI])
        .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
    Some(provider.map(base_plan))
}

fn resolve_openai_native_plan(
    config: &ProxyConfig,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    if openai::is_openai_responses_resource_path(path) {
        let provider =
            choose_provider_by_priority(config, None, &[PROVIDER_RESPONSES, PROVIDER_CHAT])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    if openai::is_openai_native_resource_path(path) {
        let provider =
            choose_provider_by_priority(config, None, &[PROVIDER_CHAT, PROVIDER_RESPONSES])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    None
}

fn resolve_responses_compact_plan(
    config: &ProxyConfig,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    if !openai::is_openai_responses_compact_path(path) {
        return None;
    }
    Some(resolve_responses_native_plan(config))
}

fn resolve_anthropic_plan(
    config: &ProxyConfig,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    if !is_anthropic_path(path) {
        return None;
    }
    let inbound_format = Some(InboundApiFormat::AnthropicMessages);
    if path == "/v1/messages" {
        if let Some(selected) = choose_provider_by_priority(
            config,
            inbound_format,
            &[PROVIDER_ANTHROPIC, PROVIDER_KIRO],
        ) {
            return Some(Ok(match selected {
                PROVIDER_ANTHROPIC => base_plan(PROVIDER_ANTHROPIC),
                PROVIDER_KIRO => DispatchPlan {
                    provider: PROVIDER_KIRO,
                    outbound_path: Some(RESPONSES_PATH),
                    request_transform: FormatTransform::None,
                    response_transform: FormatTransform::KiroToAnthropic,
                },
                _ => base_plan(PROVIDER_ANTHROPIC),
            }));
        }
        let fallback = choose_provider_by_priority(
            config,
            inbound_format,
            &[
                PROVIDER_RESPONSES,
                PROVIDER_CODEX,
                PROVIDER_CHAT,
                PROVIDER_GEMINI,
            ],
        );
        let Some(fallback) = fallback else {
            return Some(Err(ERROR_NO_UPSTREAM.to_string()));
        };
        return Some(Ok(match fallback {
            PROVIDER_RESPONSES => DispatchPlan {
                provider: PROVIDER_RESPONSES,
                outbound_path: Some(RESPONSES_PATH),
                request_transform: FormatTransform::AnthropicToResponses,
                response_transform: FormatTransform::ResponsesToAnthropic,
            },
            PROVIDER_CODEX => DispatchPlan {
                provider: PROVIDER_CODEX,
                outbound_path: Some(CODEX_RESPONSES_PATH),
                request_transform: FormatTransform::AnthropicToCodex,
                response_transform: FormatTransform::CodexToAnthropic,
            },
            PROVIDER_CHAT => DispatchPlan {
                provider: PROVIDER_CHAT,
                outbound_path: Some(CHAT_PATH),
                request_transform: FormatTransform::AnthropicToChat,
                response_transform: FormatTransform::ChatToAnthropic,
            },
            PROVIDER_GEMINI => DispatchPlan {
                provider: PROVIDER_GEMINI,
                outbound_path: None,
                request_transform: FormatTransform::AnthropicToGemini,
                response_transform: FormatTransform::GeminiToAnthropic,
            },
            _ => base_plan(PROVIDER_RESPONSES),
        }));
    }
    if provider_rank_for_inbound(config, PROVIDER_ANTHROPIC, inbound_format).is_some() {
        return Some(Ok(base_plan(PROVIDER_ANTHROPIC)));
    }
    Some(Err(ERROR_NO_UPSTREAM.to_string()))
}

fn resolve_formatless_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    let provider = choose_provider_by_priority(
        config,
        None,
        &[PROVIDER_CHAT, PROVIDER_RESPONSES, PROVIDER_ANTHROPIC],
    )
    .ok_or_else(|| ERROR_NO_UPSTREAM.to_string())?;
    Ok(base_plan(provider))
}

fn is_openai_models_path(path: &str) -> bool {
    path == OPENAI_MODELS_PATH || path.starts_with("/v1/models/")
}

pub(super) fn is_openai_models_index_path(path: &str) -> bool {
    path == OPENAI_MODELS_PATH
}

fn is_openai_compatible_models_path(path: &str) -> bool {
    path == OPENAI_COMPATIBLE_MODELS_PATH || path.starts_with("/v1beta/openai/models/")
}

pub(super) fn is_openai_compatible_models_index_path(path: &str) -> bool {
    path == OPENAI_COMPATIBLE_MODELS_PATH
}

fn is_anthropic_models_request(headers: &HeaderMap) -> bool {
    headers.contains_key("anthropic-version")
        && (headers.contains_key("x-api-key")
            || headers.contains_key("x-anthropic-api-key")
            || headers.contains_key(axum::http::header::AUTHORIZATION))
}

fn is_gemini_models_request(headers: &HeaderMap, query: Option<&str>) -> bool {
    if headers.contains_key("x-goog-api-key") {
        return true;
    }
    let Some(query) = query else {
        return false;
    };
    form_urlencoded::parse(query.as_bytes()).any(|(key, value)| key == "key" && !value.is_empty())
}

fn resolve_models_plan(
    config: &ProxyConfig,
    path: &str,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Option<Result<DispatchPlan, String>> {
    if is_openai_compatible_models_path(path) {
        let provider =
            choose_provider_by_priority(config, None, &[PROVIDER_CHAT, PROVIDER_RESPONSES])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    if is_openai_models_path(path) {
        if is_anthropic_models_request(headers) {
            let provider = choose_provider_by_priority(config, None, &[PROVIDER_ANTHROPIC])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
            return Some(provider.map(base_plan));
        }
        if is_gemini_models_request(headers, query) {
            let provider = choose_provider_by_priority(config, None, &[PROVIDER_GEMINI])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
            return Some(provider.map(base_plan));
        }
        let provider =
            choose_provider_by_priority(config, None, &[PROVIDER_CHAT, PROVIDER_RESPONSES])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    if gemini::is_gemini_model_catalog_path(path) {
        let provider = choose_provider_by_priority(config, None, &[PROVIDER_GEMINI])
            .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    None
}

pub(super) fn resolve_dispatch_plan_with_request(
    config: &ProxyConfig,
    path: &str,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<DispatchPlan, String> {
    if let Some(plan) = resolve_models_plan(config, path, headers, query) {
        return plan;
    }
    if let Some(plan) = resolve_responses_compact_plan(config, path) {
        return plan;
    }
    if let Some(plan) = resolve_openai_native_plan(config, path) {
        return plan;
    }
    if let Some(plan) = resolve_gemini_plan(config, path) {
        return plan;
    }
    if let Some(plan) = resolve_gemini_native_plan(config, path) {
        return plan;
    }
    if let Some(plan) = resolve_anthropic_plan(config, path) {
        return plan;
    }

    let Some(format) = detect_inbound_api_format(path) else {
        return resolve_formatless_plan(config);
    };

    match format {
        InboundApiFormat::OpenaiChat => resolve_chat_plan(config),
        InboundApiFormat::OpenaiResponses => {
            if openai::is_openai_responses_compact_path(path) {
                resolve_responses_native_plan(config)
            } else {
                resolve_responses_plan(config)
            }
        }
        _ => resolve_formatless_plan(config),
    }
}

fn resolve_responses_native_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    let inbound_format = Some(InboundApiFormat::OpenaiResponses);
    if let Some(selected) = choose_provider_by_priority(
        config,
        inbound_format,
        &[PROVIDER_RESPONSES, PROVIDER_CODEX],
    ) {
        return Ok(match selected {
            PROVIDER_RESPONSES => base_plan(PROVIDER_RESPONSES),
            PROVIDER_CODEX => DispatchPlan {
                provider: PROVIDER_CODEX,
                outbound_path: Some(CODEX_RESPONSES_PATH),
                request_transform: FormatTransform::ResponsesCompactToCodex,
                response_transform: FormatTransform::CodexToResponses,
            },
            _ => base_plan(PROVIDER_RESPONSES),
        });
    }
    Err(ERROR_NO_UPSTREAM.to_string())
}

fn resolve_chat_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    let inbound_format = Some(InboundApiFormat::OpenaiChat);
    if provider_rank_for_inbound(config, PROVIDER_CHAT, inbound_format).is_some() {
        return Ok(base_plan(PROVIDER_CHAT));
    }
    let selected = choose_provider_by_priority(
        config,
        inbound_format,
        &[
            PROVIDER_RESPONSES,
            PROVIDER_CODEX,
            PROVIDER_ANTHROPIC,
            PROVIDER_GEMINI,
        ],
    )
    .ok_or_else(|| ERROR_NO_UPSTREAM.to_string())?;

    Ok(match selected {
        PROVIDER_RESPONSES => DispatchPlan {
            provider: PROVIDER_RESPONSES,
            outbound_path: Some(RESPONSES_PATH),
            request_transform: FormatTransform::ChatToResponses,
            response_transform: FormatTransform::ResponsesToChat,
        },
        PROVIDER_ANTHROPIC => DispatchPlan {
            provider: PROVIDER_ANTHROPIC,
            outbound_path: Some("/v1/messages"),
            request_transform: FormatTransform::ChatToAnthropic,
            response_transform: FormatTransform::AnthropicToChat,
        },
        PROVIDER_CODEX => DispatchPlan {
            provider: PROVIDER_CODEX,
            outbound_path: Some(CODEX_RESPONSES_PATH),
            request_transform: FormatTransform::ChatToCodex,
            response_transform: FormatTransform::CodexToChat,
        },
        PROVIDER_GEMINI => DispatchPlan {
            provider: PROVIDER_GEMINI,
            outbound_path: None,
            request_transform: FormatTransform::ChatToGemini,
            response_transform: FormatTransform::GeminiToChat,
        },
        _ => base_plan(PROVIDER_RESPONSES),
    })
}

fn resolve_responses_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    let inbound_format = Some(InboundApiFormat::OpenaiResponses);
    if let Some(selected) = choose_provider_by_priority(
        config,
        inbound_format,
        &[PROVIDER_RESPONSES, PROVIDER_CODEX],
    ) {
        if selected == PROVIDER_RESPONSES {
            return Ok(base_plan(PROVIDER_RESPONSES));
        }
        if selected == PROVIDER_CODEX {
            return Ok(DispatchPlan {
                provider: PROVIDER_CODEX,
                outbound_path: Some(CODEX_RESPONSES_PATH),
                request_transform: FormatTransform::ResponsesToCodex,
                response_transform: FormatTransform::CodexToResponses,
            });
        }
    }

    let selected = choose_provider_by_priority(
        config,
        inbound_format,
        &[PROVIDER_CHAT, PROVIDER_ANTHROPIC, PROVIDER_GEMINI],
    )
    .ok_or_else(|| ERROR_NO_UPSTREAM.to_string())?;
    Ok(match selected {
        PROVIDER_CHAT => DispatchPlan {
            provider: PROVIDER_CHAT,
            outbound_path: Some(CHAT_PATH),
            request_transform: FormatTransform::ResponsesToChat,
            response_transform: FormatTransform::ChatToResponses,
        },
        PROVIDER_ANTHROPIC => DispatchPlan {
            provider: PROVIDER_ANTHROPIC,
            outbound_path: Some("/v1/messages"),
            request_transform: FormatTransform::ResponsesToAnthropic,
            response_transform: FormatTransform::AnthropicToResponses,
        },
        PROVIDER_GEMINI => DispatchPlan {
            provider: PROVIDER_GEMINI,
            outbound_path: None,
            request_transform: FormatTransform::ResponsesToGemini,
            response_transform: FormatTransform::GeminiToResponses,
        },
        _ => base_plan(PROVIDER_CHAT),
    })
}

pub(super) fn resolve_outbound_path(path: &str, plan: &DispatchPlan, meta: &RequestMeta) -> String {
    match (plan.outbound_path, plan.provider) {
        (Some(CODEX_RESPONSES_PATH), PROVIDER_CODEX)
            if openai::is_openai_responses_compact_path(path) =>
        {
            CODEX_RESPONSES_COMPACT_PATH.to_string()
        }
        (Some(outbound_path), _) => outbound_path.to_string(),
        (None, _) if is_openai_compatible_models_path(path) => {
            path.replacen(OPENAI_COMPATIBLE_MODELS_PATH, OPENAI_MODELS_PATH, 1)
        }
        (None, PROVIDER_GEMINI) if is_openai_models_path(path) => {
            path.replacen(OPENAI_MODELS_PATH, "/v1beta/models", 1)
        }
        (None, PROVIDER_GEMINI) if plan.request_transform != FormatTransform::None => {
            let model = meta
                .mapped_model
                .as_deref()
                .or(meta.original_model.as_deref())
                .unwrap_or("gemini-1.5-flash");
            let suffix = if meta.stream {
                ":streamGenerateContent"
            } else {
                ":generateContent"
            };
            format!("{}{}{}", gemini::GEMINI_MODELS_PREFIX, model, suffix)
        }
        (None, _) => path.to_string(),
    }
}

fn build_retry_fallback_plan(path: &str, provider: &'static str) -> Option<DispatchPlan> {
    if path == "/v1/messages" {
        return Some(match provider {
            PROVIDER_ANTHROPIC => base_plan(PROVIDER_ANTHROPIC),
            PROVIDER_KIRO => DispatchPlan {
                provider: PROVIDER_KIRO,
                outbound_path: Some(RESPONSES_PATH),
                request_transform: FormatTransform::None,
                response_transform: FormatTransform::KiroToAnthropic,
            },
            PROVIDER_RESPONSES => DispatchPlan {
                provider: PROVIDER_RESPONSES,
                outbound_path: Some(RESPONSES_PATH),
                request_transform: FormatTransform::AnthropicToResponses,
                response_transform: FormatTransform::ResponsesToAnthropic,
            },
            PROVIDER_CODEX => DispatchPlan {
                provider: PROVIDER_CODEX,
                outbound_path: Some(CODEX_RESPONSES_PATH),
                request_transform: FormatTransform::AnthropicToCodex,
                response_transform: FormatTransform::CodexToAnthropic,
            },
            _ => return None,
        });
    }

    match detect_inbound_api_format(path) {
        Some(InboundApiFormat::OpenaiChat) => match provider {
            PROVIDER_RESPONSES => Some(DispatchPlan {
                provider: PROVIDER_RESPONSES,
                outbound_path: Some(RESPONSES_PATH),
                request_transform: FormatTransform::ChatToResponses,
                response_transform: FormatTransform::ResponsesToChat,
            }),
            PROVIDER_CODEX => Some(DispatchPlan {
                provider: PROVIDER_CODEX,
                outbound_path: Some(CODEX_RESPONSES_PATH),
                request_transform: FormatTransform::ChatToCodex,
                response_transform: FormatTransform::CodexToChat,
            }),
            _ => None,
        },
        Some(InboundApiFormat::OpenaiResponses) => match provider {
            PROVIDER_RESPONSES => Some(base_plan(PROVIDER_RESPONSES)),
            PROVIDER_CODEX => Some(DispatchPlan {
                provider: PROVIDER_CODEX,
                outbound_path: Some(CODEX_RESPONSES_PATH),
                request_transform: if openai::is_openai_responses_compact_path(path) {
                    FormatTransform::ResponsesCompactToCodex
                } else {
                    FormatTransform::ResponsesToCodex
                },
                response_transform: FormatTransform::CodexToResponses,
            }),
            _ => None,
        },
        _ => None,
    }
}

fn resolve_retry_fallback_provider(
    path: &str,
    primary_provider: &str,
) -> Option<(&'static str, Option<InboundApiFormat>)> {
    if path == "/v1/messages" {
        let fallback = match primary_provider {
            PROVIDER_ANTHROPIC => PROVIDER_KIRO,
            PROVIDER_KIRO => PROVIDER_ANTHROPIC,
            PROVIDER_RESPONSES => PROVIDER_CODEX,
            PROVIDER_CODEX => PROVIDER_RESPONSES,
            _ => return None,
        };
        return Some((fallback, Some(InboundApiFormat::AnthropicMessages)));
    }

    match (detect_inbound_api_format(path), primary_provider) {
        (
            Some(InboundApiFormat::OpenaiChat),
            PROVIDER_CHAT | PROVIDER_RESPONSES | PROVIDER_CODEX,
        ) => {
            let fallback = match primary_provider {
                PROVIDER_CHAT => PROVIDER_RESPONSES,
                PROVIDER_RESPONSES => PROVIDER_CODEX,
                PROVIDER_CODEX => PROVIDER_RESPONSES,
                _ => unreachable!("guarded by match arm"),
            };
            Some((fallback, Some(InboundApiFormat::OpenaiChat)))
        }
        (Some(InboundApiFormat::OpenaiResponses), PROVIDER_RESPONSES | PROVIDER_CODEX) => Some((
            if primary_provider == PROVIDER_RESPONSES {
                PROVIDER_CODEX
            } else {
                PROVIDER_RESPONSES
            },
            Some(InboundApiFormat::OpenaiResponses),
        )),
        _ => None,
    }
}

pub(super) fn resolve_retry_fallback_plan(
    config: &ProxyConfig,
    path: &str,
    primary_provider: &str,
) -> Option<DispatchPlan> {
    let (fallback_provider, inbound_format) =
        resolve_retry_fallback_provider(path, primary_provider)?;
    let is_available = match (inbound_format, fallback_provider) {
        (Some(InboundApiFormat::OpenaiChat), PROVIDER_RESPONSES | PROVIDER_CODEX) => {
            config.provider_upstreams(fallback_provider).is_some()
        }
        _ => provider_rank_for_inbound(config, fallback_provider, inbound_format).is_some(),
    };
    if !is_available {
        return None;
    }
    build_retry_fallback_plan(path, fallback_provider)
}

#[cfg(test)]
pub(super) fn resolve_dispatch_plan(
    config: &ProxyConfig,
    path: &str,
) -> Result<DispatchPlan, String> {
    resolve_dispatch_plan_with_request(config, path, &HeaderMap::new(), None)
}
