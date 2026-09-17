use axum::http::HeaderMap;

use super::{dispatch::DispatchPlan, CODEX_RESPONSES_PATH};
use crate::proxy::{
    codex_compat,
    config::{InboundApiFormat, ProxyConfig},
    gemini,
    openai_compat::{FormatTransform, CHAT_PATH, RESPONSES_PATH},
};

/// 只枚举确实有协议适配器的生成入口；资源、模型目录等原生端点继续遵守原生能力边界。
pub(super) fn compatible_plans(
    config: &ProxyConfig,
    path: &str,
    headers: &HeaderMap,
) -> Option<Vec<DispatchPlan>> {
    let format = match path {
        CHAT_PATH => InboundApiFormat::OpenaiChat,
        RESPONSES_PATH => InboundApiFormat::OpenaiResponses,
        "/v1/messages" => InboundApiFormat::AnthropicMessages,
        _ if gemini::is_gemini_path(path) => InboundApiFormat::Gemini,
        _ => return None,
    };
    Some(
        [
            "openai",
            "openai-response",
            "codex",
            "xai",
            "anthropic",
            "kiro",
            "gemini",
        ]
        .into_iter()
        .filter(|provider| {
            config
                .provider_upstreams(provider)
                .is_some_and(|upstreams| {
                    upstreams
                        .groups
                        .iter()
                        .flat_map(|group| &group.items)
                        .any(|upstream| upstream.supports_inbound(format))
                })
        })
        .filter_map(|provider| plan_for_format(provider, format, headers))
        .collect(),
    )
}

pub(super) fn plan_for_format(
    provider: &'static str,
    format: InboundApiFormat,
    headers: &HeaderMap,
) -> Option<DispatchPlan> {
    use FormatTransform::*;
    use InboundApiFormat::*;

    let (outbound_path, request_transform, response_transform) = match (format, provider) {
        (OpenaiChat, "openai")
        | (OpenaiResponses, "openai-response" | "xai")
        | (AnthropicMessages, "anthropic")
        | (Gemini, "gemini") => (Option::None, None, None),
        (OpenaiChat, "openai-response" | "xai") => {
            (Some(RESPONSES_PATH), ChatToResponses, ResponsesToChat)
        }
        (OpenaiChat, "codex") => (Some(CODEX_RESPONSES_PATH), ChatToCodex, CodexToChat),
        (OpenaiChat, "anthropic") => (Some("/v1/messages"), ChatToAnthropic, AnthropicToChat),
        (OpenaiChat, "gemini") => (Option::None, ChatToGemini, GeminiToChat),
        (OpenaiResponses, "openai") => (Some(CHAT_PATH), ResponsesToChat, ChatToResponses),
        (OpenaiResponses, "codex") => {
            // 原生 Codex 头的请求和响应必须保持原生合同。
            if codex_compat::is_native_codex_request(headers) {
                (Some(CODEX_RESPONSES_PATH), None, None)
            } else {
                (
                    Some(CODEX_RESPONSES_PATH),
                    ResponsesToCodex,
                    CodexToResponses,
                )
            }
        }
        (OpenaiResponses, "anthropic") => (
            Some("/v1/messages"),
            ResponsesToAnthropic,
            AnthropicToResponses,
        ),
        (OpenaiResponses, "gemini") => (Option::None, ResponsesToGemini, GeminiToResponses),
        (AnthropicMessages, "kiro") => (Some(RESPONSES_PATH), None, KiroToAnthropic),
        (AnthropicMessages, "openai-response" | "xai") => (
            Some(RESPONSES_PATH),
            AnthropicToResponses,
            ResponsesToAnthropic,
        ),
        (AnthropicMessages, "codex") => (
            Some(CODEX_RESPONSES_PATH),
            AnthropicToCodex,
            CodexToAnthropic,
        ),
        (AnthropicMessages, "openai") => (Some(CHAT_PATH), AnthropicToChat, ChatToAnthropic),
        (AnthropicMessages, "gemini") => (Option::None, AnthropicToGemini, GeminiToAnthropic),
        (Gemini, "openai-response" | "xai") => {
            (Some(RESPONSES_PATH), GeminiToResponses, ResponsesToGemini)
        }
        (Gemini, "openai") => (Some(CHAT_PATH), GeminiToChat, ChatToGemini),
        (Gemini, "anthropic") => (Some("/v1/messages"), GeminiToAnthropic, AnthropicToGemini),
        _ => return Option::None,
    };
    Some(DispatchPlan {
        provider,
        outbound_path,
        request_transform,
        response_transform,
    })
}
