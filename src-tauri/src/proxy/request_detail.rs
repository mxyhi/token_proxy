use axum::http::HeaderMap;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

use super::request_body::ReplayableBody;

const BODY_TOO_LARGE_MESSAGE: &str = "[body omitted: too large]";

#[derive(Clone, Default)]
pub(crate) struct RequestDetailSnapshot {
    pub(crate) request_headers: Option<String>,
    pub(crate) request_body: Option<String>,
}

#[derive(Default)]
pub(crate) struct RequestDetailCapture {
    armed: AtomicBool,
}

impl RequestDetailCapture {
    pub(crate) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    pub(crate) fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    pub(crate) fn take(&self) -> bool {
        self.armed.swap(false, Ordering::SeqCst)
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }
}

pub(crate) fn serialize_request_headers(headers: &HeaderMap) -> Option<String> {
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
