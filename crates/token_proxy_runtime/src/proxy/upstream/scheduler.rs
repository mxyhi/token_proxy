use std::{collections::HashMap, future::Future, pin::Pin};

use futures_util::{stream::FuturesUnordered, StreamExt};

use super::dispatch::GroupAttemptResult;
use crate::proxy::{config::UpstreamDispatchRuntime, request_body::ReplayableBody};

pub(crate) type CandidateFuture<'a> = Pin<Box<dyn Future<Output = GroupAttemptResult> + Send + 'a>>;
type ScheduledFuture<'a> = Pin<Box<dyn Future<Output = (usize, GroupAttemptResult)> + Send + 'a>>;

/// 共用同优先级调度器：单上游重试留在各自 future 内，避免阻塞其它竞速候选。
/// 修复后的 body 按 Provider 隔离；下一优先级由调用方在整组耗尽后推进。
pub(crate) async fn dispatch_candidates<'a>(
    dispatch: &UpstreamDispatchRuntime,
    providers: &[&str],
    repaired_bodies: &mut HashMap<String, ReplayableBody>,
    mut launch: impl FnMut(usize, Option<ReplayableBody>) -> CandidateFuture<'a>,
    mut completed: impl FnMut(usize, GroupAttemptResult) -> bool,
) {
    let (initial, capacity, delay) = match dispatch {
        UpstreamDispatchRuntime::Serial => (1, 1, None),
        UpstreamDispatchRuntime::Race { max_parallel } => (*max_parallel, *max_parallel, None),
        UpstreamDispatchRuntime::Hedged {
            delay,
            max_parallel,
        } => (1, *max_parallel, Some(*delay)),
    };
    let mut in_flight: FuturesUnordered<ScheduledFuture<'a>> = FuturesUnordered::new();
    let mut next = 0;
    let mut slots = initial;
    loop {
        for _ in 0..slots.min(providers.len().saturating_sub(next)) {
            let index = next;
            next += 1;
            let future = launch(index, repaired_bodies.get(providers[index]).cloned());
            in_flight.push(Box::pin(async move { (index, future.await) }));
        }
        if in_flight.is_empty() {
            return;
        }
        let result = if let Some(delay) =
            delay.filter(|_| next < providers.len() && in_flight.len() < capacity)
        {
            tokio::select! {
                result = in_flight.next() => result,
                _ = tokio::time::sleep(delay) => {
                    slots = 1;
                    continue;
                }
            }
        } else {
            in_flight.next().await
        };
        if let Some((index, result)) = result {
            if let Some(body) = result.effective_body.clone() {
                repaired_bodies.insert(providers[index].to_string(), body);
            }
            if completed(index, result) {
                // 丢弃其它 future，保证只提交一个成功/不可重试的终态响应。
                return;
            }
        }
        slots = match dispatch {
            UpstreamDispatchRuntime::Race { .. } => capacity.saturating_sub(in_flight.len()),
            _ => usize::from(in_flight.len() < capacity),
        };
    }
}
