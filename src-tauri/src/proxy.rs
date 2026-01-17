pub(crate) mod config;
pub(crate) mod dashboard;
mod http_client;
pub(crate) mod logs;
pub(crate) mod request_detail;
pub(crate) mod service;
mod gemini;
mod gemini_compat;
mod http;
mod log;
mod model;
mod anthropic_compat;
mod compat_content;
mod compat_reason;
mod openai_compat;
mod request_body;
mod response;
mod redact;
mod server;
mod server_helpers;
mod sse;
mod sqlite;
pub(crate) mod token_rate;
mod upstream;
mod usage;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

// 上游“无数据响应”超时：同时用于等待响应头（TTFB）与流式 body 的空闲超时。
// - 生产：180s（用户要求写死）
// - 测试：缩短，避免用例卡 180s
#[cfg(test)]
pub(crate) const UPSTREAM_NO_DATA_TIMEOUT: Duration = Duration::from_millis(50);
#[cfg(not(test))]
pub(crate) const UPSTREAM_NO_DATA_TIMEOUT: Duration = Duration::from_secs(180);

struct ProxyState {
    config: config::ProxyConfig,
    http_clients: http_client::ProxyHttpClients,
    log: Arc<log::LogWriter>,
    cursors: HashMap<String, Vec<AtomicUsize>>,
    request_detail: Arc<request_detail::RequestDetailCapture>,
    token_rate: Arc<token_rate::TokenRateTracker>,
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
        match (
            self.original_model.as_deref(),
            self.mapped_model.as_deref(),
        ) {
            (Some(original), Some(mapped)) if original != mapped => Some(original),
            _ => None,
        }
    }
}
