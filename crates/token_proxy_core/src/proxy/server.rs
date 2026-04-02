use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use std::{sync::Arc, time::Instant};
use tokio::sync::RwLock;
use url::form_urlencoded;

use super::{
    config::{InboundApiFormat, ProxyConfig},
    gemini, http,
    inbound::detect_inbound_api_format,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai_compat::{
        FormatTransform, CHAT_PATH, PROVIDER_CHAT, PROVIDER_RESPONSES, RESPONSES_PATH,
    },
    request_body::ReplayableBody,
    request_detail::{capture_request_detail, serialize_request_headers, RequestDetailSnapshot},
    server_helpers::{
        extract_request_path, is_anthropic_path, log_debug_request,
        maybe_force_openai_stream_options_include_usage, maybe_transform_request_body,
        parse_request_meta_best_effort,
    },
    upstream::{aggregate_model_catalog_request, forward_upstream_request},
    ProxyState, RequestMeta,
};
use crate::logging::LogLevel;

const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_GEMINI: &str = "gemini";
const PROVIDER_KIRO: &str = "kiro";
const PROVIDER_CODEX: &str = "codex";
const PROVIDER_PROXY: &str = "proxy";
const LOCAL_UPSTREAM_ID: &str = "local";
const CODEX_RESPONSES_PATH: &str = "/responses";

type ProxyStateHandle = Arc<RwLock<Arc<ProxyState>>>;

mod bootstrap;
pub(crate) use bootstrap::{build_router, build_upstream_cursors};

struct DispatchPlan {
    provider: &'static str,
    outbound_path: Option<&'static str>,
    request_transform: FormatTransform,
    response_transform: FormatTransform,
}

struct PreparedRequest {
    path: String,
    outbound_path_with_query: String,
    plan: DispatchPlan,
    meta: RequestMeta,
    request_detail: Option<RequestDetailSnapshot>,
    source_body: ReplayableBody,
    outbound_body: ReplayableBody,
    request_auth: http::RequestAuth,
}

struct InboundRequest {
    path: String,
    plan: DispatchPlan,
    body: ReplayableBody,
    meta: RequestMeta,
    request_detail: Option<RequestDetailSnapshot>,
}

const ERROR_NO_UPSTREAM: &str = "No available upstream configured.";

fn base_plan(provider: &'static str) -> DispatchPlan {
    DispatchPlan {
        provider,
        outbound_path: None,
        request_transform: FormatTransform::None,
        response_transform: FormatTransform::None,
    }
}

struct ProviderRank {
    priority: i32,
    min_id: String,
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

fn resolve_anthropic_plan(
    config: &ProxyConfig,
    path: &str,
) -> Option<Result<DispatchPlan, String>> {
    if !is_anthropic_path(path) {
        return None;
    }
    let inbound_format = Some(InboundApiFormat::AnthropicMessages);
    if path == "/v1/messages" {
        // Claude Code uses /v1/messages. Prefer native providers (Anthropic/Kiro) by priority.
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
    path == "/v1/models" || path.starts_with("/v1/models/")
}

fn is_openai_models_index_path(path: &str) -> bool {
    path == "/v1/models"
}

fn is_openai_compatible_models_path(path: &str) -> bool {
    path == "/v1beta/openai/models" || path.starts_with("/v1beta/openai/models/")
}

fn is_openai_compatible_models_index_path(path: &str) -> bool {
    path == "/v1beta/openai/models"
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

fn is_gemini_model_catalog_path(path: &str) -> bool {
    if path == "/v1beta/models" {
        return true;
    }
    let Some(rest) = path.strip_prefix("/v1beta/models/") else {
        return false;
    };
    !rest.is_empty() && !rest.contains(':')
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
        // `/v1/models` 属于 OpenAI-compatible 模型目录路由。
        // 这里显式只在 OpenAI-compatible provider 中选择，避免被更高优先级的
        // Anthropic provider 误吞掉。
        let provider =
            choose_provider_by_priority(config, None, &[PROVIDER_CHAT, PROVIDER_RESPONSES])
                .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    if is_gemini_model_catalog_path(path) {
        // Gemini 的模型目录路由与 `:generateContent` 主调用路径不同；
        // 需要在 path 层显式识别，不能继续走 formatless fallback。
        let provider = choose_provider_by_priority(config, None, &[PROVIDER_GEMINI])
            .ok_or_else(|| ERROR_NO_UPSTREAM.to_string());
        return Some(provider.map(base_plan));
    }
    None
}

fn resolve_dispatch_plan_with_request(
    config: &ProxyConfig,
    path: &str,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<DispatchPlan, String> {
    if let Some(plan) = resolve_models_plan(config, path, headers, query) {
        return plan;
    }
    if let Some(plan) = resolve_gemini_plan(config, path) {
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
        InboundApiFormat::OpenaiResponses => resolve_responses_plan(config),
        _ => resolve_formatless_plan(config),
    }
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
            outbound_path: None, // Gemini 路径需要在 upstream 层根据 model 动态构建
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

#[cfg(test)]
fn resolve_dispatch_plan(config: &ProxyConfig, path: &str) -> Result<DispatchPlan, String> {
    resolve_dispatch_plan_with_request(config, path, &HeaderMap::new(), None)
}

fn resolve_outbound_path(path: &str, plan: &DispatchPlan, meta: &RequestMeta) -> String {
    match (plan.outbound_path, plan.provider) {
        (Some(outbound_path), _) => outbound_path.to_string(),
        (None, _) if is_openai_compatible_models_path(path) => {
            path.replacen("/v1beta/openai/models", "/v1/models", 1)
        }
        (None, PROVIDER_GEMINI) if is_openai_models_path(path) => {
            path.replacen("/v1/models", "/v1beta/models", 1)
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
                request_transform: FormatTransform::ResponsesToCodex,
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
        (Some(InboundApiFormat::OpenaiChat), PROVIDER_RESPONSES | PROVIDER_CODEX) => Some((
            if primary_provider == PROVIDER_RESPONSES {
                PROVIDER_CODEX
            } else {
                PROVIDER_RESPONSES
            },
            Some(InboundApiFormat::OpenaiChat),
        )),
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

fn resolve_retry_fallback_plan(
    config: &ProxyConfig,
    path: &str,
    primary_provider: &str,
) -> Option<DispatchPlan> {
    // 跨 provider fallback 需要重新构建目标 provider 的 dispatch plan：
    // chat/responses 到 `openai-response` / `codex` 的 request_transform 不同，
    // 不能复用主请求已经变换过的计划或 payload。
    let (fallback_provider, inbound_format) =
        resolve_retry_fallback_provider(path, primary_provider)?;
    if provider_rank_for_inbound(config, fallback_provider, inbound_format).is_none() {
        return None;
    }
    build_retry_fallback_plan(path, fallback_provider)
}

async fn forward_retry_fallback_request(
    state: Arc<ProxyState>,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
    plan: &DispatchPlan,
) -> Result<super::upstream::ForwardUpstreamResult, Response> {
    let outbound_path = resolve_outbound_path(&prepared.path, plan, &prepared.meta);
    let outbound_path_with_query = build_outbound_path_with_query(&outbound_path, uri);
    let outbound_body = build_outbound_body_or_respond(
        &state.http_clients,
        &state.log,
        prepared.request_detail.clone(),
        &prepared.path,
        plan,
        &prepared.meta,
        prepared.source_body.clone(),
        request_start,
    )
    .await?;
    Ok(forward_upstream_request(
        state,
        method,
        plan.provider,
        &prepared.path,
        &outbound_path_with_query,
        headers,
        &outbound_body,
        &prepared.meta,
        &prepared.request_auth,
        plan.response_transform,
        prepared.request_detail.clone(),
    )
    .await)
}

async fn capture_detail_from_body(
    headers: &HeaderMap,
    body: Body,
    max_body_bytes: usize,
) -> RequestDetailSnapshot {
    match ReplayableBody::from_body(body).await {
        Ok(replayable) => capture_request_detail(headers, &replayable, max_body_bytes).await,
        Err(err) => RequestDetailSnapshot {
            request_headers: serialize_request_headers(headers),
            request_body: Some(format!("Failed to read request body: {err}")),
        },
    }
}

fn log_request_error(
    log: &Arc<LogWriter>,
    detail: Option<RequestDetailSnapshot>,
    path: &str,
    provider: &str,
    upstream_id: &str,
    status: StatusCode,
    response_error: String,
    start: Instant,
) {
    let (request_headers, request_body) = detail
        .map(|detail| (detail.request_headers, detail.request_body))
        .unwrap_or((None, None));
    let context = LogContext {
        path: path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
        account_id: None,
        model: None,
        mapped_model: None,
        stream: false,
        status: status.as_u16(),
        upstream_request_id: None,
        request_headers,
        request_body,
        ttfb_ms: None,
        start,
    };
    let usage = UsageSnapshot {
        usage: None,
        cached_tokens: None,
        usage_json: None,
    };
    let entry = build_log_entry(&context, usage, Some(response_error));
    log.clone().write_detached(entry);
}

async fn ensure_local_auth_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
    path: &str,
    query: Option<&str>,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<Body, Response> {
    if let Err(message) = http::ensure_local_auth(config, headers, path, query) {
        tracing::warn!("local auth failed");
        let detail = if capture_request_detail_enabled {
            Some(capture_detail_from_body(headers, body, max_body_bytes).await)
        } else {
            None
        };
        log_request_error(
            log,
            detail,
            path,
            PROVIDER_PROXY,
            LOCAL_UPSTREAM_ID,
            StatusCode::UNAUTHORIZED,
            message.clone(),
            request_start,
        );
        return Err(http::error_response(StatusCode::UNAUTHORIZED, message));
    }
    Ok(body)
}

async fn resolve_plan_or_respond(
    config: &ProxyConfig,
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
    path: &str,
    query: Option<&str>,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<(DispatchPlan, Body), Response> {
    match resolve_dispatch_plan_with_request(config, path, headers, query) {
        Ok(plan) => {
            tracing::debug!(provider = %plan.provider, "dispatch plan resolved");
            Ok((plan, body))
        }
        Err(message) => {
            tracing::warn!("no dispatch plan found");
            let detail = if capture_request_detail_enabled {
                Some(capture_detail_from_body(headers, body, max_body_bytes).await)
            } else {
                None
            };
            log_request_error(
                log,
                detail,
                path,
                PROVIDER_PROXY,
                LOCAL_UPSTREAM_ID,
                StatusCode::BAD_GATEWAY,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::BAD_GATEWAY, message))
        }
    }
}

async fn read_body_or_respond(
    log: &Arc<LogWriter>,
    headers: &HeaderMap,
    body: Body,
    capture_request_detail_enabled: bool,
    path: &str,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match ReplayableBody::from_body(body).await {
        Ok(body) => Ok(body),
        Err(err) => {
            let message = format!("Failed to read request body: {err}");
            let detail = if capture_request_detail_enabled {
                Some(RequestDetailSnapshot {
                    request_headers: serialize_request_headers(headers),
                    request_body: Some(message.clone()),
                })
            } else {
                None
            };
            log_request_error(
                log,
                detail,
                path,
                PROVIDER_PROXY,
                LOCAL_UPSTREAM_ID,
                StatusCode::BAD_REQUEST,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::BAD_REQUEST, message))
        }
    }
}

async fn build_outbound_body_or_respond(
    http_clients: &super::http_client::ProxyHttpClients,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    let body = transform_body_or_respond(
        http_clients,
        log,
        request_detail.clone(),
        path,
        plan,
        meta,
        body,
        request_start,
    )
    .await?;
    apply_openai_stream_options_or_respond(
        log,
        request_detail,
        path,
        plan,
        meta,
        body,
        request_start,
    )
    .await
}

async fn transform_body_or_respond(
    http_clients: &super::http_client::ProxyHttpClients,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match maybe_transform_request_body(
        http_clients,
        plan.provider,
        path,
        plan.request_transform,
        meta.original_model.as_deref(),
        body,
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(err) => {
            log_request_error(
                log,
                request_detail,
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            Err(http::error_response(err.status, err.message))
        }
    }
}

async fn apply_openai_stream_options_or_respond(
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match maybe_force_openai_stream_options_include_usage(
        plan.provider,
        plan.outbound_path.unwrap_or(path),
        meta,
        body,
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(err) => {
            log_request_error(
                log,
                request_detail,
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            Err(http::error_response(err.status, err.message))
        }
    }
}

fn resolve_request_auth_or_respond(
    config: &ProxyConfig,
    headers: &HeaderMap,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    provider: &str,
    request_start: Instant,
) -> Result<http::RequestAuth, Response> {
    match http::resolve_request_auth(config, headers) {
        Ok(auth) => Ok(auth),
        Err(message) => {
            log_request_error(
                log,
                request_detail,
                path,
                provider,
                LOCAL_UPSTREAM_ID,
                StatusCode::UNAUTHORIZED,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::UNAUTHORIZED, message))
        }
    }
}

fn build_outbound_path_with_query(outbound_path: &str, uri: &Uri) -> String {
    let Some(query) = uri.query() else {
        return outbound_path.to_string();
    };
    let outbound_query = sanitize_outbound_query(uri.path(), outbound_path, query);
    if outbound_query.is_empty() {
        return outbound_path.to_string();
    }
    format!("{outbound_path}?{outbound_query}")
}

fn sanitize_outbound_query(inbound_path: &str, outbound_path: &str, query: &str) -> String {
    if !is_anthropic_path(inbound_path) || is_anthropic_path(outbound_path) {
        return query.to_string();
    }
    // `beta=true` 只对 Anthropic 原生 `/v1/messages*` 有意义；
    // 当请求 fallback 到 OpenAI/Gemini 兼容 provider 时，继续透传只会把
    // Anthropic 专属 query 泄漏到不相关上游。
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key != "beta")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if pairs.is_empty() {
        return String::new();
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(&key, &value);
    }
    serializer.finish()
}

async fn prepare_inbound_request(
    state: &ProxyState,
    headers: &HeaderMap,
    path: String,
    query: Option<String>,
    body: Body,
    capture_request_detail_enabled: bool,
    request_start: Instant,
    is_debug_log: bool,
) -> Result<InboundRequest, Response> {
    let body = ensure_local_auth_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
        &path,
        query.as_deref(),
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let (plan, body) = resolve_plan_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
        &path,
        query.as_deref(),
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let body = read_body_or_respond(
        &state.log,
        headers,
        body,
        capture_request_detail_enabled,
        &path,
        request_start,
    )
    .await?;
    if is_debug_log {
        log_debug_request(headers, &body).await;
    }
    let meta = parse_request_meta_best_effort(&path, &body).await;
    let request_detail = if capture_request_detail_enabled {
        Some(capture_request_detail(headers, &body, state.config.max_request_body_bytes).await)
    } else {
        None
    };
    Ok(InboundRequest {
        path,
        plan,
        meta,
        request_detail,
        body,
    })
}

async fn finalize_prepared_request(
    state: &ProxyState,
    headers: &HeaderMap,
    uri: &Uri,
    inbound: InboundRequest,
    request_start: Instant,
) -> Result<PreparedRequest, Response> {
    let source_body = inbound.body.clone();
    let outbound_path = resolve_outbound_path(&inbound.path, &inbound.plan, &inbound.meta);
    let outbound_path_with_query = build_outbound_path_with_query(&outbound_path, uri);
    let outbound_body = build_outbound_body_or_respond(
        &state.http_clients,
        &state.log,
        inbound.request_detail.clone(),
        &inbound.path,
        &inbound.plan,
        &inbound.meta,
        inbound.body,
        request_start,
    )
    .await?;
    let request_auth = resolve_request_auth_or_respond(
        &state.config,
        headers,
        &state.log,
        inbound.request_detail.clone(),
        &inbound.path,
        inbound.plan.provider,
        request_start,
    )?;
    Ok(PreparedRequest {
        path: inbound.path,
        outbound_path_with_query,
        plan: inbound.plan,
        meta: inbound.meta,
        request_detail: inbound.request_detail,
        source_body,
        outbound_body,
        request_auth,
    })
}

async fn proxy_request(
    State(state): State<ProxyStateHandle>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // 只在此处短暂持有读锁，避免影响并发请求性能。
    let state = { state.read().await.clone() };
    let request_start = Instant::now();
    let capture_request_detail_enabled = state.request_detail.should_capture();
    let is_debug_log = cfg!(debug_assertions)
        && matches!(state.config.log_level, LogLevel::Debug | LogLevel::Trace);
    let (path, _) = extract_request_path(&uri);
    let query = uri.query().map(|value| value.to_string());
    tracing::info!(method = %method, path = %path, "incoming request");
    tracing::debug!(headers = ?headers.keys().collect::<Vec<_>>(), "request headers");

    if method == Method::GET
        && (is_openai_models_index_path(&path) || is_openai_compatible_models_index_path(&path))
    {
        let body = match ensure_local_auth_or_respond(
            &state.config,
            &state.log,
            &headers,
            body,
            capture_request_detail_enabled,
            &path,
            query.as_deref(),
            request_start,
            state.config.max_request_body_bytes,
        )
        .await
        {
            Ok(body) => body,
            Err(response) => return response,
        };
        let (plan, _body) = match resolve_plan_or_respond(
            &state.config,
            &state.log,
            &headers,
            body,
            capture_request_detail_enabled,
            &path,
            query.as_deref(),
            request_start,
            state.config.max_request_body_bytes,
        )
        .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        let request_auth = match resolve_request_auth_or_respond(
            &state.config,
            &headers,
            &state.log,
            None,
            &path,
            plan.provider,
            request_start,
        ) {
            Ok(request_auth) => request_auth,
            Err(response) => return response,
        };
        let meta = RequestMeta {
            stream: false,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        };
        let outbound_path = resolve_outbound_path(&path, &plan, &meta);
        let outbound_path_with_query = build_outbound_path_with_query(&outbound_path, &uri);
        return aggregate_model_catalog_request(
            state,
            plan.provider,
            &path,
            &outbound_path_with_query,
            &headers,
            &request_auth,
        )
        .await;
    }

    let inbound = match prepare_inbound_request(
        &state,
        &headers,
        path,
        query,
        body,
        capture_request_detail_enabled,
        request_start,
        is_debug_log,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(response) => return response,
    };
    let prepared =
        match finalize_prepared_request(&state, &headers, &uri, inbound, request_start).await {
            Ok(prepared) => prepared,
            Err(response) => return response,
        };
    let primary = forward_upstream_request(
        state.clone(),
        method.clone(),
        prepared.plan.provider,
        &prepared.path,
        &prepared.outbound_path_with_query,
        &headers,
        &prepared.outbound_body,
        &prepared.meta,
        &prepared.request_auth,
        prepared.plan.response_transform,
        prepared.request_detail.clone(),
    )
    .await;

    if primary.should_fallback {
        if let Some(fallback_plan) =
            resolve_retry_fallback_plan(&state.config, &prepared.path, prepared.plan.provider)
        {
            tracing::warn!(
                path = %prepared.path,
                primary = %prepared.plan.provider,
                fallback = %fallback_plan.provider,
                "primary provider exhausted, falling back to alternate provider"
            );
            match forward_retry_fallback_request(
                state,
                method,
                &uri,
                &headers,
                &prepared,
                request_start,
                &fallback_plan,
            )
            .await
            {
                Ok(fallback) if !fallback.should_fallback => return fallback.response,
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        path = %prepared.path,
                        primary = %prepared.plan.provider,
                        fallback = %fallback_plan.provider,
                        "alternate provider fallback aborted before dispatch"
                    );
                }
            }
        }
    }

    primary.response
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "server.test.rs"]
mod tests;
