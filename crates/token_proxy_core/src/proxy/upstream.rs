use axum::{
    body::{Body, Bytes},
    http::{header::AUTHORIZATION, HeaderMap, Method, StatusCode},
    response::Response,
};
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const GEMINI_API_KEY_QUERY: &str = "key";
const LOCAL_UPSTREAM_ID: &str = "local";

mod attempt;
mod kiro;
mod kiro_headers;
mod kiro_http;
mod request;
mod result;
mod utils;

use utils::resolve_group_start;

#[cfg(test)]
use crate::proxy::redact::redact_query_param_value;

use super::{
    config::{InboundApiFormat, ProviderUpstreams, UpstreamDispatchRuntime, UpstreamRuntime},
    gemini, http,
    http::RequestAuth,
    inbound::detect_inbound_api_format,
    openai_compat::FormatTransform,
    request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot,
    ProxyState, RequestMeta,
};

const REQUEST_MODEL_MAPPING_LIMIT_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn aggregate_model_catalog_request(
    state: Arc<ProxyState>,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    request_auth: &RequestAuth,
) -> Response {
    let Some(provider_upstreams) = state.config.provider_upstreams(provider) else {
        return http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured.");
    };

    let mut sources: Vec<(String, Vec<String>)> = Vec::new();
    let mut successful = 0usize;
    let meta = RequestMeta {
        stream: false,
        original_model: None,
        mapped_model: None,
        reasoning_effort: None,
        estimated_input_tokens: None,
    };
    let empty_body = ReplayableBody::from_bytes(Bytes::new());

    for group in &provider_upstreams.groups {
        for upstream in &group.items {
            let upstream_model_catalog = fetch_upstream_model_catalog(
                state.as_ref(),
                provider,
                upstream,
                inbound_path,
                upstream_path_with_query,
                headers,
                &meta,
                request_auth,
                &empty_body,
            )
            .await;
            let mut models = upstream.advertised_model_ids.clone();
            match upstream_model_catalog {
                Ok(fetched_models) => {
                    successful += 1;
                    merge_model_catalog_ids(&mut models, fetched_models);
                    sources.push((upstream.id.clone(), models));
                }
                Err(err) => {
                    if !models.is_empty() {
                        successful += 1;
                        sources.push((upstream.id.clone(), models));
                        continue;
                    }
                    tracing::warn!(
                        provider = %provider,
                        upstream = %upstream.id,
                        error = %err,
                        "failed to fetch upstream model catalog"
                    );
                }
            }
        }
    }

    if successful == 0 {
        return http::error_response(
            StatusCode::BAD_GATEWAY,
            "No upstream model catalog available.",
        );
    }

    let response_body = build_model_catalog_response_body(&sources, state.config.model_list_prefix);
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    http::build_response(
        StatusCode::OK,
        response_headers,
        Body::from(response_body.to_string()),
    )
}

fn merge_model_catalog_ids(target: &mut Vec<String>, extra: Vec<String>) {
    let mut seen = target.iter().cloned().collect::<HashSet<_>>();
    for model in extra {
        if seen.insert(model.clone()) {
            target.push(model);
        }
    }
}

pub(super) struct ForwardUpstreamResult {
    pub(super) response: Response,
    pub(super) should_fallback: bool,
}

pub(super) async fn forward_upstream_request(
    state: Arc<ProxyState>,
    method: Method,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    client_gemini_api_key: Option<String>,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> ForwardUpstreamResult {
    let inbound_format = detect_inbound_api_format(inbound_path);
    let upstreams = match resolve_provider_upstreams(
        &state,
        provider,
        inbound_path,
        meta,
        request_detail.as_ref(),
    ) {
        Ok(upstreams) => upstreams,
        Err(response) => {
            return ForwardUpstreamResult {
                response,
                // Treat missing upstream config as retryable for higher-level fallback (e.g. cross-provider).
                should_fallback: true,
            };
        }
    };
    let summary = run_upstream_groups(
        &state,
        method,
        provider,
        inbound_format,
        inbound_path,
        upstream_path_with_query,
        headers,
        body,
        meta,
        request_auth,
        client_gemini_api_key.as_deref(),
        response_transform,
        request_detail.clone(),
        upstreams,
    )
    .await;
    if let Some(response) = summary.response {
        return ForwardUpstreamResult {
            response,
            should_fallback: false,
        };
    }
    let should_fallback = summary.last_retry_response.is_some()
        || summary.last_timeout_error.is_some()
        || summary.last_retry_error.is_some()
        || summary.attempted == 0;
    let response = finalize_forward_response(
        &state,
        provider,
        inbound_path,
        meta,
        request_detail.as_ref(),
        summary,
    );
    ForwardUpstreamResult {
        response,
        should_fallback,
    }
}

struct GroupAttemptResult {
    response: Option<Response>,
    attempted: usize,
    missing_auth: bool,
    last_timeout_error: Option<String>,
    last_retry_error: Option<String>,
    last_retry_response: Option<Response>,
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

struct ForwardAttemptState {
    response: Option<Response>,
    attempted: usize,
    missing_auth: bool,
    last_timeout_error: Option<String>,
    last_retry_error: Option<String>,
    last_retry_response: Option<Response>,
}

impl ForwardAttemptState {
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

enum AttemptOutcome {
    Success(Response),
    Retryable {
        message: String,
        response: Option<Response>,
        is_timeout: bool,
        should_cooldown: bool,
    },
    Fatal(Response),
    SkippedAuth,
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

pub(super) struct PreparedUpstreamRequest {
    upstream_path_with_query: String,
    upstream_url: String,
    request_headers: HeaderMap,
    proxy_url: Option<String>,
    selected_account_id: Option<String>,
    meta: RequestMeta,
}

struct ResolvedUpstreamAuth {
    upstream_url: String,
    auth: http::UpstreamAuthHeader,
    extra_headers: Option<HeaderMap>,
    proxy_url: Option<String>,
    selected_account_id: Option<String>,
}

fn resolve_provider_upstreams<'a>(
    state: &'a ProxyState,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
) -> Result<&'a ProviderUpstreams, Response> {
    match state.config.provider_upstreams(provider) {
        Some(upstreams) => Ok(upstreams),
        None => {
            result::log_upstream_error_if_needed(
                &state.log,
                request_detail,
                meta,
                provider,
                LOCAL_UPSTREAM_ID,
                None,
                inbound_path,
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.".to_string(),
                Instant::now(),
            );
            Err(http::error_response(
                StatusCode::BAD_GATEWAY,
                "No available upstream configured.",
            ))
        }
    }
}

async fn run_upstream_groups(
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
        // Only rotate within the highest priority group; retry network failures before degrading.
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

fn finalize_forward_response(
    state: &ProxyState,
    provider: &str,
    inbound_path: &str,
    meta: &RequestMeta,
    request_detail: Option<&RequestDetailSnapshot>,
    summary: ForwardAttemptState,
) -> Response {
    if summary.attempted == 0 && summary.missing_auth {
        result::log_upstream_error_if_needed(
            &state.log,
            request_detail,
            meta,
            provider,
            LOCAL_UPSTREAM_ID,
            None,
            inbound_path,
            StatusCode::UNAUTHORIZED,
            "Missing upstream API key.".to_string(),
            Instant::now(),
        );
        return http::error_response(StatusCode::UNAUTHORIZED, "Missing upstream API key.");
    }
    if let Some(response) = summary.last_retry_response {
        return response;
    }
    if let Some(err) = summary.last_timeout_error {
        return http::error_response(StatusCode::GATEWAY_TIMEOUT, err);
    }
    if let Some(err) = summary.last_retry_error {
        return http::error_response(
            StatusCode::BAD_GATEWAY,
            format!("Upstream request failed: {err}"),
        );
    }
    http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured.")
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

async fn prepare_upstream_request(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
) -> Result<PreparedUpstreamRequest, AttemptOutcome> {
    let mapped_meta = build_mapped_meta(meta, upstream);
    let upstream_path_with_query =
        resolve_upstream_path_with_query(provider, upstream_path_with_query, &mapped_meta);
    let upstream_url = upstream.upstream_url(&upstream_path_with_query);
    let resolved = resolve_upstream_auth(
        state,
        provider,
        upstream,
        request_auth,
        &upstream_path_with_query,
        &upstream_url,
    )
    .await?;
    let ResolvedUpstreamAuth {
        upstream_url,
        auth,
        extra_headers,
        proxy_url,
        selected_account_id,
    } = resolved;
    let request_headers = request::build_request_headers(
        provider,
        inbound_path,
        headers,
        auth,
        extra_headers.as_ref(),
        upstream.header_overrides.as_deref(),
    );
    Ok(PreparedUpstreamRequest {
        upstream_path_with_query,
        upstream_url,
        request_headers,
        proxy_url,
        selected_account_id,
        meta: mapped_meta,
    })
}

async fn resolve_upstream_auth(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    request_auth: &RequestAuth,
    upstream_path_with_query: &str,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    if provider == "gemini" {
        let (upstream_url, auth) = request::resolve_gemini_upstream(
            upstream,
            request_auth,
            upstream_path_with_query,
            upstream_url,
        )?;
        return Ok(ResolvedUpstreamAuth {
            upstream_url,
            auth,
            extra_headers: None,
            proxy_url: upstream.proxy_url.clone(),
            selected_account_id: None,
        });
    }
    if provider == "kiro" {
        return resolve_kiro_upstream(state, upstream, upstream_url).await;
    }
    if provider == "codex" {
        return resolve_codex_upstream(state, upstream, upstream_url).await;
    }
    let auth = match http::resolve_upstream_auth(provider, upstream, request_auth) {
        Ok(Some(auth)) => auth,
        Ok(None) => return Err(AttemptOutcome::SkippedAuth),
        Err(response) => return Err(AttemptOutcome::Fatal(response)),
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth,
        extra_headers: None,
        proxy_url: upstream.proxy_url.clone(),
        selected_account_id: None,
    })
}

async fn resolve_kiro_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let ordered_account_ids = if upstream
        .kiro_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        None
    } else {
        Some(ordered_runtime_account_ids(state, "kiro").await)
    };
    let (selected_account_id, record) = state
        .kiro_accounts
        .resolve_account_record_with_order(
            upstream.kiro_account_id.as_deref(),
            ordered_account_ids.as_deref(),
        )
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err))
        })?;
    let global_proxy_url = state.kiro_accounts.app_proxy_url().await;
    let proxy_url = record
        .proxy_url
        .clone()
        .or_else(|| upstream.proxy_url.clone())
        .or(global_proxy_url);
    let value = http::bearer_header(&record.access_token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers: None,
        proxy_url,
        selected_account_id: Some(selected_account_id),
    })
}

async fn resolve_codex_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let ordered_account_ids = if upstream
        .codex_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        None
    } else {
        Some(ordered_runtime_account_ids(state, "codex").await)
    };
    let (selected_account_id, record) = state
        .codex_accounts
        .resolve_account_record_with_order(
            upstream.codex_account_id.as_deref(),
            ordered_account_ids.as_deref(),
        )
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err))
        })?;
    let global_proxy_url = state.codex_accounts.app_proxy_url().await;
    let proxy_url = record
        .proxy_url
        .clone()
        .or_else(|| upstream.proxy_url.clone())
        .or(global_proxy_url);
    let value = http::bearer_header(&record.access_token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    let mut extra_headers = HeaderMap::new();
    if let Some(account_id) = record.account_id.as_deref() {
        if let Ok(value) = axum::http::HeaderValue::from_str(account_id) {
            extra_headers.insert(
                axum::http::HeaderName::from_static("chatgpt-account-id"),
                value,
            );
        }
    }
    let extra_headers = if extra_headers.is_empty() {
        None
    } else {
        Some(extra_headers)
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers,
        proxy_url,
        selected_account_id: Some(selected_account_id),
    })
}

pub(super) async fn ordered_runtime_account_ids(state: &ProxyState, provider: &str) -> Vec<String> {
    let account_ids = match provider {
        "kiro" => state.kiro_accounts.list_accounts().await.map(|items| {
            items
                .into_iter()
                .map(|item| item.account_id)
                .collect::<Vec<_>>()
        }),
        "codex" => state.codex_accounts.list_accounts().await.map(|items| {
            items
                .into_iter()
                .map(|item| item.account_id)
                .collect::<Vec<_>>()
        }),
        _ => Ok(Vec::new()),
    }
    .unwrap_or_default();
    state
        .account_selector
        .order_accounts(provider, &account_ids)
}

fn build_mapped_meta(meta: &RequestMeta, upstream: &UpstreamRuntime) -> RequestMeta {
    let upstream_input_model = meta.original_model.as_deref().map(|original| {
        strip_target_upstream_prefix(original, upstream.id.as_str())
            .unwrap_or_else(|| original.to_string())
    });
    // 只有当实际发生映射时才设置 mapped_model，避免与 original_model 重复
    let mapped_model = upstream_input_model
        .as_deref()
        .and_then(|original| upstream.map_model(original))
        .or_else(|| {
            let mapped_input = upstream_input_model.as_deref()?;
            let original = meta.original_model.as_deref()?;
            (mapped_input != original).then(|| mapped_input.to_string())
        });
    let (mapped_model, reasoning_effort) =
        normalize_mapped_model_reasoning_suffix(mapped_model, meta.reasoning_effort.clone());
    RequestMeta {
        stream: meta.stream,
        original_model: meta.original_model.clone(),
        mapped_model,
        reasoning_effort,
        estimated_input_tokens: meta.estimated_input_tokens,
    }
}

async fn fetch_upstream_model_catalog(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    body: &ReplayableBody,
) -> Result<Vec<String>, String> {
    let prepared = prepare_upstream_request(
        state,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        meta,
        request_auth,
    )
    .await
    .map_err(|_| "Failed to prepare upstream model catalog request.".to_string())?;

    let client = state
        .http_clients
        .client_for_proxy_url(prepared.proxy_url.as_deref())?;
    let request_body = body
        .to_reqwest_body()
        .await
        .map_err(|err| format!("Failed to build upstream request body: {err}"))?;
    let request = client
        .request(Method::GET, &prepared.upstream_url)
        .headers(prepared.request_headers)
        .body(request_body);
    let response = tokio::time::timeout(state.config.upstream_no_data_timeout, request.send())
        .await
        .map_err(|_| "Timed out fetching upstream model catalog.".to_string())?
        .map_err(|err| format!("Failed to fetch upstream model catalog: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Upstream model catalog returned status {}.",
            response.status()
        ));
    }

    let value = response
        .json::<Value>()
        .await
        .map_err(|err| format!("Failed to parse upstream model catalog JSON: {err}"))?;
    Ok(extract_model_ids_from_catalog(&value))
}

fn extract_model_ids_from_catalog(value: &Value) -> Vec<String> {
    if let Some(items) = value.get("data").and_then(Value::as_array) {
        return items
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("name").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches("models/").to_string())
        .collect()
}

fn build_model_catalog_response_body(
    sources: &[(String, Vec<String>)],
    include_prefixed: bool,
) -> Value {
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut upstreams_by_model: HashMap<String, Vec<String>> = HashMap::new();
    let mut base_order = Vec::new();

    for (upstream_id, models) in sources {
        let mut seen = HashSet::new();
        for model in models {
            let trimmed = model.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                continue;
            }
            if !upstreams_by_model.contains_key(trimmed) {
                base_order.push(trimmed.to_string());
            }
            upstreams_by_model
                .entry(trimmed.to_string())
                .or_default()
                .push(upstream_id.clone());
        }
    }

    let mut data = Vec::new();
    for model in base_order {
        let Some(upstream_ids) = upstreams_by_model.get(&model) else {
            continue;
        };
        if include_prefixed {
            if upstream_ids.len() > 1 {
                data.push(model_catalog_item(model.as_str(), model.as_str(), created));
            }
            for upstream_id in upstream_ids {
                let prefixed = format!("{upstream_id}/{model}");
                data.push(model_catalog_item(&prefixed, upstream_id.as_str(), created));
            }
            continue;
        }
        data.push(model_catalog_item(model.as_str(), "token_proxy", created));
    }

    json!({
        "object": "list",
        "data": data,
    })
}

fn model_catalog_item(id: &str, owned_by: &str, created: i64) -> Value {
    json!({
        "id": id,
        "object": "model",
        "created": created,
        "owned_by": owned_by,
    })
}

fn requested_target_upstream_id(
    upstreams: &ProviderUpstreams,
    original_model: Option<&str>,
) -> Option<String> {
    let original_model = original_model?.trim();
    let (prefix, rest) = original_model.split_once('/')?;
    if prefix.trim().is_empty() || rest.trim().is_empty() {
        return None;
    }
    upstreams
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .find(|upstream| upstream.id == prefix)
        .map(|upstream| upstream.id.clone())
}

fn strip_target_upstream_prefix(model: &str, upstream_id: &str) -> Option<String> {
    let (prefix, rest) = model.split_once('/')?;
    if prefix != upstream_id || rest.trim().is_empty() {
        return None;
    }
    Some(rest.to_string())
}

fn normalize_mapped_model_reasoning_suffix(
    mapped_model: Option<String>,
    reasoning_effort: Option<String>,
) -> (Option<String>, Option<String>) {
    let Some(mapped_model) = mapped_model else {
        return (None, reasoning_effort);
    };
    let Some((base_model, mapped_effort)) =
        super::server_helpers::parse_openai_reasoning_effort_from_model_suffix(&mapped_model)
    else {
        return (Some(mapped_model), reasoning_effort);
    };

    // If the user already specified an explicit effort in the incoming `model`, keep it.
    let reasoning_effort = reasoning_effort.or(Some(mapped_effort));
    (Some(base_model), reasoning_effort)
}

fn resolve_upstream_path_with_query(
    provider: &str,
    upstream_path_with_query: &str,
    meta: &RequestMeta,
) -> String {
    if provider != "gemini" || meta.model_override().is_none() {
        return upstream_path_with_query.to_string();
    }
    let Some(mapped_model) = meta.mapped_model.as_deref() else {
        return upstream_path_with_query.to_string();
    };
    let (path, query) = request::split_path_query(upstream_path_with_query);
    let replaced = gemini::replace_gemini_model_in_path(path, mapped_model)
        .unwrap_or_else(|| path.to_string());
    match query {
        Some(query) => format!("{replaced}?{query}"),
        None => replaced,
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "upstream.test.rs"]
mod tests;
