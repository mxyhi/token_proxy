use axum::http::HeaderMap;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::request_body::ReplayableBody;

const BODY_TOO_LARGE_MESSAGE: &str = "[body omitted: too large]";

#[derive(Clone, Default)]
pub struct RequestDetailSnapshot {
    pub request_headers: Option<String>,
    pub request_body: Option<String>,
}

pub struct RequestDetailCapture {
    armed: AtomicBool,
    on_change: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}

impl RequestDetailCapture {
    pub fn new(on_change: Option<Arc<dyn Fn(bool) + Send + Sync>>) -> Self {
        Self {
            armed: AtomicBool::new(false),
            on_change,
        }
    }

    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
        self.notify(true);
    }

    pub fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.notify(false);
    }

    pub fn take(&self) -> bool {
        let was_armed = self.armed.swap(false, Ordering::SeqCst);
        if was_armed {
            self.notify(false);
        }
        was_armed
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    fn notify(&self, enabled: bool) {
        let Some(callback) = self.on_change.as_ref() else {
            return;
        };
        callback(enabled);
    }
}

impl Default for RequestDetailCapture {
    fn default() -> Self {
        Self::new(None)
    }
}

pub fn serialize_request_headers(headers: &HeaderMap) -> Option<String> {
    let items: Vec<HeaderEntry> = headers
        .iter()
        .map(|(name, value)| HeaderEntry {
            name: name.to_string(),
            value: value.to_str().unwrap_or("<binary>").to_string(),
        })
        .collect();
    serde_json::to_string(&items).ok()
}

pub(crate) async fn capture_request_detail(
    headers: &HeaderMap,
    body: &ReplayableBody,
    max_body_bytes: usize,
) -> RequestDetailSnapshot {
    let request_headers = serialize_request_headers(headers);
    let request_body = match body.read_bytes_if_small(max_body_bytes).await {
        Ok(Some(bytes)) => Some(String::from_utf8_lossy(&bytes).to_string()),
        Ok(None) => Some(BODY_TOO_LARGE_MESSAGE.to_string()),
        Err(err) => Some(format!("Failed to read request body: {err}")),
    };

    RequestDetailSnapshot {
        request_headers,
        request_body,
    }
}

#[derive(Serialize)]
struct HeaderEntry {
    name: String,
    value: String,
}
