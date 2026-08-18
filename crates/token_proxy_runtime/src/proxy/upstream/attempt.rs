use std::time::Instant;

use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::ResponseBuilderExt;

use super::retry::{
    finalize_attempt, retry_after_kiro_refresh, retry_with_account_auth_recovery,
    AccountAuthRecoveryResult, UpstreamAttempt, UpstreamAttemptFailure,
};
use super::transport::send_upstream_request;
use super::{
    request_repair::{repair_request_body, RequestRepair, RequestRepairState},
    AttemptOutcome, PreparedUpstreamRequest, RetryDirective, RetryScope,
};
use crate::proxy::log::RequestTimings;
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::{
    config::UpstreamRuntime, cooldown_scope::CooldownScope, ProxyState, RequestMeta,
};

/// 发送前注册 token rate 窗口：先只计 connections，input 等响应阶段再写。
async fn register_token_tracker_for_attempt(
    state: &ProxyState,
    meta: &RequestMeta,
) -> crate::proxy::token_rate::RequestTokenTracker {
    let model_for_tokens = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref())
        .map(|value| value.to_string());
    tracing::debug!(
        model = model_for_tokens.as_deref().unwrap_or(""),
        "token_rate register before upstream send"
    );
    state.token_rate.register(model_for_tokens, None).await
}

pub(super) async fn attempt_upstream(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &crate::proxy::http::RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
) -> AttemptOutcome {
    if provider == "kiro" {
        return super::kiro::attempt_kiro_upstream(
            state,
            method,
            upstream,
            inbound_path,
            headers,
            body,
            meta,
            response_transform,
            request_detail,
        )
        .await;
    }
    let mut effective_body = body.clone();
    let mut repair_state = RequestRepairState::new(&effective_body);
    let mut repair_count = 0usize;
    let mut fixed_upstream = upstream.clone();
    let mut first = attempt_send(
        state,
        method.clone(),
        provider,
        &fixed_upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        &effective_body,
        meta,
        request_auth,
        request_detail.as_ref(),
        cooldown_scope,
    )
    .await;
    loop {
        let attempt = match first {
            Ok(attempt) => attempt,
            Err(failure) => {
                first = Err(failure);
                break;
            }
        };
        let (attempt, repair) =
            match inspect_request_repair(attempt, &effective_body, &mut repair_state).await {
                Ok(value) => value,
                Err(failure) => {
                    first = Err(failure);
                    break;
                }
            };
        let Some(repair) = repair else {
            first = Ok(attempt);
            break;
        };
        // 修复重试必须固定当前 account_id；跨账户选号已删除。
        if matches!(provider, "codex" | "xai" | "kiro") {
            super::retry::pin_account_if_missing(
                provider,
                &mut fixed_upstream,
                attempt.selected_account_id.as_deref(),
            );
        }
        repair_count += 1;
        tracing::info!(
            provider,
            upstream = %upstream.id,
            account_id = attempt.selected_account_id.as_deref().unwrap_or(""),
            repair_kind = ?repair.kind,
            reason = repair.reason,
            repair_count,
            "retrying upstream after safe request repair"
        );
        drop(attempt);
        effective_body = repair.body;
        first = attempt_send(
            state,
            method.clone(),
            provider,
            &fixed_upstream,
            inbound_path,
            upstream_path_with_query,
            headers,
            &effective_body,
            meta,
            request_auth,
            request_detail.as_ref(),
            cooldown_scope,
        )
        .await;
    }
    let first = match retry_with_account_auth_recovery(
        state,
        method.clone(),
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        &effective_body,
        meta,
        request_auth,
        client_gemini_api_key,
        response_transform,
        request_detail.clone(),
        first,
        cooldown_scope,
    )
    .await
    {
        AccountAuthRecoveryResult::Pending(attempt) => attempt,
        AccountAuthRecoveryResult::Resolved(outcome) => {
            return attach_effective_body(outcome, &effective_body, repair_count > 0);
        }
    };
    if let Some(outcome) = retry_after_kiro_refresh(
        state,
        method.clone(),
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        &effective_body,
        meta,
        request_auth,
        client_gemini_api_key,
        response_transform,
        request_detail.clone(),
        &first,
        cooldown_scope,
    )
    .await
    {
        return attach_effective_body(outcome, &effective_body, repair_count > 0);
    }
    let outcome = finalize_attempt(
        state,
        provider,
        upstream,
        inbound_path,
        client_gemini_api_key,
        response_transform,
        request_detail,
        first,
        cooldown_scope,
    )
    .await;
    attach_effective_body(outcome, &effective_body, repair_count > 0)
}

async fn inspect_request_repair(
    attempt: UpstreamAttempt,
    request_body: &ReplayableBody,
    state: &mut RequestRepairState,
) -> Result<(UpstreamAttempt, Option<RequestRepair>), UpstreamAttemptFailure> {
    if attempt.response.status() != StatusCode::BAD_REQUEST {
        return Ok((attempt, None));
    }
    let UpstreamAttempt {
        response,
        selected_account_id,
        meta,
        start_time,
        timings,
        token_tracker,
        xai_client_tools,
        skip_same_upstream_retry,
    } = attempt;
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let url = response.url().clone();
    let response_body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            drop(token_tracker);
            return Err(UpstreamAttemptFailure {
                outcome: AttemptOutcome::Retryable {
                    message: format!("Failed to read upstream repair response: {error}"),
                    response: None,
                    is_timeout: error.is_timeout(),
                    should_cooldown: false,
                    deferred_log: None,
                },
                selected_account_id,
            });
        }
    };
    // response.bytes() 消耗响应；未命中修复时重建原状态、头和错误体供既有流程处理。
    let mut rebuilt = axum::http::Response::builder()
        .status(status)
        .version(version)
        .url(url)
        .body(response_body.clone())
        .map(reqwest::Response::from)
        .map_err(|error| UpstreamAttemptFailure {
            outcome: AttemptOutcome::Fatal(crate::proxy::http::error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to rebuild upstream repair response: {error}"),
            )),
            selected_account_id: selected_account_id.clone(),
        })?;
    *rebuilt.headers_mut() = headers;
    let repair =
        repair_request_body(status, request_body, &response_body, state).map_err(|error| {
            UpstreamAttemptFailure {
                outcome: AttemptOutcome::Fatal(crate::proxy::http::error_response(
                    StatusCode::BAD_GATEWAY,
                    error,
                )),
                selected_account_id: selected_account_id.clone(),
            }
        })?;
    Ok((
        UpstreamAttempt {
            response: rebuilt,
            selected_account_id,
            meta,
            start_time,
            timings,
            token_tracker,
            xai_client_tools,
            skip_same_upstream_retry,
        },
        repair,
    ))
}

fn attach_effective_body(
    mut outcome: AttemptOutcome,
    effective_body: &ReplayableBody,
    repaired: bool,
) -> AttemptOutcome {
    if !repaired {
        return outcome;
    }
    let AttemptOutcome::Retryable {
        response: Some(response),
        ..
    } = &mut outcome
    else {
        return outcome;
    };
    let scope = response
        .extensions()
        .get::<RetryDirective>()
        .map_or(RetryScope::SameThenNext, |directive| directive.scope);
    response.extensions_mut().insert(RetryDirective {
        scope,
        effective_body: Some(effective_body.clone()),
    });
    outcome
}

pub(super) async fn attempt_send(
    state: &ProxyState,
    method: Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &crate::proxy::http::RequestAuth,
    request_detail: Option<&RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
) -> Result<UpstreamAttempt, UpstreamAttemptFailure> {
    let prepared = super::prepare_upstream_request_with_body(
        state,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        cooldown_scope,
    )
    .await
    .map_err(|outcome| UpstreamAttemptFailure {
        outcome,
        selected_account_id: None,
    })?;
    let xai_client_tools = super::request_body::xai_client_tool_mapping(
        provider,
        upstream_path_with_query,
        body,
        state.config.xai_inject_x_search,
        state.config.max_request_body_bytes,
    )
    .await
    .map_err(|outcome| UpstreamAttemptFailure {
        outcome,
        selected_account_id: None,
    })?;
    let PreparedUpstreamRequest {
        upstream_path_with_query,
        upstream_url,
        request_headers,
        proxy_url,
        selected_account_id,
        codex_openai_device_id,
        meta,
    } = prepared;
    // 在真正发上游前 register，TTFB 期间托盘也能显示 connections（↑ fallback）。
    let token_tracker = register_token_tracker_for_attempt(state, &meta).await;
    let start_time = Instant::now();
    let timings = RequestTimings::with_billing(meta.billing.clone());
    let response = match send_upstream_request(
        state,
        method,
        provider,
        upstream,
        inbound_path,
        &upstream_path_with_query,
        &upstream_url,
        proxy_url.as_deref(),
        &request_headers,
        body,
        &meta,
        selected_account_id.as_deref(),
        codex_openai_device_id.as_deref(),
        request_detail,
        start_time,
        timings.clone(),
        cooldown_scope,
    )
    .await
    {
        Ok(response) => response,
        Err(outcome) => {
            // 发送失败：drop tracker，active 回落。
            drop(token_tracker);
            return Err(UpstreamAttemptFailure {
                outcome,
                selected_account_id: selected_account_id.clone(),
            });
        }
    };
    Ok(UpstreamAttempt {
        response,
        selected_account_id,
        meta,
        start_time,
        timings,
        token_tracker,
        xai_client_tools,
        skip_same_upstream_retry: false,
    })
}
