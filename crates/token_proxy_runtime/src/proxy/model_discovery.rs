use std::collections::{HashMap, HashSet};

use tokio::sync::RwLock;

pub use token_proxy_storage::dashboard::{UpstreamModelProbe, UpstreamModelProbeStatus};

/// 上游模型目录条目；展示名只用于 `/v1/models` 与 Codex manifest 输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelCatalogEntry {
    pub(crate) id: String,
    pub(crate) display_name: Option<String>,
}

/// 同一 upstream id 可按账号拆成多个运行时上游，缓存需按 provider + upstream + account 区分。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ModelCatalogKey {
    pub(crate) provider: String,
    pub(crate) upstream_id: String,
    pub(crate) account_id: Option<String>,
}

impl ModelCatalogKey {
    fn of_probe(probe: &UpstreamModelProbe) -> Self {
        Self {
            provider: probe.provider.clone(),
            upstream_id: probe.upstream_id.clone(),
            account_id: probe.account_id.clone(),
        }
    }
}

#[derive(Default)]
struct DiscoveryState {
    /// Dashboard 展示用的探测状态。
    probes: Vec<UpstreamModelProbe>,
    /// 最近一次成功拉到的上游原始目录（未叠加本地 available_models / 映射）。
    /// 刷新失败不覆盖，避免单次抖动让模型列表变空；本地配置在读取时按当前 config 叠加。
    fetched: HashMap<ModelCatalogKey, Vec<ModelCatalogEntry>>,
}

/// 模型目录缓存：只在启动、配置热加载与手动刷新时写入，请求路径只读。
#[derive(Default)]
pub(crate) struct UpstreamModelDiscoveryCache {
    state: RwLock<DiscoveryState>,
}

impl UpstreamModelDiscoveryCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn snapshot(&self) -> Vec<UpstreamModelProbe> {
        self.state.read().await.probes.clone()
    }

    pub(crate) async fn fetched_catalogs(
        &self,
    ) -> HashMap<ModelCatalogKey, Vec<ModelCatalogEntry>> {
        self.state.read().await.fetched.clone()
    }

    pub(crate) async fn fetched_catalog(
        &self,
        key: &ModelCatalogKey,
    ) -> Option<Vec<ModelCatalogEntry>> {
        self.state.read().await.fetched.get(key).cloned()
    }

    /// 开始新一轮刷新：探测状态置为 pending，并清掉已不在配置里的上游目录。
    pub(crate) async fn begin_refresh(&self, pending: Vec<UpstreamModelProbe>) {
        let keys = pending
            .iter()
            .map(ModelCatalogKey::of_probe)
            .collect::<HashSet<_>>();
        let mut state = self.state.write().await;
        state.fetched.retain(|key, _| keys.contains(key));
        state.probes = pending;
    }

    /// 写入单个上游的探测结果；`fetched` 为 None 表示本次失败或无远端目录，保留旧目录。
    pub(crate) async fn complete_probe(
        &self,
        probe: UpstreamModelProbe,
        fetched: Option<Vec<ModelCatalogEntry>>,
    ) {
        let key = ModelCatalogKey::of_probe(&probe);
        let mut state = self.state.write().await;
        let Some(slot) = state
            .probes
            .iter_mut()
            .find(|current| ModelCatalogKey::of_probe(current) == key)
        else {
            // 配置热加载后旧刷新任务可能晚到，丢弃已不在当前配置中的结果。
            tracing::debug!(
                provider = %key.provider,
                upstream = %key.upstream_id,
                "drop stale model discovery result"
            );
            return;
        };
        *slot = probe;
        if let Some(entries) = fetched {
            state.fetched.insert(key, entries);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> ModelCatalogEntry {
        ModelCatalogEntry {
            id: id.to_string(),
            display_name: None,
        }
    }

    fn key(upstream_id: &str) -> ModelCatalogKey {
        ModelCatalogKey {
            provider: "openai".to_string(),
            upstream_id: upstream_id.to_string(),
            account_id: None,
        }
    }

    fn completed(upstream_id: &str, status: UpstreamModelProbeStatus) -> UpstreamModelProbe {
        UpstreamModelProbe::completed(upstream_id, "openai", None, status, None, Vec::new())
    }

    #[tokio::test]
    async fn failed_refresh_keeps_last_fetched_catalog() {
        let cache = UpstreamModelDiscoveryCache::new();
        cache
            .begin_refresh(vec![UpstreamModelProbe::pending("alpha", "openai", None)])
            .await;
        cache
            .complete_probe(
                completed("alpha", UpstreamModelProbeStatus::Ok),
                Some(vec![entry("gpt-5")]),
            )
            .await;

        cache
            .begin_refresh(vec![UpstreamModelProbe::pending("alpha", "openai", None)])
            .await;
        cache
            .complete_probe(completed("alpha", UpstreamModelProbeStatus::Failed), None)
            .await;

        assert_eq!(
            cache.fetched_catalogs().await.get(&key("alpha")),
            Some(&vec![entry("gpt-5")])
        );
        assert_eq!(
            cache.snapshot().await[0].status,
            UpstreamModelProbeStatus::Failed
        );
    }

    #[tokio::test]
    async fn refresh_prunes_removed_upstreams_and_ignores_stale_results() {
        let cache = UpstreamModelDiscoveryCache::new();
        cache
            .begin_refresh(vec![
                UpstreamModelProbe::pending("alpha", "openai", None),
                UpstreamModelProbe::pending("beta", "openai", None),
            ])
            .await;
        for upstream_id in ["alpha", "beta"] {
            cache
                .complete_probe(
                    completed(upstream_id, UpstreamModelProbeStatus::Ok),
                    Some(vec![entry("gpt-5")]),
                )
                .await;
        }

        cache
            .begin_refresh(vec![UpstreamModelProbe::pending("alpha", "openai", None)])
            .await;
        cache
            .complete_probe(
                completed("beta", UpstreamModelProbeStatus::Ok),
                Some(vec![entry("late")]),
            )
            .await;

        let fetched = cache.fetched_catalogs().await;
        assert!(fetched.contains_key(&key("alpha")));
        assert!(!fetched.contains_key(&key("beta")));
        assert_eq!(cache.snapshot().await.len(), 1);
    }
}
