mod account_selector;
mod anthropic_compat;
mod claude_reasoning;
mod codex_compat;
mod codex_models_manifest;
pub(crate) use token_proxy_config as config;
pub(crate) use token_proxy_protocol::{codex_tool_types, compat_content, compat_reason};
mod cooldown_scope;
mod gemini;
mod gemini_compat;
mod http;
mod http_client;
mod inbound;
mod kiro;
mod log;
mod model;
pub(crate) mod model_discovery;
mod openai;
mod openai_compat;
mod path_guard;
mod redact;
mod request_body;
pub mod request_detail;
pub(crate) use token_proxy_protocol::request_token_estimate;
mod response;
mod server;
mod server_helpers;
pub mod service;
pub(crate) mod sqlite;
pub(crate) use token_proxy_protocol::{sse, token_estimator};
pub mod token_rate;
mod upstream;
mod upstream_selector;
pub(crate) use token_proxy_storage::usage;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};
use token_proxy_account_codex::CodexAccountStore;
use token_proxy_account_kiro::KiroAccountStore;
use token_proxy_account_xai::XaiAccountStore;

struct ProxyState {
    config: token_proxy_config::ProxyConfig,
    http_clients: http_client::ProxyHttpClients,
    log: Arc<log::LogWriter>,
    cursors: HashMap<String, Vec<AtomicUsize>>,
    upstream_selector: upstream_selector::UpstreamSelectorRuntime,
    account_selector: account_selector::AccountSelectorRuntime,
    request_detail: Arc<request_detail::RequestDetailCapture>,
    token_rate: Arc<token_rate::TokenRateTracker>,
    model_discovery: Arc<model_discovery::UpstreamModelDiscoveryCache>,
    kiro_accounts: Arc<KiroAccountStore>,
    codex_accounts: Arc<CodexAccountStore>,
    xai_accounts: Arc<XaiAccountStore>,
}

struct RequestMeta {
    client_ip: Option<String>,
    stream: bool,
    original_model: Option<String>,
    mapped_model: Option<String>,
    reasoning_effort: Option<String>,
    response_format: Option<String>,
    estimated_input_tokens: Option<u64>,
    billing: log::ClientRequestBilling,
}

impl RequestMeta {
    fn model_override(&self) -> Option<&str> {
        match (self.original_model.as_deref(), self.mapped_model.as_deref()) {
            (Some(original), Some(mapped)) if original != mapped => Some(original),
            _ => None,
        }
    }
}
