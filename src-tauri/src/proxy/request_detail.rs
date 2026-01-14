use axum::http::HeaderMap;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

use super::request_body::ReplayableBody;

const BODY_TOO_LARGE_MESSAGE: &str = "[body omitted: too large]";

#[derive(Clone, Default)]
pub(crate) struct RequestDetailSnapshot {
    pub(crate) request_headers: Option<String>,
    pub(crate) request_body: Option<String>,
}

const REQUEST_DETAIL_CAPTURE_EVENT: &str = "request-detail-capture-changed";

pub(crate) struct RequestDetailCapture {
    armed: AtomicBool,
    app: Option<AppHandle>,
}

impl RequestDetailCapture {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self {
            armed: AtomicBool::new(false),
            app: Some(app),
        }
    }

    pub(crate) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
        self.emit(true);
    }

    pub(crate) fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.emit(false);
    }

    pub(crate) fn take(&self) -> bool {
        let was_armed = self.armed.swap(false, Ordering::SeqCst);
        if was_armed {
            self.emit(false);
        }
        was_armed
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    fn emit(&self, enabled: bool) {
        let Some(app) = self.app.as_ref() else {
            return;
        };
        let _ = app.emit(
            REQUEST_DETAIL_CAPTURE_EVENT,
            RequestDetailCaptureEvent { enabled },
        );
    }
}

impl Default for RequestDetailCapture {
    fn default() -> Self {
        Self {
            armed: AtomicBool::new(false),
            app: None,
        }
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

#[derive(Clone, Serialize)]
struct RequestDetailCaptureEvent {
    enabled: bool,
}
