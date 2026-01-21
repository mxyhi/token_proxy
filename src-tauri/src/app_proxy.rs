use std::sync::Arc;

use tokio::sync::RwLock;

pub(crate) type AppProxyState = Arc<RwLock<Option<String>>>;

pub(crate) fn new_state() -> AppProxyState {
    Arc::new(RwLock::new(None))
}

pub(crate) async fn set(state: &AppProxyState, value: Option<String>) {
    *state.write().await = value;
}
