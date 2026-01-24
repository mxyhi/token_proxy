//! token_proxy 的核心库：不依赖 Tauri，可供 CLI/Tauri 适配层复用。
//!
//! 设计原则：
//! - Core 不直接依赖 Tauri（Ports & Adapters）
//! - 运行时相关能力（路径、事件、UI）由外层注入

pub mod app_proxy;
pub mod jsonc;
pub mod logging;
pub mod oauth_util;
pub mod paths;

pub mod antigravity;
pub mod codex;
pub mod kiro;
pub mod proxy;

