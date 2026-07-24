//! Token Proxy 应用运行时：不依赖 Tauri，可供 CLI/Tauri adapter 复用。
//!
//! 设计原则：
//! - App 不直接依赖 Tauri
//! - 账户与纯协议能力由 leaf crates 提供
//! - CLI/Tauri 通过 [`app::TokenProxyApp`] 组装运行时

pub mod account_lifecycle;
pub mod account_upstreams;
pub mod app;
mod pricing_refresh;
pub mod storage_usage;

pub use account_lifecycle::{
    AccountBindingMutationSummary, ConfigSideEffects, LifecycleFaults, SaveProxyConfigOutcome,
};
pub use token_proxy_runtime::logging;
