use axum::{
    http::{HeaderMap, Method},
    response::Response,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::{future::Future, pin::Pin, time::Duration};

use super::super::{
    config::{InboundApiFormat, ProviderUpstreams, UpstreamDispatchRuntime, UpstreamRuntime},
    http::RequestAuth,
    openai_compat::FormatTransform,
    request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot,
    ProxyState, RequestMeta,
};
use super::{attempt, requested_target_upstream_id, utils::resolve_group_start, AttemptOutcome};

pub(super) struct GroupAttemptResult {
    pub(super) response: Option<Response>,
    pub(super) attempted: usize,
    pub(super) missing_auth: bool,
    pub(super) last_timeout_error: Option<String>,
    pub(super) last_retry_error: Option<String>,
    pub(super) last_retry_response: Option<Response>,
}

impl GroupAttemptResult {
    fn new() -> Self {
        Self {
            response: None,
            attempted: 0,
            missing_auth: false,
            last_timeout_error: None,
            last_retry_error: None,
            last_retry_response: None,
        }
    }
}

pub(super) struct ForwardAttemptState {
    pub(super) response: Option<Response>,
    pub(super) attempted: usize,
    pub(super) missing_auth: bool,
    pub(super) last_timeout_error: Option<String>,
    pub(super) last_retry_error: Option<String>,
    pub(super) last_retry_response: Option<Response>,
}

impl ForwardAttemptState {
    pub(super) fn new() -> Self {
        Self {
            response: None,
            attempted: 0,
            missing_auth: false,
            last_timeout_error: None,
            last_retry_error: None,
            last_retry_response: None,
        }
    }
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
    match outcome {
        AttemptOutcome::Success(response) | AttemptOutcome::Fatal(response) => {
            result.response = Some(response);
            true
        }
        AttemptOutcome::Retryable {
            message,
            response,
            is_timeout,
            should_cooldown: _,
        } => {
            if is_timeout {
                result.last_timeout_error = Some(message.clone());
            } else {
                result.last_retry_error = Some(message.clone());
            }
            if response.is_some() {
                result.last_retry_response = response;
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
    state.missing_auth |= result.missing_auth;
    if let Some(response) = result.response {
        state.response = Some(response);
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
            body,
            meta,
            target_upstream_id.as_deref(),
            request_auth,
            client_gemini_api_key,
            response_transform,
            request_detail.clone(),
        )
        .await;
        if merge_group_result(&mut summary, result) {
            break;
        }
    }
    summary
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
) -> GroupAttemptResult {
    let start = resolve_group_start(state, provider, group_index, items.len());
    let order = state.upstream_selector.order_group(
        state.config.upstream_strategy.order,
        provider,
        items,
        start,
    );
    let eligible_order =
        filter_eligible_upstreams(order, items, inbound_format, target_upstream_id);
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
    )
    .await
}

fn filter_eligible_upstreams(
    order: Vec<usize>,
    items: &[UpstreamRuntime],
    inbound_format: Option<InboundApiFormat>,
    target_upstream_id: Option<&str>,
) -> Vec<usize> {
    order
        .into_iter()
        .filter(|item_index| {
            inbound_format.is_none_or(|format| items[*item_index].supports_inbound(format))
                && target_upstream_id.is_none_or(|target| items[*item_index].id.as_str() == target)
        })
        .collect()
}

fn apply_group_attempt_outcome(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    result: &mut GroupAttemptResult,
    outcome: AttemptOutcome,
) -> bool {
    match &outcome {
        AttemptOutcome::Success(_) => {
            state
                .upstream_selector
                .clear_cooldown(provider, upstream.selector_key.as_str());
        }
        AttemptOutcome::Retryable {
            should_cooldown: true,
            ..
        } => {
            state
                .upstream_selector
                .mark_retryable_failure(provider, upstream.selector_key.as_str());
        }
        _ => {}
    }
    if !matches!(outcome, AttemptOutcome::SkippedAuth) {
        result.attempted += 1;
    }
    apply_attempt_outcome(result, outcome)
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
) -> GroupAttemptResult {
    let mut result = GroupAttemptResult::new();
    let mut in_flight: FuturesUnordered<GroupAttemptFuture<'_>> = FuturesUnordered::new();
    let mut next_to_launch = 0usize;

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
                body,
                meta,
                request_auth,
                client_gemini_api_key,
                response_transform,
                &request_detail,
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
                        body,
                        meta,
                        request_auth,
                        client_gemini_api_key,
                        response_transform,
                        &request_detail,
                    );
                    None
                }
            }
        } else {
            in_flight.next().await
        };

        if let Some((item_index, outcome)) = completed {
            let upstream = &items[item_index];
            if apply_group_attempt_outcome(state, provider, upstream, &mut result, outcome) {
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
                    body,
                    meta,
                    request_auth,
                    client_gemini_api_key,
                    response_transform,
                    &request_detail,
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
    body: &'a ReplayableBody,
    meta: &'a RequestMeta,
    request_auth: &'a RequestAuth,
    client_gemini_api_key: Option<&'a str>,
    response_transform: FormatTransform,
    request_detail: &Option<RequestDetailSnapshot>,
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
    body: &'a ReplayableBody,
    meta: &'a RequestMeta,
    request_auth: &'a RequestAuth,
    client_gemini_api_key: Option<&'a str>,
    response_transform: FormatTransform,
    request_detail: &Option<RequestDetailSnapshot>,
) {
    let upstream = &items[item_index];
    let method = method.clone();
    let request_detail = request_detail.clone();
    in_flight.push(Box::pin(async move {
        let outcome = attempt::attempt_upstream(
            state,
            method,
            provider,
            upstream,
            inbound_path,
            upstream_path_with_query,
            headers,
            body,
            meta,
            request_auth,
            client_gemini_api_key,
            response_transform,
            request_detail,
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
