use serde::Serialize;
use sqlx::SqlitePool;
use std::future::IntoFuture;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::config::ProxyConfig;
use super::log::LogWriter;
use super::request_detail::RequestDetailCapture;
use super::sqlite;
use super::server;
use super::ProxyState;
use crate::logging::LoggingState;

/// 默认优雅停机等待时间；超时后会强制 abort server task。
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

type ProxyStateHandle = Arc<RwLock<Arc<ProxyState>>>;
type ProxyRouter = axum::Router;

#[derive(Clone)]
pub(crate) struct ProxyServiceHandle {
    inner: Arc<ProxyService>,
}

impl ProxyServiceHandle {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(ProxyService::new()),
        }
    }

    pub(crate) async fn status(&self) -> ProxyServiceStatus {
        self.inner.status().await
    }

    pub(crate) async fn start(&self, app: AppHandle) -> Result<ProxyServiceStatus, String> {
        self.inner.start(app).await
    }

    pub(crate) async fn stop(&self) -> Result<ProxyServiceStatus, String> {
        self.inner.stop().await
    }

    pub(crate) async fn restart(&self, app: AppHandle) -> Result<ProxyServiceStatus, String> {
        self.inner.restart(app).await
    }

    pub(crate) async fn reload(&self, app: AppHandle) -> Result<ProxyServiceStatus, String> {
        self.inner.reload(app).await
    }
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProxyServiceState {
    Running,
    Stopped,
}

#[derive(Clone, Serialize)]
pub(crate) struct ProxyServiceStatus {
    pub(crate) state: ProxyServiceState,
    pub(crate) addr: Option<String>,
    pub(crate) last_error: Option<String>,
}

impl ProxyServiceStatus {
    fn stopped(last_error: Option<String>) -> Self {
        Self {
            state: ProxyServiceState::Stopped,
            addr: None,
            last_error,
        }
    }

    fn running(addr: String, last_error: Option<String>) -> Self {
        Self {
            state: ProxyServiceState::Running,
            addr: Some(addr),
            last_error,
        }
    }
}

struct ProxyService {
    inner: Mutex<ProxyServiceInner>,
}

impl ProxyService {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ProxyServiceInner::new()),
        }
    }

    async fn status(&self) -> ProxyServiceStatus {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.status()
    }

    async fn start(&self, app: AppHandle) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.start(app).await?;
        Ok(inner.status())
    }

    async fn stop(&self) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.stop().await?;
        Ok(inner.status())
    }

    async fn restart(&self, app: AppHandle) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.restart(app).await?;
        Ok(inner.status())
    }

    async fn reload(&self, app: AppHandle) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.reload(app).await?;
        Ok(inner.status())
    }
}

struct ProxyServiceInner {
    running: Option<RunningProxy>,
    sqlite_pool: Option<SqlitePool>,
    last_error: Option<String>,
}

impl ProxyServiceInner {
    fn new() -> Self {
        Self {
            running: None,
            sqlite_pool: None,
            last_error: None,
        }
    }

    fn status(&self) -> ProxyServiceStatus {
        match &self.running {
            Some(running) => ProxyServiceStatus::running(running.addr.clone(), self.last_error.clone()),
            None => ProxyServiceStatus::stopped(self.last_error.clone()),
        }
    }

    async fn refresh_if_finished(&mut self) {
        let Some(running) = self.running.as_mut() else {
            return;
        };
        let Some(task) = running.task.as_ref() else {
            return;
        };
        if !task.is_finished() {
            return;
        }
        let running = self.running.take().expect("running must exist");
        self.finish_task(running).await;
    }

    async fn start(&mut self, app: AppHandle) -> Result<(), String> {
        if self.running.is_some() {
            return Ok(());
        }
        if self.sqlite_pool.is_none() {
            self.sqlite_pool = sqlite::open_write_pool(&app).await.ok();
        }
        let sqlite_pool = self.sqlite_pool.clone();
        let loaded_config = ProxyConfig::load(&app).await?;
        let addr = loaded_config.addr();

        let (state_handle, router) = build_router_state(&app, loaded_config, sqlite_pool).await?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|err| format!("Failed to bind {addr}: {err}"))?;
        tracing::info!(addr = %addr, "proxy listening");

        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .into_future()
                .await
                .map_err(|err| format!("Proxy server failed: {err}"))
        });

        self.running = Some(RunningProxy {
            addr,
            state_handle,
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        });
        self.last_error = None;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), String> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        self.finish_task(running).await;
        Ok(())
    }

    async fn restart(&mut self, app: AppHandle) -> Result<(), String> {
        self.stop().await?;
        self.start(app).await
    }

    async fn reload(&mut self, app: AppHandle) -> Result<(), String> {
        tracing::debug!("proxy reload start");
        let start = Instant::now();
        if self.running.is_none() {
            tracing::debug!("proxy reload: not running, start instead");
            return self.start(app).await;
        }
        let loaded_config = ProxyConfig::load(&app).await?;
        let addr = loaded_config.addr();
        let current_addr = self
            .running
            .as_ref()
            .map(|running| running.addr.as_str())
            .unwrap_or_default()
            .to_string();

        tracing::debug!(addr = %addr, current_addr = %current_addr, "proxy reload config loaded");
        if addr != current_addr {
            // host/port 变更无法热更新监听地址；退化为安全重启。
            tracing::info!(
                addr = %addr,
                current_addr = %current_addr,
                "proxy reload detected addr change, restarting"
            );
            return self.restart(app).await;
        }
        let current_max_request_body_bytes = if let Some(running) = self.running.as_ref() {
            let guard = running.state_handle.read().await;
            guard.config.max_request_body_bytes
        } else {
            loaded_config.max_request_body_bytes
        };
        if loaded_config.max_request_body_bytes != current_max_request_body_bytes {
            tracing::info!(
                new_max_request_body_bytes = loaded_config.max_request_body_bytes,
                current_max_request_body_bytes = current_max_request_body_bytes,
                "proxy reload detected body limit change, restarting"
            );
            return self.restart(app).await;
        }

        let sqlite_pool = self.sqlite_pool.clone();
        let new_state = build_proxy_state(&app, loaded_config, sqlite_pool).await?;
        let Some(running) = self.running.as_ref() else {
            tracing::debug!("proxy reload: running cleared before swap");
            return Ok(());
        };
        {
            let mut guard = running.state_handle.write().await;
            *guard = new_state;
        }
        tracing::debug!(elapsed_ms = start.elapsed().as_millis(), "proxy reload applied");
        Ok(())
    }

    async fn finish_task(&mut self, mut running: RunningProxy) {
        if let Some(tx) = running.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = running.task.take() {
            self.await_stop(task, running.shutdown_timeout).await;
        }
    }

    async fn await_stop(&mut self, task: JoinHandle<Result<(), String>>, timeout_duration: Duration) {
        let mut task = task;
        match timeout(timeout_duration, &mut task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(message))) => {
                self.last_error = Some(message);
            }
            Ok(Err(err)) => {
                self.last_error = Some(format!("Proxy task join failed: {err}"));
            }
            Err(_) => {
                task.abort();
                self.last_error = Some("Proxy stop timed out; aborted.".to_string());
            }
        }
    }
}

struct RunningProxy {
    addr: String,
    state_handle: ProxyStateHandle,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), String>>>,
    shutdown_timeout: Duration,
}

async fn build_router_state(
    app: &AppHandle,
    config: ProxyConfig,
    sqlite_pool: Option<SqlitePool>,
) -> Result<(ProxyStateHandle, ProxyRouter), String> {
    let state = build_proxy_state(app, config, sqlite_pool).await?;
    let max_request_body_bytes = state.config.max_request_body_bytes;
    let state_handle = Arc::new(RwLock::new(state));
    let router =
        server::build_router(state_handle.clone(), max_request_body_bytes).with_state::<()>(
            state_handle.clone(),
        );
    Ok((state_handle.clone(), router))
}

async fn build_proxy_state(
    app: &AppHandle,
    config: ProxyConfig,
    sqlite_pool: Option<SqlitePool>,
) -> Result<Arc<ProxyState>, String> {
    if let Some(logging_state) = app.try_state::<LoggingState>() {
        logging_state.apply_level(config.log_level);
    }
    let log = Arc::new(LogWriter::new(sqlite_pool));
    let http_clients = super::http_client::ProxyHttpClients::new()?;
    let cursors = server::build_upstream_cursors(&config);
    let request_detail = app
        .state::<Arc<RequestDetailCapture>>()
        .inner()
        .clone();
    let token_rate = app
        .state::<Arc<super::token_rate::TokenRateTracker>>()
        .inner()
        .clone();
    let kiro_accounts = app
        .state::<Arc<crate::kiro::KiroAccountStore>>()
        .inner()
        .clone();
    let codex_accounts = app
        .state::<Arc<crate::codex::CodexAccountStore>>()
        .inner()
        .clone();
    Ok(Arc::new(ProxyState {
        config,
        http_clients,
        log,
        cursors,
        request_detail,
        token_rate,
        kiro_accounts,
        codex_accounts,
    }))
}
