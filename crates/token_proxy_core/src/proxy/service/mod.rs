use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::future::IntoFuture;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::config::ProxyConfig;
use super::log::LogWriter;
use super::request_detail::RequestDetailCapture;
use super::server;
use super::sqlite;
use super::ProxyState;
use crate::logging::LoggingState;
use crate::paths::TokenProxyPaths;

/// 默认优雅停机等待时间；超时后会强制 abort server task。
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

type ProxyStateHandle = Arc<RwLock<Arc<ProxyState>>>;
type ProxyRouter = axum::Router;

/// Proxy 运行时依赖（Ports & Adapters 注入点）。
///
/// - `paths`：配置/数据文件位置策略（CLI 使用 `./config.jsonc`；Tauri 使用 app_config_dir）
/// - `logging`：用于根据配置动态调整日志等级
/// - `request_detail` / `token_rate`：用于 UI/统计的可选能力（CLI 可用默认实现）
/// - `*_accounts`：上游鉴权 token 存储（CLI/Tauri 共享同一数据目录时可复用）
#[derive(Clone)]
pub struct ProxyContext {
    pub paths: Arc<TokenProxyPaths>,
    pub logging: LoggingState,
    pub request_detail: Arc<RequestDetailCapture>,
    pub token_rate: Arc<super::token_rate::TokenRateTracker>,
    pub kiro_accounts: Arc<crate::kiro::KiroAccountStore>,
    pub codex_accounts: Arc<crate::codex::CodexAccountStore>,
}

#[derive(Clone)]
pub struct ProxyServiceHandle {
    inner: Arc<ProxyService>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyConfigApplyBehavior {
    SavedOnly,
    Reload,
    Restart,
}

impl ProxyServiceHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ProxyService::new()),
        }
    }

    pub async fn status(&self) -> ProxyServiceStatus {
        self.inner.status().await
    }

    pub async fn start(&self, ctx: &ProxyContext) -> Result<ProxyServiceStatus, String> {
        self.inner.start(ctx).await
    }

    pub async fn stop(&self) -> Result<ProxyServiceStatus, String> {
        self.inner.stop().await
    }

    pub async fn restart(&self, ctx: &ProxyContext) -> Result<ProxyServiceStatus, String> {
        self.inner.restart(ctx).await
    }

    pub async fn reload(&self, ctx: &ProxyContext) -> Result<ProxyServiceStatus, String> {
        self.inner.reload(ctx).await
    }

    pub async fn reload_behavior(
        &self,
        ctx: &ProxyContext,
    ) -> Result<ProxyConfigApplyBehavior, String> {
        self.inner.reload_behavior(ctx).await
    }

    pub async fn apply_saved_config(&self, ctx: &ProxyContext) -> ProxyConfigSaveResult {
        self.inner.apply_saved_config(ctx).await
    }

    pub async fn cooling_account_ids(
        &self,
        provider: &str,
        account_ids: &[String],
    ) -> HashSet<String> {
        self.inner.cooling_account_ids(provider, account_ids).await
    }
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ProxyServiceState {
    Running,
    Stopped,
}

#[derive(Clone, Serialize)]
pub struct ProxyServiceStatus {
    pub state: ProxyServiceState,
    pub addr: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct ProxyConfigSaveResult {
    pub status: ProxyServiceStatus,
    pub apply_error: Option<String>,
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

impl ProxyConfigSaveResult {
    fn success(status: ProxyServiceStatus) -> Self {
        Self {
            status,
            apply_error: None,
        }
    }

    fn apply_error(status: ProxyServiceStatus, error: String) -> Self {
        Self {
            status,
            apply_error: Some(error),
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

    async fn start(&self, ctx: &ProxyContext) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.start(ctx).await?;
        Ok(inner.status())
    }

    async fn stop(&self) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.stop().await?;
        Ok(inner.status())
    }

    async fn restart(&self, ctx: &ProxyContext) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.restart(ctx).await?;
        Ok(inner.status())
    }

    async fn reload(&self, ctx: &ProxyContext) -> Result<ProxyServiceStatus, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.reload(ctx).await?;
        Ok(inner.status())
    }

    async fn reload_behavior(
        &self,
        ctx: &ProxyContext,
    ) -> Result<ProxyConfigApplyBehavior, String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.reload_behavior(ctx).await
    }

    async fn apply_saved_config(&self, ctx: &ProxyContext) -> ProxyConfigSaveResult {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.apply_saved_config(ctx).await
    }

    async fn cooling_account_ids(&self, provider: &str, account_ids: &[String]) -> HashSet<String> {
        let mut inner = self.inner.lock().await;
        inner.refresh_if_finished().await;
        inner.cooling_account_ids(provider, account_ids).await
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
            Some(running) => {
                ProxyServiceStatus::running(running.addr.clone(), self.last_error.clone())
            }
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

    async fn start(&mut self, ctx: &ProxyContext) -> Result<(), String> {
        let was_running = self.running.is_some();
        let result = self.start_inner(ctx).await;
        match &result {
            Ok(()) if !was_running => {
                self.last_error = None;
            }
            Err(error) => {
                self.last_error = Some(error.clone());
            }
            Ok(()) => {}
        }
        result
    }

    async fn start_inner(&mut self, ctx: &ProxyContext) -> Result<(), String> {
        if self.running.is_some() {
            return Ok(());
        }
        if self.sqlite_pool.is_none() {
            self.sqlite_pool = sqlite::open_write_pool(ctx.paths.as_ref()).await.ok();
        }
        let sqlite_pool = self.sqlite_pool.clone();
        let loaded_config = ProxyConfig::load(ctx.paths.as_ref()).await?;
        let addr = loaded_config.addr();

        let (state_handle, router) = build_router_state(ctx, loaded_config, sqlite_pool).await?;
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
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), String> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        self.finish_task(running).await;
        Ok(())
    }

    async fn restart(&mut self, ctx: &ProxyContext) -> Result<(), String> {
        self.stop().await?;
        self.start(ctx).await
    }

    async fn reload(&mut self, ctx: &ProxyContext) -> Result<(), String> {
        let was_running = self.running.is_some();
        let result = self.reload_inner(ctx).await;
        match &result {
            Ok(()) if was_running => {
                self.last_error = None;
            }
            Err(error) => {
                self.last_error = Some(error.clone());
            }
            Ok(()) => {}
        }
        result
    }

    async fn reload_inner(&mut self, ctx: &ProxyContext) -> Result<(), String> {
        tracing::debug!("proxy reload start");
        let start = Instant::now();
        if self.running.is_none() {
            tracing::debug!("proxy reload: not running, start instead");
            return self.start(ctx).await;
        }
        let loaded_config = ProxyConfig::load(ctx.paths.as_ref()).await?;
        let current_running_config = self.current_running_config().await;
        let addr = loaded_config.addr();
        let current_addr = current_running_config
            .as_ref()
            .map(|(current_addr, _)| current_addr.as_str())
            .unwrap_or_default()
            .to_string();

        tracing::debug!(addr = %addr, current_addr = %current_addr, "proxy reload config loaded");
        if classify_reload_behavior(current_running_config, &loaded_config)
            == ProxyConfigApplyBehavior::Restart
        {
            tracing::info!(
                addr = %addr,
                current_addr = %current_addr,
                "proxy reload detected restart-required config change"
            );
            return self.restart(ctx).await;
        }

        let sqlite_pool = self.sqlite_pool.clone();
        let new_state = build_proxy_state(ctx, loaded_config, sqlite_pool).await?;
        let Some(running) = self.running.as_ref() else {
            tracing::debug!("proxy reload: running cleared before swap");
            return Ok(());
        };
        {
            let mut guard = running.state_handle.write().await;
            *guard = new_state;
        }
        tracing::debug!(
            elapsed_ms = start.elapsed().as_millis(),
            "proxy reload applied"
        );
        Ok(())
    }

    async fn reload_behavior(
        &mut self,
        ctx: &ProxyContext,
    ) -> Result<ProxyConfigApplyBehavior, String> {
        let loaded_config = ProxyConfig::load(ctx.paths.as_ref()).await?;
        let current_running_config = self.current_running_config().await;
        Ok(classify_reload_behavior(
            current_running_config,
            &loaded_config,
        ))
    }

    async fn apply_saved_config(&mut self, ctx: &ProxyContext) -> ProxyConfigSaveResult {
        // 保存后的自动应用必须把“是否仍在运行”的判断与真正的 reload/restart
        // 放在同一把锁内完成，避免 save 与 stop/start 交错后把已停止的代理重新拉起。
        if self.running.is_none() {
            return ProxyConfigSaveResult::success(self.status());
        }

        match self.reload(ctx).await {
            Ok(()) => ProxyConfigSaveResult::success(self.status()),
            Err(error) => ProxyConfigSaveResult::apply_error(self.status(), error),
        }
    }

    async fn current_running_config(&self) -> Option<(String, usize)> {
        let running = self.running.as_ref()?;
        let guard = running.state_handle.read().await;
        Some((running.addr.clone(), guard.config.max_request_body_bytes))
    }

    async fn cooling_account_ids(&self, provider: &str, account_ids: &[String]) -> HashSet<String> {
        let Some(running) = self.running.as_ref() else {
            return HashSet::new();
        };
        let guard = running.state_handle.read().await;
        account_ids
            .iter()
            .filter(|account_id| guard.account_selector.is_cooling_down(provider, account_id))
            .cloned()
            .collect()
    }

    async fn finish_task(&mut self, mut running: RunningProxy) {
        if let Some(tx) = running.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = running.task.take() {
            self.await_stop(task, running.shutdown_timeout).await;
        }
    }

    async fn await_stop(
        &mut self,
        task: JoinHandle<Result<(), String>>,
        timeout_duration: Duration,
    ) {
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
    ctx: &ProxyContext,
    config: ProxyConfig,
    sqlite_pool: Option<SqlitePool>,
) -> Result<(ProxyStateHandle, ProxyRouter), String> {
    let state = build_proxy_state(ctx, config, sqlite_pool).await?;
    let max_request_body_bytes = state.config.max_request_body_bytes;
    let state_handle = Arc::new(RwLock::new(state));
    let router = server::build_router(state_handle.clone(), max_request_body_bytes)
        .with_state::<()>(state_handle.clone());
    Ok((state_handle.clone(), router))
}

async fn build_proxy_state(
    ctx: &ProxyContext,
    config: ProxyConfig,
    sqlite_pool: Option<SqlitePool>,
) -> Result<Arc<ProxyState>, String> {
    ctx.logging.apply_level(config.log_level);
    let log = Arc::new(LogWriter::new(sqlite_pool));
    let http_clients = super::http_client::ProxyHttpClients::new()?;
    let cursors = server::build_upstream_cursors(&config);
    let request_detail = ctx.request_detail.clone();
    let token_rate = ctx.token_rate.clone();
    let kiro_accounts = ctx.kiro_accounts.clone();
    let codex_accounts = ctx.codex_accounts.clone();
    Ok(Arc::new(ProxyState {
        upstream_selector: super::upstream_selector::UpstreamSelectorRuntime::new_with_cooldown(
            config.retryable_failure_cooldown,
        ),
        account_selector: super::account_selector::AccountSelectorRuntime::new_with_cooldown(
            config.retryable_failure_cooldown,
        ),
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

fn classify_reload_behavior(
    current_running_config: Option<(String, usize)>,
    loaded_config: &ProxyConfig,
) -> ProxyConfigApplyBehavior {
    let Some((current_addr, current_max_request_body_bytes)) = current_running_config else {
        return ProxyConfigApplyBehavior::SavedOnly;
    };
    if loaded_config.addr() != current_addr
        || loaded_config.max_request_body_bytes != current_max_request_body_bytes
    {
        return ProxyConfigApplyBehavior::Restart;
    }
    ProxyConfigApplyBehavior::Reload
}

#[cfg(test)]
mod tests;
