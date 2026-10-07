use axum::{extract::DefaultBodyLimit, routing::any, Router};
use std::{collections::HashMap, sync::atomic::AtomicUsize};

use super::{proxy_request_with_connect_info, ProxyStateHandle};
use token_proxy_config::ProxyConfig;

pub(crate) fn build_upstream_cursors(config: &ProxyConfig) -> HashMap<String, Vec<AtomicUsize>> {
    let mut cursors: HashMap<String, Vec<AtomicUsize>> = HashMap::new();
    for (provider, upstreams) in &config.upstreams {
        let group_cursors = upstreams
            .groups
            .iter()
            .map(|_| AtomicUsize::new(0))
            .collect();
        cursors.insert(provider.clone(), group_cursors);
    }
    cursors
}

pub(crate) fn build_router(
    state: ProxyStateHandle,
    max_request_body_bytes: usize,
) -> Router<ProxyStateHandle> {
    Router::new()
        .route("/{*path}", any(proxy_request_with_connect_info))
        // 保留提取器的默认预算；Body 手动读取的硬上限由 ReplayableBody 执行。
        .layer(DefaultBodyLimit::max(max_request_body_bytes))
        .with_state(state)
}
