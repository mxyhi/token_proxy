mod anthropic_compat;
mod antigravity_compat;
mod antigravity_schema;
mod codex_compat;
mod compat_content;
mod compat_reason;
pub mod config;
pub mod dashboard;
mod gemini;
mod gemini_compat;
mod http;
mod http_client;
mod inbound;
mod kiro;
mod log;
pub mod logs;
mod model;
mod openai_compat;
mod redact;
mod request_body;
pub mod request_detail;
mod request_token_estimate;
mod response;
mod server;
mod server_helpers;
pub mod service;
pub mod sqlite;
mod sse;
mod token_estimator;
pub mod token_rate;
mod upstream;
mod upstream_selector;
mod usage;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};

use crate::antigravity::AntigravityAccountStore;
use crate::codex::CodexAccountStore;
use crate::kiro::KiroAccountStore;

struct ProxyState {
    config: config::ProxyConfig,
    http_clients: http_client::ProxyHttpClients,
    log: Arc<log::LogWriter>,
    cursors: HashMap<String, Vec<AtomicUsize>>,
    upstream_selector: upstream_selector::UpstreamSelectorRuntime,
    request_detail: Arc<request_detail::RequestDetailCapture>,
    token_rate: Arc<token_rate::TokenRateTracker>,
    kiro_accounts: Arc<KiroAccountStore>,
    codex_accounts: Arc<CodexAccountStore>,
    antigravity_accounts: Arc<AntigravityAccountStore>,
}

struct RequestMeta {
    stream: bool,
    original_model: Option<String>,
    mapped_model: Option<String>,
    reasoning_effort: Option<String>,
    estimated_input_tokens: Option<u64>,
}

impl RequestMeta {
    fn model_override(&self) -> Option<&str> {
        match (self.original_model.as_deref(), self.mapped_model.as_deref()) {
            (Some(original), Some(mapped)) if original != mapped => Some(original),
            _ => None,
        }
    }
}
