use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

use crate::proxy::config::ProxyConfig;

/// 只保存配置索引，避免为每条请求克隆包含凭据和模型映射的 UpstreamRuntime。
pub(crate) struct UpstreamAddress {
    pub(crate) provider: String,
    pub(crate) group: usize,
    pub(crate) item: usize,
}

pub(crate) struct GlobalUpstreamGroup {
    pub(crate) priority: i32,
    pub(crate) candidates: Vec<UpstreamAddress>,
    pub(crate) cursor: AtomicUsize,
}

/// 配置加载/重载时编排全局优先级和稳定的同级顺序，运行时只过滤当前请求候选。
pub(crate) fn build_global_upstreams(config: &ProxyConfig) -> Vec<GlobalUpstreamGroup> {
    let mut groups: BTreeMap<i32, Vec<(String, UpstreamAddress)>> = BTreeMap::new();
    for (provider, upstreams) in &config.upstreams {
        for (group_index, group) in upstreams.groups.iter().enumerate() {
            let min_id = group
                .items
                .iter()
                .map(|upstream| upstream.id.as_str())
                .min()
                .unwrap_or("");
            for item in 0..group.items.len() {
                groups.entry(group.priority).or_default().push((
                    min_id.to_string(),
                    UpstreamAddress {
                        provider: provider.clone(),
                        group: group_index,
                        item,
                    },
                ));
            }
        }
    }
    let groups: Vec<_> = groups
        .into_iter()
        .rev()
        .map(|(priority, mut candidates)| {
            // 每组的 min-id 只计算一次；同 Provider 保留配置顺序（包括多 API key）。
            candidates.sort_by(|(left_rank, left), (right_rank, right)| {
                left_rank
                    .cmp(right_rank)
                    .then_with(|| left.provider.cmp(&right.provider))
                    .then_with(|| left.item.cmp(&right.item))
            });
            GlobalUpstreamGroup {
                priority,
                candidates: candidates.into_iter().map(|(_, address)| address).collect(),
                cursor: AtomicUsize::new(0),
            }
        })
        .collect();
    tracing::debug!(
        priority_groups = groups.len(),
        "compiled global upstream priority groups"
    );
    groups
}
