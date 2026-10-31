use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{
    config::{UpstreamOrderStrategy, UpstreamRuntime},
    cooldown_scope::CooldownScope,
};

#[derive(Hash, PartialEq, Eq)]
struct CooldownKey {
    provider: String,
    upstream_key: String,
    scope: CooldownScope,
}

impl CooldownKey {
    fn new(provider: &str, upstream_key: &str, scope: &CooldownScope) -> Self {
        Self {
            provider: provider.to_string(),
            upstream_key: upstream_key.to_string(),
            scope: scope.clone(),
        }
    }
}

pub(crate) struct UpstreamSelectorRuntime {
    retryable_failure_cooldown: Duration,
    cooldowns: Mutex<HashMap<CooldownKey, Instant>>,
    /// 容量等待只约束一个实际上游凭据的一个模型，不改变整个账户健康状态。
    model_waits: Arc<Mutex<HashMap<(String, String, String), Instant>>>,
}

impl UpstreamSelectorRuntime {
    pub(crate) fn new_with_cooldown(retryable_failure_cooldown: Duration) -> Self {
        Self {
            retryable_failure_cooldown,
            cooldowns: Mutex::new(HashMap::new()),
            model_waits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn mark_model_wait(
        &self,
        provider: &str,
        upstream_key: &str,
        model: &str,
        headers: &axum::http::HeaderMap,
    ) {
        let now = Instant::now();
        let Some(until) = super::account_selector::retry_after_deadline(now, headers)
            .filter(|until| *until > now)
        else {
            return;
        };
        let mut waits = self.model_waits.lock().expect("model wait lock poisoned");
        waits.retain(|_, until| *until > now);
        let entry = waits
            .entry((provider.into(), upstream_key.into(), model.into()))
            .or_insert(until);
        *entry = (*entry).max(until);
        tracing::debug!(
            provider,
            upstream_key,
            model,
            "recorded model capacity Retry-After"
        );
    }

    pub(crate) fn model_wait_seconds(
        &self,
        provider: &str,
        upstream_key: &str,
        model: &str,
    ) -> Option<u64> {
        let now = Instant::now();
        let mut waits = self.model_waits.lock().expect("model wait lock poisoned");
        waits.retain(|_, until| *until > now);
        let remaining = waits
            .get(&(provider.into(), upstream_key.into(), model.into()))?
            .checked_duration_since(now)?;
        Some(
            remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0)),
        )
    }

    pub(crate) fn inherit_model_waits(&mut self, previous: &Self) {
        // 共享等待表，热加载前已经发出的请求稍后返回 Retry-After 时也必须生效。
        self.model_waits = previous.model_waits.clone();
    }

    pub(crate) fn order_group_scoped(
        &self,
        order: UpstreamOrderStrategy,
        provider: &str,
        items: &[UpstreamRuntime],
        cursor_start: usize,
        scope: &CooldownScope,
    ) -> Vec<usize> {
        let base_order = match order {
            UpstreamOrderStrategy::FillFirst => (0..items.len()).collect(),
            UpstreamOrderStrategy::RoundRobin => (0..items.len())
                .map(|offset| (cursor_start + offset) % items.len())
                .collect(),
        };
        self.prioritize_ready_upstreams(provider, items, base_order, scope)
    }

    pub(crate) fn mark_retryable_failure_scoped(
        &self,
        provider: &str,
        upstream_key: &str,
        scope: &CooldownScope,
    ) {
        let Some(until) = Instant::now().checked_add(self.retryable_failure_cooldown) else {
            return;
        };
        self.mark_cooldown_until(provider, upstream_key, scope, until);
    }

    pub(crate) fn clear_cooldown_scoped(
        &self,
        provider: &str,
        upstream_key: &str,
        scope: &CooldownScope,
    ) {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");
        prune_expired_cooldowns(&mut cooldowns, Instant::now());
        cooldowns.remove(&CooldownKey::new(provider, upstream_key, scope));
    }

    pub(crate) fn clear_provider_scope(&self, provider: &str, scope: &CooldownScope) {
        if scope.is_global() {
            return;
        }
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");
        prune_expired_cooldowns(&mut cooldowns, Instant::now());
        cooldowns.retain(|key, _| key.provider != provider || &key.scope != scope);
    }

    fn prioritize_ready_upstreams(
        &self,
        provider: &str,
        items: &[UpstreamRuntime],
        base_order: Vec<usize>,
        scope: &CooldownScope,
    ) -> Vec<usize> {
        let candidates: Vec<_> = items.iter().map(|item| (provider, item, scope)).collect();
        self.prioritize_candidates(&candidates, base_order)
    }

    /// 全局同级候选仍使用各自 Provider/会话的冷却域，不能把 Codex 冷却扩散给 API-key 上游。
    pub(crate) fn prioritize_candidates(
        &self,
        candidates: &[(&str, &UpstreamRuntime, &CooldownScope)],
        base_order: Vec<usize>,
    ) -> Vec<usize> {
        let now = Instant::now();
        let mut ready = Vec::with_capacity(base_order.len());
        let mut cooled = Vec::new();
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");
        prune_expired_cooldowns(&mut cooldowns, now);

        // 选择顺序遵循两层规则：
        // 1. 先保留既有策略（fill-first / round-robin）的基准顺序；
        // 2. 再把仍在 cooldown 的 upstream 后置，避免每个请求都重复撞到刚失败的账号。
        // 如果整组都在 cooldown，则按最早恢复时间优先，保证请求仍有机会探测恢复。
        for (position, item_index) in base_order.into_iter().enumerate() {
            let (provider, upstream, scope) = candidates[item_index];
            let upstream_key = upstream.selector_key.as_str();
            let key = CooldownKey::new(provider, upstream_key, scope);
            match cooldowns.get(&key).copied() {
                Some(until) if until > now => cooled.push((position, item_index, until)),
                Some(_) => {
                    cooldowns.remove(&key);
                    ready.push(item_index);
                }
                None => ready.push(item_index),
            }
        }

        if cooled.is_empty() {
            return ready;
        }

        cooled.sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0)));

        let cooled_indexes = cooled.into_iter().map(|(_, item_index, _)| item_index);
        if ready.is_empty() {
            return cooled_indexes.collect();
        }

        ready.extend(cooled_indexes);
        ready
    }

    fn mark_cooldown_until(
        &self,
        provider: &str,
        upstream_key: &str,
        scope: &CooldownScope,
        until: Instant,
    ) {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");
        let now = Instant::now();
        if until <= now {
            prune_expired_cooldowns(&mut cooldowns, now);
            return;
        }
        prune_expired_cooldowns(&mut cooldowns, now);
        let key = CooldownKey::new(provider, upstream_key, scope);
        match cooldowns.get_mut(&key) {
            Some(existing) if *existing >= until => {}
            Some(existing) => *existing = until,
            None => {
                cooldowns.insert(key, until);
            }
        }
    }
}

fn prune_expired_cooldowns(cooldowns: &mut HashMap<CooldownKey, Instant>, now: Instant) {
    cooldowns.retain(|_, until| *until > now);
}

// 单元测试拆到独立文件，保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
