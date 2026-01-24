use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
};
use std::{
    sync::Arc,
    time::Instant,
};
use tokio::sync::RwLock;

use super::{
    config::ProxyConfig,
    gemini,
    http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai_compat::{
        inbound_format, ApiFormat, FormatTransform, CHAT_PATH, PROVIDER_CHAT, PROVIDER_RESPONSES,
        RESPONSES_PATH,
    },
    request_detail::{capture_request_detail, serialize_request_headers, RequestDetailSnapshot},
    request_body::ReplayableBody,
    server_helpers::{
        extract_request_path, is_anthropic_path, log_debug_request,
        maybe_force_openai_stream_options_include_usage, maybe_transform_request_body,
        parse_request_meta_best_effort,
    },
    upstream::forward_upstream_request,
    ProxyState, RequestMeta,
};
use crate::logging::LogLevel;

const PROVIDER_ANTHROPIC: &str = "anthropic";
const PROVIDER_ANTIGRAVITY: &str = "antigravity";
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
const ERROR_CHAT_CONVERSION_DISABLED: &str =
    "API format conversion is disabled (enable_api_format_conversion=false). Configure provider \"openai\" for /v1/chat/completions or enable conversion.";
const ERROR_RESPONSES_CONVERSION_DISABLED: &str =
    "API format conversion is disabled (enable_api_format_conversion=false). Configure provider \"openai-response\" for /v1/responses or enable conversion.";
const ERROR_ANTHROPIC_CONVERSION_DISABLED: &str =
    "API format conversion is disabled (enable_api_format_conversion=false). Configure provider \"anthropic\" for /v1/messages or enable conversion.";
const ERROR_GEMINI_CONVERSION_DISABLED: &str =
    "API format conversion is disabled (enable_api_format_conversion=false). Configure provider \"gemini\" for Gemini paths or enable conversion.";

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

fn choose_provider_by_priority(config: &ProxyConfig, candidates: &[&'static str]) -> Option<&'static str> {
    let mut selected: Option<(&'static str, ProviderRank)> = None;
    for candidate in candidates {
        let Some(rank) = provider_rank(config, candidate) else {
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
    if let Some(selected) =
        choose_provider_by_priority(config, &[PROVIDER_GEMINI, PROVIDER_ANTIGRAVITY])
    {
        return Some(Ok(base_plan(selected)));
    }
    let fallback = choose_provider_by_priority(
        config,
        &[PROVIDER_RESPONSES, PROVIDER_CHAT, PROVIDER_ANTHROPIC],
    );
    let Some(fallback) = fallback else {
        return Some(Err(ERROR_NO_UPSTREAM.to_string()));
    };
    if !config.enable_api_format_conversion {
        return Some(Err(ERROR_GEMINI_CONVERSION_DISABLED.to_string()));
    }
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
    if path == "/v1/messages" {
        // Claude Code uses /v1/messages. Prefer native providers (Anthropic/Kiro) by priority.
        if let Some(selected) =
            choose_provider_by_priority(config, &[PROVIDER_ANTHROPIC, PROVIDER_KIRO])
        {
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
        if !config.enable_api_format_conversion {
            if config.provider_upstreams(PROVIDER_ANTIGRAVITY).is_some() {
                return Some(Ok(DispatchPlan {
                    provider: PROVIDER_ANTIGRAVITY,
                    outbound_path: None,
                    request_transform: FormatTransform::AnthropicToGemini,
                    response_transform: FormatTransform::GeminiToAnthropic,
                }));
            }
            return Some(Err(ERROR_ANTHROPIC_CONVERSION_DISABLED.to_string()));
        }
        // If native providers are missing, fall back to other formats when enabled (new-api style).
        let fallback = choose_provider_by_priority(
            config,
            &[
                PROVIDER_RESPONSES,
                PROVIDER_CHAT,
                PROVIDER_GEMINI,
                PROVIDER_ANTIGRAVITY,
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
            PROVIDER_ANTIGRAVITY => DispatchPlan {
                provider: PROVIDER_ANTIGRAVITY,
                outbound_path: None,
                request_transform: FormatTransform::AnthropicToGemini,
                response_transform: FormatTransform::GeminiToAnthropic,
            },
            _ => base_plan(PROVIDER_RESPONSES),
        }));
    }
    if config.provider_upstreams(PROVIDER_ANTHROPIC).is_some() {
        return Some(Ok(base_plan(PROVIDER_ANTHROPIC)));
    }
    Some(Err(ERROR_NO_UPSTREAM.to_string()))
}

fn resolve_formatless_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    let provider = choose_provider_by_priority(
        config,
        &[PROVIDER_CHAT, PROVIDER_RESPONSES, PROVIDER_ANTHROPIC],
    )
    .ok_or_else(|| ERROR_NO_UPSTREAM.to_string())?;
    Ok(base_plan(provider))
}

fn resolve_chat_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    if config.provider_upstreams(PROVIDER_CHAT).is_some() {
        return Ok(base_plan(PROVIDER_CHAT));
    }
    let selected = choose_provider_by_priority(
        config,
        &[
            PROVIDER_RESPONSES,
            PROVIDER_CODEX,
            PROVIDER_ANTHROPIC,
            PROVIDER_GEMINI,
            PROVIDER_ANTIGRAVITY,
            PROVIDER_KIRO,
        ],
    )
    .ok_or_else(|| ERROR_NO_UPSTREAM.to_string())?;
    if !config.enable_api_format_conversion {
        return Err(ERROR_CHAT_CONVERSION_DISABLED.to_string());
    }

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
        PROVIDER_ANTIGRAVITY => DispatchPlan {
            provider: PROVIDER_ANTIGRAVITY,
            outbound_path: None,
            request_transform: FormatTransform::ChatToGemini,
            response_transform: FormatTransform::GeminiToChat,
        },
        PROVIDER_KIRO => DispatchPlan {
            provider: PROVIDER_KIRO,
            outbound_path: Some(RESPONSES_PATH),
            request_transform: FormatTransform::None,
            response_transform: FormatTransform::KiroToChat,
        },
        _ => base_plan(PROVIDER_RESPONSES),
    })
}

fn resolve_responses_plan(config: &ProxyConfig) -> Result<DispatchPlan, String> {
    if let Some(selected) =
        choose_provider_by_priority(config, &[PROVIDER_RESPONSES, PROVIDER_CODEX, PROVIDER_KIRO])
    {
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
        return Ok(DispatchPlan {
            provider: PROVIDER_KIRO,
            outbound_path: Some(RESPONSES_PATH),
            request_transform: FormatTransform::None,
            response_transform: FormatTransform::KiroToResponses,
        });
    }
    if !config.enable_api_format_conversion {
        return Err(ERROR_RESPONSES_CONVERSION_DISABLED.to_string());
    }

    let selected = choose_provider_by_priority(
        config,
        &[
            PROVIDER_CHAT,
            PROVIDER_ANTHROPIC,
            PROVIDER_GEMINI,
            PROVIDER_ANTIGRAVITY,
        ],
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
        PROVIDER_ANTIGRAVITY => DispatchPlan {
            provider: PROVIDER_ANTIGRAVITY,
            outbound_path: None,
            request_transform: FormatTransform::ResponsesToGemini,
            response_transform: FormatTransform::GeminiToResponses,
        },
        _ => base_plan(PROVIDER_CHAT),
    })
}

fn resolve_dispatch_plan(config: &ProxyConfig, path: &str) -> Result<DispatchPlan, String> {
    if let Some(plan) = resolve_gemini_plan(config, path) {
        return plan;
    }
    if let Some(plan) = resolve_anthropic_plan(config, path) {
        return plan;
    }

    let Some(format) = inbound_format(path) else {
        return resolve_formatless_plan(config);
    };

    match format {
        ApiFormat::ChatCompletions => resolve_chat_plan(config),
        ApiFormat::Responses => resolve_responses_plan(config),
    }
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
    let (request_headers, request_body) =
        detail.map(|detail| (detail.request_headers, detail.request_body)).unwrap_or((None, None));
    let context = LogContext {
        path: path.to_string(),
        provider: provider.to_string(),
        upstream_id: upstream_id.to_string(),
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
    capture_next: bool,
    path: &str,
    query: Option<&str>,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<Body, Response> {
    if let Err(message) = http::ensure_local_auth(config, headers, path, query) {
        tracing::warn!("local auth failed");
        let detail = if capture_next {
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
    capture_next: bool,
    path: &str,
    request_start: Instant,
    max_body_bytes: usize,
) -> Result<(DispatchPlan, Body), Response> {
    match resolve_dispatch_plan(config, path) {
        Ok(plan) => {
            tracing::debug!(provider = %plan.provider, "dispatch plan resolved");
            Ok((plan, body))
        }
        Err(message) => {
            tracing::warn!("no dispatch plan found");
            let detail = if capture_next {
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
    capture_next: bool,
    path: &str,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match ReplayableBody::from_body(body).await {
        Ok(body) => Ok(body),
        Err(err) => {
            let message = format!("Failed to read request body: {err}");
            let detail = if capture_next {
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
    let body = match maybe_transform_request_body(
        http_clients,
        plan.request_transform,
        meta.original_model.as_deref(),
        body,
    )
    .await
    {
        Ok(body) => body,
        Err(err) => {
            log_request_error(
                log,
                request_detail.clone(),
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            return Err(http::error_response(err.status, err.message));
        }
    };

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
    uri.query()
        .map(|query| format!("{outbound_path}?{query}"))
        .unwrap_or_else(|| outbound_path.to_string())
}

async fn prepare_inbound_request(
    state: &ProxyState,
    headers: &HeaderMap,
    path: String,
    query: Option<String>,
    body: Body,
    capture_next: bool,
    request_start: Instant,
    is_debug_log: bool,
) -> Result<InboundRequest, Response> {
    let body = ensure_local_auth_or_respond(
        &state.config,
        &state.log,
        headers,
        body,
        capture_next,
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
        capture_next,
        &path,
        request_start,
        state.config.max_request_body_bytes,
    )
    .await?;
    let body = read_body_or_respond(&state.log, headers, body, capture_next, &path, request_start)
        .await?;
    if is_debug_log {
        log_debug_request(headers, &body).await;
    }
    let meta = parse_request_meta_best_effort(&path, &body).await;
    let request_detail = if capture_next {
        Some(
            capture_request_detail(headers, &body, state.config.max_request_body_bytes).await,
        )
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
    // 对于 ChatToGemini 转换，需要根据 model 动态构建 Gemini 路径
    let outbound_path = match (inbound.plan.outbound_path, inbound.plan.provider) {
        (Some(path), _) => path.to_string(),
        (None, PROVIDER_GEMINI) if inbound.plan.request_transform != FormatTransform::None => {
            // 从 meta 中获取 model，构建 Gemini API 路径
            let model = inbound
                .meta
                .mapped_model
                .as_deref()
                .or(inbound.meta.original_model.as_deref())
                .unwrap_or("gemini-1.5-flash");
            let suffix = if inbound.meta.stream {
                ":streamGenerateContent"
            } else {
                ":generateContent"
            };
            format!("{}{}{}", gemini::GEMINI_MODELS_PREFIX, model, suffix)
        }
        (None, _) => inbound.path.clone(),
    };
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
    let capture_next = state.request_detail.take();
    let is_debug_log = cfg!(debug_assertions)
        && matches!(state.config.log_level, LogLevel::Debug | LogLevel::Trace);
    let (path, _) = extract_request_path(&uri);
    let query = uri.query().map(|value| value.to_string());
    tracing::info!(method = %method, path = %path, "incoming request");
    tracing::debug!(headers = ?headers.keys().collect::<Vec<_>>(), "request headers");

    let inbound = match prepare_inbound_request(
        &state,
        &headers,
        path,
        query,
        body,
        capture_next,
        request_start,
        is_debug_log,
    )
    .await
    {
        Ok(inbound) => inbound,
        Err(response) => return response,
    };
    let prepared = match finalize_prepared_request(&state, &headers, &uri, inbound, request_start)
        .await
    {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    forward_upstream_request(
        state,
        method,
        prepared.plan.provider,
        &prepared.path,
        &prepared.outbound_path_with_query,
        headers,
        prepared.outbound_body,
        prepared.meta,
        prepared.request_auth,
        prepared.plan.response_transform,
        prepared.request_detail,
    )
    .await
}

#[cfg(test)]
#[path = "server.test.rs"]
mod tests;
