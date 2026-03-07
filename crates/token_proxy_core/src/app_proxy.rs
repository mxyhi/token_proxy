use std::sync::Arc;

use tokio::sync::RwLock;

/// 可选的“应用级代理”URL（例如用于更新请求、以及上游请求复用）。
///
/// Core 只关心：能不能读到一个 `Option<String>`；具体如何设置由外层负责。
pub type AppProxyState = Arc<RwLock<Option<String>>>;

pub fn new_state() -> AppProxyState {
    Arc::new(RwLock::new(None))
}

pub async fn set(state: &AppProxyState, value: Option<String>) {
    *state.write().await = value;
}
