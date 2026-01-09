use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tiktoken_rs::{cl100k_base, o200k_base, CoreBPE};
use tokio::{
    sync::{watch, Mutex, RwLock},
    time::{interval, MissedTickBehavior},
};

const RATE_WINDOW: Duration = Duration::from_secs(1);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
// 超过该时长未记录 token 的请求窗口视为过期，避免 HashMap 无界增长。
const REQUEST_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub(crate) struct TokenRateTracker {
    inner: Arc<TrackerInner>,
    activity_tx: watch::Sender<u64>,
}

struct TrackerInner {
    next_id: AtomicU64,
    active: AtomicUsize,
    enabled: AtomicBool,
    generation: AtomicU64,
    cleanup_started: AtomicBool,
    last_cleanup: Mutex<Instant>,
    requests: RwLock<HashMap<u64, Arc<Mutex<RequestWindow>>>>,
}

struct RequestWindow {
    events: VecDeque<TokenEvent>,
    last_seen: Instant,
}

struct TokenEvent {
    ts: Instant,
    input: u64,
    output: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TokenRateSnapshot {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) total: u64,
    pub(crate) connections: u64,
}

pub(crate) struct RequestTokenTracker {
    id: Option<u64>,
    window: Option<Arc<Mutex<RequestWindow>>>,
    tracker: TokenRateTracker,
    model: Option<String>,
    generation: Option<u64>,
}

impl TokenRateTracker {
    pub(crate) fn new() -> Arc<Self> {
        let (activity_tx, _activity_rx) = watch::channel(0u64);
        let tracker = Arc::new(Self {
            inner: Arc::new(TrackerInner {
                next_id: AtomicU64::new(1),
                active: AtomicUsize::new(0),
                enabled: AtomicBool::new(true),
                generation: AtomicU64::new(1),
                cleanup_started: AtomicBool::new(false),
                last_cleanup: Mutex::new(Instant::now()),
                requests: RwLock::new(HashMap::new()),
            }),
            activity_tx,
        });
        tracker.try_start_cleanup();
        tracker
    }

    pub(crate) fn subscribe_activity(&self) -> watch::Receiver<u64> {
        self.activity_tx.subscribe()
    }

    pub(crate) fn notify_activity(&self) {
        let next = self.activity_tx.borrow().wrapping_add(1);
        let _ = self.activity_tx.send(next);
    }

    pub(crate) async fn set_enabled(&self, enabled: bool) {
        self.try_start_cleanup();
        tracing::debug!(enabled, "token_rate set_enabled start");
        let previous = self.inner.enabled.swap(enabled, Ordering::SeqCst);
        if previous == enabled {
            tracing::debug!(enabled, "token_rate set_enabled noop");
            return;
        }
        // 每次开关切换递增 generation，确保旧请求不会在重新开启后继续计数。
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        if !enabled {
            tracing::debug!("token_rate set_enabled clearing requests start");
            let mut guard = self.inner.requests.write().await;
            guard.clear();
            self.inner.active.store(0, Ordering::SeqCst);
            tracing::debug!("token_rate set_enabled clearing requests done");
        }
        tracing::debug!(enabled, "token_rate set_enabled done");
    }

    pub(crate) async fn register(
        &self,
        model: Option<String>,
        input_tokens: Option<u64>,
    ) -> RequestTokenTracker {
        self.try_start_cleanup();
        self.maybe_cleanup(Instant::now()).await;
        let enabled = self.inner.enabled.load(Ordering::SeqCst);
        let generation = self.inner.generation.load(Ordering::SeqCst);
        let (mut id, mut window) = if enabled {
            let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
            let window = Arc::new(Mutex::new(RequestWindow::new()));
            let mut guard = self.inner.requests.write().await;
            guard.insert(id, window.clone());
            self.inner.active.fetch_add(1, Ordering::SeqCst);
            (Some(id), Some(window))
        } else {
            (None, None)
        };
        let mut effective_generation = if enabled { Some(generation) } else { None };
        if let Some(current_id) = id {
            let still_enabled = self.inner.enabled.load(Ordering::SeqCst);
            let current_generation = self.inner.generation.load(Ordering::SeqCst);
            if !still_enabled || current_generation != generation {
                // 开关状态变更后不再追踪该请求，避免重新开启时继续计数。
                self.unregister(current_id).await;
                id = None;
                window = None;
                effective_generation = None;
            }
        }

        let tracker = RequestTokenTracker {
            id,
            window,
            tracker: self.clone(),
            model,
            generation: effective_generation,
        };
        if let Some(tokens) = input_tokens {
            tracker.add_input_tokens(tokens).await;
        }
        if enabled {
            self.notify_activity();
        }
        tracker
    }

    pub(crate) async fn snapshot(&self) -> TokenRateSnapshot {
        self.try_start_cleanup();
        if !self.inner.enabled.load(Ordering::SeqCst) {
            return TokenRateSnapshot {
                input: 0,
                output: 0,
                total: 0,
                connections: 0,
            };
        }
        self.maybe_cleanup(Instant::now()).await;
        let now = Instant::now();
        let windows: Vec<Arc<Mutex<RequestWindow>>> = self
            .inner
            .requests
            .read()
            .await
            .values()
            .cloned()
            .collect();
        let mut input = 0u64;
        let mut output = 0u64;
        for window in windows {
            let mut guard = window.lock().await;
            guard.prune(now);
            let (i, o) = guard.sum();
            input = input.saturating_add(i);
            output = output.saturating_add(o);
        }
        TokenRateSnapshot {
            input,
            output,
            total: input.saturating_add(output),
            connections: self.inner.active.load(Ordering::SeqCst) as u64,
        }
    }

    pub(crate) fn has_active_requests(&self) -> bool {
        if !self.inner.enabled.load(Ordering::SeqCst) {
            return false;
        }
        self.inner.active.load(Ordering::SeqCst) > 0
    }

    async fn record(&self, window: &Arc<Mutex<RequestWindow>>, input: u64, output: u64) {
        if input == 0 && output == 0 {
            return;
        }
        let now = Instant::now();
        {
            let mut guard = window.lock().await;
            guard.push(TokenEvent {
                ts: now,
                input,
                output,
            });
        }
        self.maybe_cleanup(now).await;
    }

    async fn unregister(&self, id: u64) {
        let removed = self
            .inner
            .requests
            .write()
            .await
            .remove(&id)
            .is_some();
        if removed {
            self.inner.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    // 在有 Tokio runtime 时启动清理任务，避免无 reactor 场景崩溃。
    fn try_start_cleanup(&self) {
        if self.inner.cleanup_started.load(Ordering::SeqCst) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .inner
            .cleanup_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let weak_inner = Arc::downgrade(&self.inner);
        handle.spawn(async move {
            let mut ticker = interval(CLEANUP_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let Some(inner) = weak_inner.upgrade() else {
                    break;
                };
                if !inner.enabled.load(Ordering::SeqCst) {
                    continue;
                }
                cleanup_expired_inner(&inner, Instant::now()).await;
            }
        });
    }

    // 惰性清理：在流量发生时按间隔触发，减少单独后台依赖。
    async fn maybe_cleanup(&self, now: Instant) {
        if !self.inner.enabled.load(Ordering::SeqCst) {
            return;
        }
        if !self.should_cleanup(now).await {
            return;
        }
        self.cleanup_expired(now).await;
    }

    async fn should_cleanup(&self, now: Instant) -> bool {
        let mut guard = self.inner.last_cleanup.lock().await;
        if now.duration_since(*guard) < CLEANUP_INTERVAL {
            return false;
        }
        *guard = now;
        true
    }

    async fn cleanup_expired(&self, now: Instant) {
        cleanup_expired_inner(&self.inner, now).await;
    }
}

async fn cleanup_expired_inner(inner: &TrackerInner, now: Instant) {
    let windows: Vec<(u64, Arc<Mutex<RequestWindow>>)> = inner
        .requests
        .read()
        .await
        .iter()
        .map(|(id, window)| (*id, window.clone()))
        .collect();
    if windows.is_empty() {
        return;
    }
    let mut expired = Vec::new();
    for (id, window) in windows {
        let guard = window.lock().await;
        if guard.is_expired(now) {
            expired.push(id);
        }
    }
    if expired.is_empty() {
        return;
    }
    let mut guard = inner.requests.write().await;
    let mut removed = 0usize;
    for id in expired {
        if guard.remove(&id).is_some() {
            removed += 1;
        }
    }
    if removed > 0 {
        inner.active.fetch_sub(removed, Ordering::SeqCst);
    }
}

impl RequestWindow {
    fn new() -> Self {
        Self {
            events: VecDeque::new(),
            last_seen: Instant::now(),
        }
    }

    fn push(&mut self, event: TokenEvent) {
        let now = event.ts;
        self.events.push_back(event);
        self.last_seen = now;
        self.prune(now);
    }

    fn prune(&mut self, now: Instant) {
        while let Some(front) = self.events.front() {
            if now.duration_since(front.ts) <= RATE_WINDOW {
                break;
            }
            self.events.pop_front();
        }
    }

    fn sum(&self) -> (u64, u64) {
        let mut input = 0u64;
        let mut output = 0u64;
        for event in &self.events {
            input = input.saturating_add(event.input);
            output = output.saturating_add(event.output);
        }
        (input, output)
    }

    fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_seen) > REQUEST_TTL
    }
}

impl RequestTokenTracker {
    pub(crate) async fn add_input_tokens(&self, tokens: u64) {
        if !self.can_record() {
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        self.tracker.record(window, tokens, 0).await;
    }

    pub(crate) async fn add_output_text(&self, text: &str) {
        if !self.can_record() {
            return;
        }
        let tokens = estimate_text_tokens(self.model.as_deref(), text);
        let Some(window) = self.window.as_ref() else {
            return;
        };
        self.tracker.record(window, 0, tokens).await;
    }

    fn can_record(&self) -> bool {
        let Some(generation) = self.generation else {
            return false;
        };
        if !self.tracker.inner.enabled.load(Ordering::SeqCst) {
            return false;
        }
        // generation 不一致说明开关已经切换，旧请求不再计数。
        self.tracker.inner.generation.load(Ordering::SeqCst) == generation
    }
}

impl Drop for RequestTokenTracker {
    fn drop(&mut self) {
        let Some(id) = self.id else {
            return;
        };
        if let Ok(mut guard) = self.tracker.inner.requests.try_write() {
            if guard.remove(&id).is_some() {
                self.tracker.inner.active.fetch_sub(1, Ordering::SeqCst);
            }
            return;
        }
        // 避免在 Drop 中阻塞异步运行时，使用最佳努力异步清理。
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let tracker = self.tracker.clone();
            handle.spawn(async move {
                tracker.unregister(id).await;
            });
        }
    }
}

pub(crate) fn estimate_text_tokens(model: Option<&str>, text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let bpe = bpe_for_model(model);
    bpe.encode_with_special_tokens(text).len() as u64
}

fn bpe_for_model(model: Option<&str>) -> &'static CoreBPE {
    if matches_o200k(model) {
        static O200K: OnceLock<CoreBPE> = OnceLock::new();
        return O200K.get_or_init(|| {
            o200k_base().unwrap_or_else(|_| cl100k_base().expect("cl100k_base"))
        });
    }

    static CL100K: OnceLock<CoreBPE> = OnceLock::new();
    CL100K.get_or_init(|| cl100k_base().expect("cl100k_base"))
}

fn matches_o200k(model: Option<&str>) -> bool {
    let Some(model) = model else {
        return false;
    };
    let model = model.trim();
    if model.is_empty() {
        return false;
    }
    let model = model.to_ascii_lowercase();
    model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("gpt-4o")
        || model.starts_with("gpt-4.1")
}
