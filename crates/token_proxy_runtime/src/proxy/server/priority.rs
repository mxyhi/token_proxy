use std::{collections::HashMap, sync::atomic::Ordering, time::Instant};

use axum::http::{HeaderMap, Method, Uri};
use tokio::sync::OnceCell;

use super::{
    dispatch::DispatchPlan,
    execute::{prepare_dispatch_request_for_model, OutboundRequest},
    prepared::PreparedRequest,
};
use crate::proxy::{
    config::{UpstreamOrderStrategy, UpstreamRuntime},
    cooldown_scope::CooldownScope,
    inbound::detect_inbound_api_format,
    upstream::{
        attempt_with_retries, dispatch_candidates, finalize_forward_result, merge_group_result,
        ForwardAttemptState, ForwardUpstreamResult,
    },
    ProxyState,
};

struct Route {
    plan: DispatchPlan,
    cooldown: CooldownScope,
    outbound: [OnceCell<OutboundRequest>; 2],
}

struct Candidate<'a> {
    route: &'a Route,
    upstream: &'a UpstreamRuntime,
}

/// 候选遍历以配置预编排的 priority 为外层，Provider 只决定协议/认证，不再阻断优先级。
pub(super) async fn forward_by_priority(
    state: &ProxyState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    prepared: &PreparedRequest,
    request_start: Instant,
    plans: Vec<DispatchPlan>,
    codex_scope: &CooldownScope,
) -> ForwardUpstreamResult {
    let inbound_format = detect_inbound_api_format(&prepared.path);
    let routes: Vec<_> = plans
        .into_iter()
        .map(|plan| Route {
            cooldown: codex_scope.for_provider(plan.provider, inbound_format),
            plan,
            outbound: [OnceCell::new(), OnceCell::new()],
        })
        .collect();
    // 显式 upstream/model 前缀在所有 Provider 中解析一次，避免跨 Provider 后丢失定向约束。
    let target = prepared
        .meta
        .original_model
        .as_deref()
        .and_then(|model| model.trim().split_once('/'))
        .filter(|(_, model)| !model.trim().is_empty())
        .map(|(prefix, _)| prefix)
        .filter(|prefix| {
            state
                .config
                .upstreams
                .values()
                .flat_map(|upstreams| &upstreams.groups)
                .flat_map(|group| &group.items)
                .any(|upstream| upstream.id == *prefix)
        });
    let requires_search =
        crate::proxy::model_capabilities::requires_native_search(&prepared.source_body);
    let mut summary = ForwardAttemptState::new();
    let mut repaired_bodies = HashMap::new();
    let mut has_route_candidate = false;
    let mut has_model_candidate = false;
    for group in &state.global_upstreams {
        let mut candidates = Vec::new();
        for address in &group.candidates {
            let Some(route) = routes
                .iter()
                .find(|route| route.plan.provider == address.provider)
            else {
                continue;
            };
            let upstream = &state.config.upstreams[&address.provider].groups[address.group].items
                [address.item];
            if !inbound_format.is_none_or(|format| upstream.supports_inbound(format))
                || target.is_some_and(|target| target != upstream.id)
            {
                continue;
            }
            has_route_candidate = true;
            if !upstream.supports_model(prepared.meta.original_model.as_deref()) {
                tracing::debug!(provider = route.plan.provider, upstream = %upstream.id,
                    exclusion_reason = "model_not_supported", "skipped global routing candidate");
                continue;
            }
            if requires_search
                && upstream
                    .capabilities_for_model(prepared.meta.original_model.as_deref())
                    .native_web_search
                    == Some(false)
            {
                tracing::debug!(upstream = %upstream.id, "skipped model explicitly lacking native web search");
                continue;
            }
            has_model_candidate = true;
            candidates.push(Candidate { route, upstream });
        }
        if candidates.is_empty() {
            continue;
        }
        let start = match state.config.upstream_strategy.order {
            UpstreamOrderStrategy::FillFirst => 0,
            UpstreamOrderStrategy::RoundRobin => {
                group.cursor.fetch_add(1, Ordering::Relaxed) % candidates.len()
            }
        };
        let selector_candidates: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.route.plan.provider,
                    candidate.upstream,
                    &candidate.route.cooldown,
                )
            })
            .collect();
        let order = state.upstream_selector.prioritize_candidates(
            &selector_candidates,
            (0..candidates.len())
                .map(|offset| (start + offset) % candidates.len())
                .collect(),
        );
        let candidates: Vec<_> = order.into_iter().map(|index| &candidates[index]).collect();
        let providers: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.route.plan.provider)
            .collect();
        tracing::debug!(
            priority = group.priority,
            candidates = candidates.len(),
            "dispatching global priority group"
        );
        dispatch_candidates(
            &state.config.upstream_strategy.dispatch,
            &providers,
            &mut repaired_bodies,
            |index, repaired_body| {
                let candidate = candidates[index];
                Box::pin(async move {
                    // OnceCell 合并同 Provider 的并发转换；没有执行到的 Provider 不读取/转换请求体。
                    let capabilities = candidate
                        .upstream
                        .capabilities_for_model(prepared.meta.original_model.as_deref());
                    let text_only = capabilities.image_input == Some(false);
                    let outbound = candidate.route.outbound[usize::from(text_only)]
                        .get_or_try_init(|| {
                            prepare_dispatch_request_for_model(
                                state,
                                uri,
                                headers,
                                prepared,
                                request_start,
                                &candidate.route.plan,
                                text_only,
                            )
                        })
                        .await;
                    let outbound = match outbound {
                        Ok(outbound) => outbound,
                        Err(response) => {
                            let mut failure = ForwardAttemptState::new();
                            failure.response = Some(response);
                            return failure;
                        }
                    };
                    let body = repaired_body.as_ref().unwrap_or(&outbound.body);
                    attempt_with_retries(
                        state,
                        method.clone(),
                        candidate.route.plan.provider,
                        candidate.upstream,
                        &prepared.path,
                        &outbound.path,
                        headers,
                        body,
                        &prepared.meta,
                        &prepared.request_auth,
                        prepared.client_gemini_api_key.as_deref(),
                        candidate.route.plan.response_transform,
                        prepared.request_detail.clone(),
                        &candidate.route.cooldown,
                    )
                    .await
                })
            },
            |_, result| merge_group_result(&mut summary, result),
        )
        .await;
        if summary.response.is_some() {
            break;
        }
    }
    summary.model_unsupported = has_route_candidate && !has_model_candidate;
    finalize_forward_result(
        state,
        prepared.plan.provider,
        &prepared.path,
        &prepared.meta,
        prepared.request_detail.as_ref(),
        summary,
    )
}
