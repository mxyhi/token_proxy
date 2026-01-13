use axum::{
    extract::DefaultBodyLimit,
    routing::any,
    Router,
};
use std::{collections::HashMap, sync::atomic::AtomicUsize};

use crate::proxy::config::ProxyConfig;
use super::{proxy_request, ProxyStateHandle};

pub(crate) fn build_upstream_cursors(
    config: &ProxyConfig,
) -> HashMap<String, Vec<AtomicUsize>> {
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
        .route("/{*path}", any(proxy_request))
        // 限制入站请求体，避免超大请求占用内存/临时盘并拖慢首字节。
        .layer(DefaultBodyLimit::max(max_request_body_bytes))
        .with_state(state)
}
