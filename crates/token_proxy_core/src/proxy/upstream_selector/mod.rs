use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use super::{config::UpstreamOrderStrategy, config::UpstreamRuntime};

#[derive(Hash, PartialEq, Eq)]
struct CooldownKey {
    provider: String,
    upstream_key: String,
}

impl CooldownKey {
    fn new(provider: &str, upstream_key: &str) -> Self {
        Self {
            provider: provider.to_string(),
            upstream_key: upstream_key.to_string(),
        }
    }
}

pub(crate) struct UpstreamSelectorRuntime {
    retryable_failure_cooldown: Duration,
    cooldowns: Mutex<HashMap<CooldownKey, Instant>>,
}

impl UpstreamSelectorRuntime {
    pub(crate) fn new_with_cooldown(retryable_failure_cooldown: Duration) -> Self {
        Self {
            retryable_failure_cooldown,
            cooldowns: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn order_group(
        &self,
        order: UpstreamOrderStrategy,
        provider: &str,
        items: &[UpstreamRuntime],
        cursor_start: usize,
    ) -> Vec<usize> {
        let base_order = match order {
            UpstreamOrderStrategy::FillFirst => (0..items.len()).collect(),
            UpstreamOrderStrategy::RoundRobin => (0..items.len())
                .map(|offset| (cursor_start + offset) % items.len())
                .collect(),
        };
        self.prioritize_ready_upstreams(provider, items, base_order)
    }

    pub(crate) fn mark_retryable_failure(&self, provider: &str, upstream_key: &str) {
        let Some(until) = Instant::now().checked_add(self.retryable_failure_cooldown) else {
            return;
        };
        self.mark_cooldown_until(provider, upstream_key, until);
    }

    pub(crate) fn clear_cooldown(&self, provider: &str, upstream_key: &str) {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");
        cooldowns.remove(&CooldownKey::new(provider, upstream_key));
    }

    fn prioritize_ready_upstreams(
        &self,
        provider: &str,
        items: &[UpstreamRuntime],
        base_order: Vec<usize>,
    ) -> Vec<usize> {
        let now = Instant::now();
        let mut ready = Vec::with_capacity(base_order.len());
        let mut cooled = Vec::new();
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");

        // 选择顺序遵循两层规则：
        // 1. 先保留既有策略（fill-first / round-robin）的基准顺序；
        // 2. 再把仍在 cooldown 的 upstream 后置，避免每个请求都重复撞到刚失败的账号。
        // 如果整组都在 cooldown，则按最早恢复时间优先，保证请求仍有机会探测恢复。
        for (position, item_index) in base_order.into_iter().enumerate() {
            let upstream_key = items[item_index].selector_key.as_str();
            let key = CooldownKey::new(provider, upstream_key);
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

    fn mark_cooldown_until(&self, provider: &str, upstream_key: &str, until: Instant) {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("selector cooldown lock poisoned");
        let key = CooldownKey::new(provider, upstream_key);
        match cooldowns.get_mut(&key) {
            Some(existing) if *existing >= until => {}
            Some(existing) => *existing = until,
            None => {
                cooldowns.insert(key, until);
            }
        }
    }
}

// 单元测试拆到独立文件，保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
