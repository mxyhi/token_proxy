pub use token_proxy_core::app_proxy::{set, AppProxyState};

pub fn new_state() -> AppProxyState {
    token_proxy_core::app_proxy::new_state()
}
