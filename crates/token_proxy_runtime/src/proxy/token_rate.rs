use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

const INPUT_DISPLAY_WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct TokenRateTracker {
    inner: Arc<Mutex<TrackerState>>,
    activity_tx: watch::Sender<u64>,
}

struct TrackerState {
    enabled: bool,
    generation: u64,
    connections: u64,
    inputs: VecDeque<InputEvent>,
}

struct InputEvent {
    expires_at: Instant,
    tokens: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct TokenRateSnapshot {
    pub input: u64,
    /// 活跃代理请求数，包含等待响应头及正在传输响应的请求。
    pub connections: u64,
    /// 下一次输入 token 展示到期；无输入时只需等待请求生命周期事件。
    pub refresh_after: Option<Duration>,
}

pub struct RequestTokenTracker {
    tracker: Option<TokenRateTracker>,
    generation: u64,
}

impl TokenRateTracker {
    pub fn new() -> Arc<Self> {
        let (activity_tx, _) = watch::channel(0u64);
        Arc::new(Self {
            inner: Arc::new(Mutex::new(TrackerState {
                enabled: true,
                generation: 0,
                connections: 0,
                inputs: VecDeque::new(),
            })),
            activity_tx,
        })
    }

    pub fn subscribe_activity(&self) -> watch::Receiver<u64> {
        self.activity_tx.subscribe()
    }

    pub fn notify_activity(&self) {
        self.activity_tx
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub async fn set_enabled(&self, enabled: bool) {
        {
            let mut state = self.inner.lock().expect("tray activity lock poisoned");
            if state.enabled == enabled {
                return;
            }
            state.enabled = enabled;
            // 开关切换后，旧请求的 Drop 不能扣减新一代请求的连接数。
            state.generation = state.generation.wrapping_add(1);
            state.connections = 0;
            state.inputs.clear();
        }
        tracing::debug!(enabled, "tray activity tracking changed");
        self.notify_activity();
    }

    pub async fn register(&self, input_tokens: Option<u64>) -> RequestTokenTracker {
        let generation = {
            // 锁内只有计数及输入事件操作，不做 IO、分词或 await。
            let mut state = self.inner.lock().expect("tray activity lock poisoned");
            if !state.enabled {
                return RequestTokenTracker::disabled();
            }
            let now = Instant::now();
            state.prune_inputs(now);
            if let Some(tokens) = input_tokens.filter(|tokens| *tokens > 0) {
                state.inputs.push_back(InputEvent {
                    expires_at: now + INPUT_DISPLAY_WINDOW,
                    tokens,
                });
            }
            state.connections += 1;
            state.generation
        };
        self.notify_activity();
        RequestTokenTracker {
            tracker: Some(self.clone()),
            generation,
        }
    }

    pub async fn snapshot(&self) -> TokenRateSnapshot {
        let mut state = self.inner.lock().expect("tray activity lock poisoned");
        let now = Instant::now();
        state.prune_inputs(now);
        TokenRateSnapshot {
            input: state
                .inputs
                .iter()
                .fold(0u64, |sum, event| sum.saturating_add(event.tokens)),
            connections: state.connections,
            refresh_after: state
                .inputs
                .front()
                .map(|event| event.expires_at.saturating_duration_since(now)),
        }
    }
}

impl TrackerState {
    fn prune_inputs(&mut self, now: Instant) {
        while self
            .inputs
            .front()
            .is_some_and(|event| event.expires_at <= now)
        {
            self.inputs.pop_front();
        }
    }
}

impl RequestTokenTracker {
    pub(crate) fn disabled() -> Self {
        Self {
            tracker: None,
            generation: 0,
        }
    }
}

impl Drop for RequestTokenTracker {
    fn drop(&mut self) {
        let Some(tracker) = self.tracker.as_ref() else {
            return;
        };
        {
            let mut state = tracker.inner.lock().expect("tray activity lock poisoned");
            if self.generation != state.generation {
                return;
            }
            // 生命周期守卫同步释放；长时间无输出不会被 TTL 误判为连接结束。
            state.connections -= 1;
        }
        tracker.notify_activity();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connections_follow_request_lifetime_and_notify() {
        let rate = TokenRateTracker::new();
        let mut activity = rate.subscribe_activity();
        let first = rate.register(None).await;
        activity.changed().await.unwrap();
        let second = rate.register(None).await;
        let snapshot = rate.snapshot().await;
        assert_eq!(snapshot.connections, 2);
        assert_eq!(snapshot.input, 0);
        assert_eq!(snapshot.refresh_after, None);
        activity.borrow_and_update();
        drop(first);
        assert!(activity.has_changed().unwrap());
        assert_eq!(rate.snapshot().await.connections, 1);
        drop(second);
        assert_eq!(rate.snapshot().await.connections, 0);
    }

    #[tokio::test]
    async fn upload_tokens_expire_without_ending_active_connections() {
        let rate = TokenRateTracker::new();
        let first = rate.register(Some(42)).await;
        let second = rate.register(Some(8)).await;
        let snapshot = rate.snapshot().await;
        assert_eq!(snapshot.input, 50);
        assert_eq!(snapshot.connections, 2);
        assert!(snapshot
            .refresh_after
            .is_some_and(|delay| delay <= INPUT_DISPLAY_WINDOW));
        // 短请求的上传脉冲保留到展示到期，避免请求很快完成而完全不可见。
        drop(first);
        assert_eq!(rate.snapshot().await.input, 50);
        tokio::time::sleep(INPUT_DISPLAY_WINDOW + Duration::from_millis(20)).await;
        let snapshot = rate.snapshot().await;
        assert_eq!(snapshot.input, 0);
        assert_eq!(snapshot.connections, 1);
        assert_eq!(snapshot.refresh_after, None);
        drop(second);
    }

    #[tokio::test]
    async fn old_requests_cannot_decrement_connections_after_toggle() {
        let rate = TokenRateTracker::new();
        let old = rate.register(Some(42)).await;
        rate.set_enabled(false).await;
        let disabled = rate.register(Some(100)).await;
        let snapshot = rate.snapshot().await;
        assert_eq!(snapshot.input, 0);
        assert_eq!(snapshot.connections, 0);
        assert_eq!(snapshot.refresh_after, None);
        rate.set_enabled(true).await;
        let current = rate.register(Some(8)).await;
        drop(old);
        drop(disabled);
        assert_eq!(rate.snapshot().await.connections, 1);
        assert_eq!(rate.snapshot().await.input, 8);
        drop(current);
        assert_eq!(rate.snapshot().await.connections, 0);
    }

    #[tokio::test]
    async fn cancelled_task_releases_connection() {
        let rate = TokenRateTracker::new();
        let task_rate = rate.clone();
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = task_rate.register(None).await;
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        assert_eq!(rate.snapshot().await.connections, 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(rate.snapshot().await.connections, 0);
    }

    #[test]
    fn guard_can_drop_after_runtime_shutdown() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let rate = TokenRateTracker::new();
        let guard = runtime.block_on(rate.register(None));
        drop(runtime);
        drop(guard);
        assert_eq!(rate.inner.lock().unwrap().connections, 0);
    }
}
