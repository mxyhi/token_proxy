use axum::{
    http::{HeaderMap, Method},
    response::Response,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::{collections::HashSet, future::Future, pin::Pin, time::Duration};

use super::super::{
    config::{InboundApiFormat, ProviderUpstreams, UpstreamDispatchRuntime, UpstreamRuntime},
    cooldown_scope::CooldownScope,
    http::RequestAuth,
    openai_compat::FormatTransform,
    request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot,
    ProxyState, RequestMeta,
};
use super::{
    attempt, requested_target_upstream_id, utils::resolve_group_start, AttemptOutcome,
    RetryDirective, RetryScope,
};

pub(super) struct GroupAttemptResult {
    pub(super) response: Option<Response>,
    pub(super) attempted: usize,
    /// 本组实际尝试过的不同 runtime Upstream（selector_key 去重；同 upstream 原地重试只算一次）。
    pub(super) attempted_upstream_keys: HashSet<String>,
    pub(super) missing_auth: bool,
    pub(super) last_timeout_error: Option<String>,
    pub(super) last_retry_error: Option<String>,
    pub(super) last_retry_response: Option<Response>,
    pub(super) effective_body: Option<ReplayableBody>,
    /// 首响应头前 transport 诊断；仅在最终失败时落库。
    pub(super) last_deferred_log: Option<DeferredTransportLog>,
}

impl GroupAttemptResult {
    fn new() -> Self {
        Self {
            response: None,
            attempted: 0,
            attempted_upstream_keys: HashSet::new(),
            missing_auth: false,
            last_timeout_error: None,
            last_retry_error: None,
            last_retry_response: None,
            effective_body: None,
            last_deferred_log: None,
        }
    }
}

pub(super) struct ForwardAttemptState {
    pub(super) response: Option<Response>,
    pub(super) attempted: usize,
    /// 跨 priority group 累计的实际尝试过的不同 runtime Upstream（selector_key）。
    pub(super) attempted_upstream_keys: HashSet<String>,
    pub(super) missing_auth: bool,
    pub(super) last_timeout_error: Option<String>,
    pub(super) last_retry_error: Option<String>,
    pub(super) last_retry_response: Option<Response>,
    pub(super) effective_body: Option<ReplayableBody>,
    pub(super) last_deferred_log: Option<DeferredTransportLog>,
    pub(super) model_unsupported: bool,
}

impl ForwardAttemptState {
    pub(super) fn new() -> Self {
        Self {
            response: None,
            attempted: 0,
            attempted_upstream_keys: HashSet::new(),
            missing_auth: false,
            last_timeout_error: None,
            last_retry_error: None,
            last_retry_response: None,
            effective_body: None,
            last_deferred_log: None,
            model_unsupported: false,
        }
    }
}

/// 可重试 transport 失败的延后落库载荷；成功恢复时丢弃。
#[derive(Clone, Debug)]
pub(super) struct DeferredTransportLog {
    pub(super) upstream_id: String,
    pub(super) account_id: Option<String>,
    pub(super) status: u16,
    pub(super) message: String,
    pub(super) start_time: std::time::Instant,
}

type GroupAttemptFuture<'a> = Pin<Box<dyn Future<Output = (usize, AttemptOutcome)> + Send + 'a>>;

#[derive(Clone, Copy)]
enum CompletionLaunchMode {
    FillToCapacity,
    SingleSlot,
}

#[derive(Clone, Copy)]
struct GroupDispatchPlan {
    initial_parallel: usize,
    max_parallel: usize,
    hedge_delay: Option<Duration>,
    completion_launch_mode: CompletionLaunchMode,
}

impl GroupDispatchPlan {
    fn from_dispatch(dispatch: &UpstreamDispatchRuntime) -> Self {
        match dispatch {
            UpstreamDispatchRuntime::Serial => Self {
                initial_parallel: 1,
                max_parallel: 1,
                hedge_delay: None,
                completion_launch_mode: CompletionLaunchMode::SingleSlot,
            },
            UpstreamDispatchRuntime::Hedged {
                delay,
                max_parallel,
            } => Self {
                initial_parallel: 1,
                max_parallel: *max_parallel,
                hedge_delay: Some(*delay),
                completion_launch_mode: CompletionLaunchMode::SingleSlot,
            },
            UpstreamDispatchRuntime::Race { max_parallel } => Self {
                initial_parallel: *max_parallel,
                max_parallel: *max_parallel,
                hedge_delay: None,
                completion_launch_mode: CompletionLaunchMode::FillToCapacity,
            },
        }
    }

    fn completion_launch_slots(self, in_flight_len: usize) -> usize {
        match self.completion_launch_mode {
            CompletionLaunchMode::FillToCapacity => self.max_parallel.saturating_sub(in_flight_len),
            CompletionLaunchMode::SingleSlot => usize::from(in_flight_len < self.max_parallel),
        }
    }
}

fn apply_attempt_outcome(result: &mut GroupAttemptResult, outcome: AttemptOutcome) -> bool {
    if let AttemptOutcome::Retryable {
        response: Some(response),
        ..
    } = &outcome
    {
        if let Some(body) = response
            .extensions()
            .get::<RetryDirective>()
            .and_then(|directive| directive.effective_body.clone())
        {
            result.effective_body = Some(body);
        }
    }
    match outcome {
        AttemptOutcome::Success(response) | AttemptOutcome::Fatal(response) => {
            // 成功或 Fatal 已自带终态日志路径；丢弃中间 deferred。
            result.last_deferred_log = None;
            result.response = Some(response);
            true
        }
        AttemptOutcome::Retryable {
            message,
            response,
            is_timeout,
            should_cooldown: _,
            deferred_log,
        } => {
            if is_timeout {
                result.last_timeout_error = Some(message.clone());
            } else {
                result.last_retry_error = Some(message.clone());
            }
            if response.is_some() {
                result.last_retry_response = response;
            }
            // deferred_log 仅 transport 路径设置；HTTP 可重试响应已由 response 路径记日志。
            // 中间 attempt 不落库，仅保留最后一次，供 finalize 终态失败时写一条。
            if deferred_log.is_none() {
                // HTTP/语义 Retryable 没有 deferred 诊断，清掉旧 transport deferred，避免串台。
                result.last_deferred_log = None;
            }
            false
        }
        AttemptOutcome::SkippedAuth => {
            result.missing_auth = true;
            false
        }
    }
}

fn merge_group_result(state: &mut ForwardAttemptState, result: GroupAttemptResult) -> bool {
    state.attempted += result.attempted;
    // 跨 priority group 并入 distinct runtime Upstream，供 finalize 判断是否 mask 401/403。
    state
        .attempted_upstream_keys
        .extend(result.attempted_upstream_keys);
    state.missing_auth |= result.missing_auth;
    if let Some(response) = result.response {
        state.response = Some(response);
        state.last_deferred_log = None;
        return true;
    }
    if result.last_timeout_error.is_some() {
        state.last_timeout_error = result.last_timeout_error;
    }
    if result.last_retry_error.is_some() {
        state.last_retry_error = result.last_retry_error;
    }
    if let Some(response) = result.last_retry_response {
        state.last_retry_response = Some(response);
    }
    if result.effective_body.is_some() {
        state.effective_body = result.effective_body;
    }
    if result.last_deferred_log.is_some() {
        state.last_deferred_log = result.last_deferred_log;
    }
    false
}

pub(super) async fn run_upstream_groups(
    state: &ProxyState,
    method: Method,
    provider: &str,
    inbound_format: Option<InboundApiFormat>,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    upstreams: &ProviderUpstreams,
    cooldown_scope: &CooldownScope,
) -> ForwardAttemptState {
    let target_upstream_id =
        requested_target_upstream_id(upstreams, meta.original_model.as_deref());
    let mut summary = ForwardAttemptState::new();
    for (group_index, group) in upstreams.groups.iter().enumerate() {
        if group.items.is_empty() {
            continue;
        }
        if let Some(inbound_format) = inbound_format {
            if group
                .items
                .iter()
                .all(|item| !item.supports_inbound(inbound_format))
            {
                continue;
            }
        }
        let active_body = summary.effective_body.as_ref().unwrap_or(body);
        let result = try_group_upstreams(
            state,
            method.clone(),
            provider,
            group_index,
            &group.items,
            inbound_format,
            inbound_path,
            upstream_path_with_query,
            headers,
            active_body,
            meta,
            target_upstream_id.as_deref(),
            request_auth,
            client_gemini_api_key,
            response_transform,
            request_detail.clone(),
            cooldown_scope,
        )
        .await;
        if merge_group_result(&mut summary, result) {
            break;
        }
    }
    if summary.attempted == 0
        && meta.original_model.is_some()
        && provider_has_route_candidate(upstreams, inbound_format, target_upstream_id.as_deref())
        && !provider_has_model_candidate(
            upstreams,
            inbound_format,
            target_upstream_id.as_deref(),
            meta.original_model.as_deref(),
        )
    {
        summary.model_unsupported = true;
        tracing::warn!(
            provider,
            model = meta.original_model.as_deref().unwrap_or(""),
            exclusion_reason = "model_not_supported",
            "all upstream candidates excluded by model allowlist"
        );
    }
    summary
}

fn provider_has_route_candidate(
    upstreams: &ProviderUpstreams,
    inbound_format: Option<InboundApiFormat>,
    target_upstream_id: Option<&str>,
) -> bool {
    upstreams
        .groups
        .iter()
        .flat_map(|group| &group.items)
        .any(|item| {
            inbound_format.is_none_or(|format| item.supports_inbound(format))
                && target_upstream_id.is_none_or(|target| item.id == target)
        })
}

fn provider_has_model_candidate(
    upstreams: &ProviderUpstreams,
    inbound_format: Option<InboundApiFormat>,
    target_upstream_id: Option<&str>,
    original_model: Option<&str>,
) -> bool {
    upstreams
        .groups
        .iter()
        .flat_map(|group| &group.items)
        .any(|item| {
            inbound_format.is_none_or(|format| item.supports_inbound(format))
                && target_upstream_id.is_none_or(|target| item.id == target)
                && item.supports_model(original_model)
        })
}

async fn try_group_upstreams(
    state: &ProxyState,
    method: Method,
    provider: &str,
    group_index: usize,
    items: &[UpstreamRuntime],
    inbound_format: Option<InboundApiFormat>,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    target_upstream_id: Option<&str>,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
) -> GroupAttemptResult {
    let start = resolve_group_start(state, provider, group_index, items.len());
    let order = state.upstream_selector.order_group_scoped(
        state.config.upstream_strategy.order,
        provider,
        items,
        start,
        cooldown_scope,
    );
    let eligible_order = filter_eligible_upstreams(
        order,
        items,
        inbound_format,
        target_upstream_id,
        meta.original_model.as_deref(),
    );
    if eligible_order.is_empty() {
        return GroupAttemptResult::new();
    }
    dispatch_group_upstreams(
        state,
        method,
        provider,
        items,
        &eligible_order,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        client_gemini_api_key,
        response_transform,
        request_detail,
        GroupDispatchPlan::from_dispatch(&state.config.upstream_strategy.dispatch),
        cooldown_scope,
    )
    .await
}

fn filter_eligible_upstreams(
    order: Vec<usize>,
    items: &[UpstreamRuntime],
    inbound_format: Option<InboundApiFormat>,
    target_upstream_id: Option<&str>,
    original_model: Option<&str>,
) -> Vec<usize> {
    order
        .into_iter()
        .filter(|item_index| {
            inbound_format.is_none_or(|format| items[*item_index].supports_inbound(format))
                && target_upstream_id.is_none_or(|target| items[*item_index].id.as_str() == target)
                && items[*item_index].supports_model(original_model)
        })
        .collect()
}

fn apply_group_attempt_outcome(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    result: &mut GroupAttemptResult,
    outcome: AttemptOutcome,
    cooldown_scope: &CooldownScope,
) -> bool {
    match &outcome {
        AttemptOutcome::Success(_) => {
            state.upstream_selector.clear_cooldown_scoped(
                provider,
                upstream.selector_key.as_str(),
                cooldown_scope,
            );
        }
        AttemptOutcome::Retryable {
            should_cooldown: true,
            ..
        } => {
            state.upstream_selector.mark_retryable_failure_scoped(
                provider,
                upstream.selector_key.as_str(),
                cooldown_scope,
            );
        }
        _ => {}
    }
    // 仅真实 attempt 计数并记入去重集合；SkippedAuth 不算。
    // 同 selector_key 的 same-upstream retry / 内部 recovery 不膨胀 distinct 数。
    if !matches!(outcome, AttemptOutcome::SkippedAuth) {
        result.attempted += 1;
        let key = upstream.selector_key.as_str();
        if result.attempted_upstream_keys.insert(key.to_string()) {
            tracing::debug!(
                provider,
                upstream = %upstream.id,
                selector_key = %key,
                distinct = result.attempted_upstream_keys.len(),
                "recorded distinct attempted runtime upstream"
            );
        }
    }
    // 在 move outcome 前抽出 deferred，绑定当前 upstream。
    let deferred = match &outcome {
        AttemptOutcome::Retryable {
            deferred_log: Some(message),
            is_timeout,
            ..
        } => Some(DeferredTransportLog {
            upstream_id: upstream.id.clone(),
            account_id: None,
            status: if *is_timeout { 504 } else { 502 },
            message: message.clone(),
            start_time: std::time::Instant::now(),
        }),
        _ => None,
    };
    let terminal = apply_attempt_outcome(result, outcome);
    if let Some(deferred) = deferred {
        result.last_deferred_log = Some(deferred);
    }
    terminal
}

async fn dispatch_group_upstreams(
    state: &ProxyState,
    method: Method,
    provider: &str,
    items: &[UpstreamRuntime],
    order: &[usize],
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    dispatch_plan: GroupDispatchPlan,
    cooldown_scope: &CooldownScope,
) -> GroupAttemptResult {
    let mut result = GroupAttemptResult::new();
    let mut in_flight: FuturesUnordered<GroupAttemptFuture<'_>> = FuturesUnordered::new();
    let mut next_to_launch = 0usize;
    let max_same_upstream_retries = state.config.same_upstream_retry_count;

    launch_group_attempts(
        &mut in_flight,
        &mut next_to_launch,
        dispatch_plan.initial_parallel.min(order.len()),
        state,
        &method,
        provider,
        items,
        order,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        client_gemini_api_key,
        response_transform,
        &request_detail,
        cooldown_scope,
    );

    let mut hedge_timer = next_hedge_timer(
        dispatch_plan.hedge_delay,
        next_to_launch < order.len(),
        in_flight.len(),
        dispatch_plan.max_parallel,
    );
    while next_to_launch < order.len() || !in_flight.is_empty() {
        if in_flight.is_empty() {
            let remaining = order.len() - next_to_launch;
            launch_group_attempts(
                &mut in_flight,
                &mut next_to_launch,
                dispatch_plan.initial_parallel.min(remaining),
                state,
                &method,
                provider,
                items,
                order,
                inbound_path,
                upstream_path_with_query,
                headers,
                result.effective_body.as_ref().unwrap_or(body),
                meta,
                request_auth,
                client_gemini_api_key,
                response_transform,
                &request_detail,
                cooldown_scope,
            );
            hedge_timer = next_hedge_timer(
                dispatch_plan.hedge_delay,
                next_to_launch < order.len(),
                in_flight.len(),
                dispatch_plan.max_parallel,
            );
            continue;
        }

        let completed = if let Some(timer) = hedge_timer.as_mut() {
            tokio::select! {
                maybe = in_flight.next(), if !in_flight.is_empty() => maybe,
                _ = timer.as_mut(), if next_to_launch < order.len() => {
                    launch_group_attempts(
                        &mut in_flight,
                        &mut next_to_launch,
                        1,
                        state,
                        &method,
                        provider,
                        items,
                        order,
                        inbound_path,
                        upstream_path_with_query,
                        headers,
                        result.effective_body.as_ref().unwrap_or(body),
                        meta,
                        request_auth,
                        client_gemini_api_key,
                        response_transform,
                        &request_detail,
                        cooldown_scope,
                    );
                    None
                }
            }
        } else {
            in_flight.next().await
        };

        if let Some((item_index, outcome)) = completed {
            let upstream = &items[item_index];
            let retry_body = result
                .effective_body
                .clone()
                .unwrap_or_else(|| body.clone());
            // 任意 Retryable：先在同一上游原地重试最多 N 次，再进入跨上游 failover。
            if apply_same_upstream_retries(
                state,
                &method,
                provider,
                upstream,
                inbound_path,
                upstream_path_with_query,
                headers,
                &retry_body,
                meta,
                request_auth,
                client_gemini_api_key,
                response_transform,
                &request_detail,
                cooldown_scope,
                &mut result,
                outcome,
                max_same_upstream_retries,
            )
            .await
            {
                return result;
            }
            let immediate_slots = dispatch_plan
                .completion_launch_slots(in_flight.len())
                .min(order.len().saturating_sub(next_to_launch));
            if immediate_slots > 0 {
                launch_group_attempts(
                    &mut in_flight,
                    &mut next_to_launch,
                    immediate_slots,
                    state,
                    &method,
                    provider,
                    items,
                    order,
                    inbound_path,
                    upstream_path_with_query,
                    headers,
                    result.effective_body.as_ref().unwrap_or(body),
                    meta,
                    request_auth,
                    client_gemini_api_key,
                    response_transform,
                    &request_detail,
                    cooldown_scope,
                );
            }
        }

        hedge_timer = next_hedge_timer(
            dispatch_plan.hedge_delay,
            next_to_launch < order.len(),
            in_flight.len(),
            dispatch_plan.max_parallel,
        );
    }

    result
}

/// 对当前完成结果记账；若为 Retryable 且未达上限，则同步原地再试。
/// 返回 true 表示已得到终态响应（Success/Fatal），调用方应立即返回。
/// Same-Upstream Retry 固定 account_id：清掉该 pin 上刚写入的 cooldown，避免原地重试被 prepare 短路。
fn clear_pinned_account_cooldown_before_same_upstream_retry(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    cooldown_scope: &CooldownScope,
) {
    let account_id = match provider {
        "codex" => upstream.codex_account_id.as_deref(),
        "xai" => upstream.xai_account_id.as_deref(),
        "kiro" => upstream.kiro_account_id.as_deref(),
        _ => None,
    };
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if state
        .account_selector
        .clear_cooldown_scoped(provider, account_id, cooldown_scope)
    {
        tracing::debug!(
            provider,
            account_id,
            "cleared pinned account cooldown before same-upstream retry"
        );
    }
}

async fn apply_same_upstream_retries(
    state: &ProxyState,
    method: &Method,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<&str>,
    response_transform: FormatTransform,
    request_detail: &Option<RequestDetailSnapshot>,
    cooldown_scope: &CooldownScope,
    result: &mut GroupAttemptResult,
    initial: AttemptOutcome,
    max_retries: u32,
) -> bool {
    let mut current = initial;
    let mut current_body = body.clone();
    let mut used = 0u32;
    loop {
        let directive = match &current {
            AttemptOutcome::Retryable {
                response: Some(response),
                ..
            } => response.extensions().get::<RetryDirective>().cloned(),
            _ => None,
        };
        if let Some(body) = directive
            .as_ref()
            .and_then(|directive| directive.effective_body.clone())
        {
            current_body = body;
        }
        let is_retryable = matches!(current, AttemptOutcome::Retryable { .. });
        let retry_same_upstream = directive
            .as_ref()
            .is_none_or(|directive| directive.scope == RetryScope::SameThenNext);
        if apply_group_attempt_outcome(state, provider, upstream, result, current, cooldown_scope) {
            return true;
        }
        if !is_retryable || !retry_same_upstream || used >= max_retries {
            return false;
        }
        used += 1;
        tracing::info!(
            provider,
            upstream = %upstream.id,
            attempt = used,
            max = max_retries,
            "retrying same upstream before upstream failover"
        );
        // 失败 attempt 可能刚写入 account cooldown；same-upstream 仍钉死同一 account_id，
        // 必须清掉该 pin 的冷却，否则 prepare 会 503 短路、实际请求数永远只有 1。
        clear_pinned_account_cooldown_before_same_upstream_retry(
            state,
            provider,
            upstream,
            cooldown_scope,
        );
        // 原地重试前：已 rotate 过的毒连接由 transport 内层处理；此处再清一次 H2 槽，
        // 覆盖 dispatch 层 capacity/HTTP 类 Retryable 之外的同连接复用风险。
        let rotate_result = if provider == "xai" {
            state
                .http_clients
                .rotate_xai_client_for_proxy_url(upstream.proxy_url.as_deref())
        } else {
            state
                .http_clients
                .rotate_client_for_proxy_url(upstream.proxy_url.as_deref())
        };
        if let Err(message) = rotate_result {
            tracing::warn!(
                provider,
                upstream = %upstream.id,
                error = %message,
                "failed to rotate HTTP client before same-upstream retry"
            );
        }
        current = attempt::attempt_upstream(
            state,
            method.clone(),
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            headers,
            &current_body,
            meta,
            request_auth,
            client_gemini_api_key,
            response_transform,
            request_detail.clone(),
            cooldown_scope,
        )
        .await;
        let retry_result = match &current {
            AttemptOutcome::Success(_) => "success",
            AttemptOutcome::Retryable { .. } => "retryable_failure",
            AttemptOutcome::Fatal(_) => "fatal_failure",
            AttemptOutcome::SkippedAuth => "skipped_auth",
        };
        tracing::info!(
            provider,
            upstream = %upstream.id,
            attempt = used,
            max = max_retries,
            retry_result,
            "same upstream retry completed"
        );
    }
}

fn launch_group_attempts<'a>(
    in_flight: &mut FuturesUnordered<GroupAttemptFuture<'a>>,
    next_to_launch: &mut usize,
    slots: usize,
    state: &'a ProxyState,
    method: &Method,
    provider: &'a str,
    items: &'a [UpstreamRuntime],
    order: &'a [usize],
    inbound_path: &'a str,
    upstream_path_with_query: &'a str,
    headers: &'a HeaderMap,
    body: &ReplayableBody,
    meta: &'a RequestMeta,
    request_auth: &'a RequestAuth,
    client_gemini_api_key: Option<&'a str>,
    response_transform: FormatTransform,
    request_detail: &Option<RequestDetailSnapshot>,
    cooldown_scope: &'a CooldownScope,
) {
    for _ in 0..slots {
        let Some(item_index) = order.get(*next_to_launch).copied() else {
            break;
        };
        *next_to_launch += 1;
        enqueue_group_attempt(
            in_flight,
            state,
            method,
            provider,
            items,
            item_index,
            inbound_path,
            upstream_path_with_query,
            headers,
            body,
            meta,
            request_auth,
            client_gemini_api_key,
            response_transform,
            request_detail,
            cooldown_scope,
        );
    }
}

fn enqueue_group_attempt<'a>(
    in_flight: &mut FuturesUnordered<GroupAttemptFuture<'a>>,
    state: &'a ProxyState,
    method: &Method,
    provider: &'a str,
    items: &'a [UpstreamRuntime],
    item_index: usize,
    inbound_path: &'a str,
    upstream_path_with_query: &'a str,
    headers: &'a HeaderMap,
    body: &ReplayableBody,
    meta: &'a RequestMeta,
    request_auth: &'a RequestAuth,
    client_gemini_api_key: Option<&'a str>,
    response_transform: FormatTransform,
    request_detail: &Option<RequestDetailSnapshot>,
    cooldown_scope: &'a CooldownScope,
) {
    let upstream = &items[item_index];
    let method = method.clone();
    let request_detail = request_detail.clone();
    let body = body.clone();
    in_flight.push(Box::pin(async move {
        let outcome = attempt::attempt_upstream(
            state,
            method,
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            headers,
            &body,
            meta,
            request_auth,
            client_gemini_api_key,
            response_transform,
            request_detail,
            cooldown_scope,
        )
        .await;
        (item_index, outcome)
    }));
}

fn next_hedge_timer(
    hedged_request_delay: Option<Duration>,
    has_pending_attempts: bool,
    in_flight_len: usize,
    max_parallel: usize,
) -> Option<Pin<Box<tokio::time::Sleep>>> {
    let Some(hedged_request_delay) = hedged_request_delay else {
        return None;
    };
    if !has_pending_attempts || in_flight_len == 0 || in_flight_len >= max_parallel {
        return None;
    }
    Some(Box::pin(tokio::time::sleep(hedged_request_delay)))
}

#[cfg(test)]
#[path = "dispatch.test.rs"]
mod tests;
