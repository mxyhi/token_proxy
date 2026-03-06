use axum::http::HeaderMap;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::request_body::ReplayableBody;

const BODY_TOO_LARGE_MESSAGE: &str = "[body omitted: too large]";
const REQUEST_DETAIL_CAPTURE_WINDOW: Duration = Duration::from_secs(30);
const DISARMED_AT_MS: u64 = 0;

#[derive(Clone, Default)]
pub struct RequestDetailSnapshot {
    pub request_headers: Option<String>,
    pub request_body: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDetailCaptureState {
    pub enabled: bool,
    pub expires_at_ms: Option<u64>,
}

impl RequestDetailCaptureState {
    pub fn idle() -> Self {
        Self {
            enabled: false,
            expires_at_ms: None,
        }
    }

    fn active(expires_at_ms: u64) -> Self {
        Self {
            enabled: true,
            expires_at_ms: Some(expires_at_ms),
        }
    }
}

pub struct RequestDetailCapture {
    expires_at_ms: AtomicU64,
    window_ms: u64,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    on_change: Option<Arc<dyn Fn(RequestDetailCaptureState) + Send + Sync>>,
}

impl RequestDetailCapture {
    pub fn new(
        on_change: Option<Arc<dyn Fn(RequestDetailCaptureState) + Send + Sync>>,
    ) -> Self {
        Self {
            expires_at_ms: AtomicU64::new(DISARMED_AT_MS),
            window_ms: duration_to_millis(REQUEST_DETAIL_CAPTURE_WINDOW),
            now_ms: Arc::new(current_time_millis),
            on_change,
        }
    }

    #[cfg(test)]
    fn new_with_clock(
        window: Duration,
        on_change: Option<Arc<dyn Fn(RequestDetailCaptureState) + Send + Sync>>,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            expires_at_ms: AtomicU64::new(DISARMED_AT_MS),
            window_ms: duration_to_millis(window),
            now_ms,
            on_change,
        }
    }

    pub fn arm(&self) -> RequestDetailCaptureState {
        let expires_at_ms = (self.now_ms)().saturating_add(self.window_ms);
        let state = RequestDetailCaptureState::active(expires_at_ms);
        self.expires_at_ms.store(expires_at_ms, Ordering::SeqCst);
        self.notify(state);
        state
    }

    pub fn disarm(&self) -> RequestDetailCaptureState {
        let state = RequestDetailCaptureState::idle();
        self.expires_at_ms.store(DISARMED_AT_MS, Ordering::SeqCst);
        self.notify(state);
        state
    }

    pub fn is_armed(&self) -> bool {
        self.snapshot().enabled
    }

    pub fn should_capture(&self) -> bool {
        self.snapshot().enabled
    }

    pub fn snapshot(&self) -> RequestDetailCaptureState {
        loop {
            let expires_at_ms = self.expires_at_ms.load(Ordering::SeqCst);
            if expires_at_ms == DISARMED_AT_MS {
                return RequestDetailCaptureState::idle();
            }

            if (self.now_ms)() <= expires_at_ms {
                return RequestDetailCaptureState::active(expires_at_ms);
            }

            // 窗口过期后仅第一个观察者负责清空并广播关闭，避免并发重复通知。
            if self
                .expires_at_ms
                .compare_exchange(
                    expires_at_ms,
                    DISARMED_AT_MS,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                let state = RequestDetailCaptureState::idle();
                self.notify(state);
                return state;
            }
        }
    }

    fn notify(&self, state: RequestDetailCaptureState) {
        let Some(callback) = self.on_change.as_ref() else {
            return;
        };
        callback(state);
    }
}

impl Default for RequestDetailCapture {
    fn default() -> Self {
        Self::new(None)
    }
}

fn current_time_millis() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration_to_millis(elapsed)
}

fn duration_to_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;
    use std::time::Duration;

    fn create_capture(
        now_ms: Arc<AtomicU64>,
        changes: Arc<Mutex<Vec<RequestDetailCaptureState>>>,
    ) -> RequestDetailCapture {
        let change_sink = changes.clone();
        RequestDetailCapture::new_with_clock(
            Duration::from_secs(30),
            Some(Arc::new(move |state| {
                change_sink
                    .lock()
                    .expect("lock change sink")
                    .push(state);
            })),
            Arc::new(move || now_ms.load(Ordering::SeqCst)),
        )
    }

    #[test]
    fn capture_stays_enabled_within_window() {
        let now_ms = Arc::new(AtomicU64::new(5_000));
        let changes = Arc::new(Mutex::new(Vec::new()));
        let capture = create_capture(now_ms.clone(), changes);

        let state = capture.arm();
        assert_eq!(
            state,
            RequestDetailCaptureState {
                enabled: true,
                expires_at_ms: Some(35_000),
            }
        );
        assert!(capture.should_capture());

        now_ms.store(34_999, Ordering::SeqCst);
        assert!(capture.should_capture());
        assert_eq!(capture.snapshot(), state);
    }

    #[test]
    fn capture_expires_after_window_and_notifies_idle_once() {
        let now_ms = Arc::new(AtomicU64::new(1_000));
        let changes = Arc::new(Mutex::new(Vec::new()));
        let capture = create_capture(now_ms.clone(), changes.clone());

        let active = capture.arm();
        now_ms.store(31_001, Ordering::SeqCst);

        assert!(!capture.should_capture());
        assert_eq!(capture.snapshot(), RequestDetailCaptureState::idle());

        let observed = changes.lock().expect("lock changes").clone();
        assert_eq!(
            observed,
            vec![active, RequestDetailCaptureState::idle()]
        );
    }

    #[test]
    fn rearming_extends_capture_deadline() {
        let now_ms = Arc::new(AtomicU64::new(10_000));
        let changes = Arc::new(Mutex::new(Vec::new()));
        let capture = create_capture(now_ms.clone(), changes);

        let first = capture.arm();
        now_ms.store(20_000, Ordering::SeqCst);
        let second = capture.arm();

        assert_eq!(first.expires_at_ms, Some(40_000));
        assert_eq!(second.expires_at_ms, Some(50_000));

        now_ms.store(45_000, Ordering::SeqCst);
        assert!(capture.should_capture());
    }
}
