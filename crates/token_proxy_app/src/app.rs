//! Application composition root shared by CLI and Tauri adapters.

use std::{collections::HashSet, sync::Arc};

use token_proxy_account_codex::{CodexAccountStore, CodexLoginManager};
use token_proxy_account_kiro::{KiroAccountStore, KiroLoginManager};
use token_proxy_account_store::app_proxy::{self, AppProxyState};
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_account_xai::{XaiAccountStore, XaiLoginManager};

use token_proxy_runtime::{
    logging::LoggingState,
    proxy::{
        request_detail::RequestDetailCapture,
        service::{ProxyContext, ProxyServiceHandle},
        token_rate::TokenRateTracker,
    },
};

pub use token_proxy_runtime::proxy::{
    request_detail::RequestDetailCaptureState,
    service::{
        ProxyConfigApplyBehavior, ProxyConfigSaveResult, ProxyServiceState, ProxyServiceStatus,
        UpstreamModelProbe,
    },
    token_rate::TokenRateSnapshot,
};
pub use token_proxy_storage::{
    dashboard::{DashboardRange, DashboardSnapshot},
    logs::RequestLogDetail,
    pricing::{ModelPricingSettingsInput, ModelPricingSettingsSnapshot, RemoteCatalogRefresh},
};

pub type RequestDetailChangeHandler = Arc<dyn Fn(RequestDetailCaptureState) + Send + Sync>;

/// Fully composed application services. Adapters may register the returned
/// handles in their own state container, but they do not construct internals.
#[derive(Clone)]
pub struct TokenProxyApp {
    pub(crate) paths: Arc<TokenProxyPaths>,
    logging: LoggingState,
    app_proxy: AppProxyState,
    request_detail: Arc<RequestDetailCapture>,
    token_rate: Arc<TokenRateTracker>,
    pub(crate) kiro_accounts: Arc<KiroAccountStore>,
    pub(crate) codex_accounts: Arc<CodexAccountStore>,
    pub(crate) xai_accounts: Arc<XaiAccountStore>,
    pub(crate) kiro_login: Arc<KiroLoginManager>,
    pub(crate) codex_login: Arc<CodexLoginManager>,
    pub(crate) xai_login: Arc<XaiLoginManager>,
    pub(crate) proxy: ProxyServiceHandle,
    pub(crate) proxy_context: ProxyContext,
    /// Account/config mutation 串行化与 fault-injection 状态。
    pub(crate) lifecycle: Arc<crate::account_lifecycle::AccountLifecycle>,
}

impl TokenProxyApp {
    /// Composes one application instance around a single data directory.
    pub fn open(
        paths: TokenProxyPaths,
        logging: LoggingState,
        on_request_detail_change: Option<RequestDetailChangeHandler>,
    ) -> Result<Self, String> {
        let paths = Arc::new(paths);
        let app_proxy = app_proxy::new_state();
        let request_detail = Arc::new(RequestDetailCapture::new(on_request_detail_change));
        let token_rate = TokenRateTracker::new();
        let kiro_accounts = Arc::new(KiroAccountStore::new(&paths, app_proxy.clone())?);
        let codex_accounts = Arc::new(CodexAccountStore::new(&paths, app_proxy.clone())?);
        let xai_accounts = Arc::new(XaiAccountStore::new(&paths, app_proxy.clone())?);
        let kiro_login = Arc::new(KiroLoginManager::new(
            kiro_accounts.clone(),
            app_proxy.clone(),
        ));
        let codex_login = Arc::new(CodexLoginManager::new(
            codex_accounts.clone(),
            app_proxy.clone(),
        ));
        let xai_login = Arc::new(XaiLoginManager::new(
            xai_accounts.clone(),
            app_proxy.clone(),
        ));
        let proxy = ProxyServiceHandle::new();
        let proxy_context = ProxyContext {
            paths: paths.clone(),
            logging: logging.clone(),
            request_detail: request_detail.clone(),
            token_rate: token_rate.clone(),
            kiro_accounts: kiro_accounts.clone(),
            codex_accounts: codex_accounts.clone(),
            xai_accounts: xai_accounts.clone(),
        };
        tracing::debug!(data_dir = %paths.data_dir().display(), "token proxy app composed");
        Ok(Self {
            paths,
            logging,
            app_proxy,
            request_detail,
            token_rate,
            kiro_accounts,
            codex_accounts,
            xai_accounts,
            kiro_login,
            codex_login,
            xai_login,
            proxy,
            proxy_context,
            lifecycle: Arc::new(crate::account_lifecycle::AccountLifecycle::new()),
        })
    }

    /// 测试用 fault-injection 句柄（生产路径不依赖）。
    pub fn lifecycle_faults(&self) -> Arc<crate::account_lifecycle::LifecycleFaults> {
        self.lifecycle.faults()
    }

    pub fn paths(&self) -> Arc<TokenProxyPaths> {
        self.paths.clone()
    }

    pub fn logging(&self) -> LoggingState {
        self.logging.clone()
    }

    pub fn app_proxy(&self) -> AppProxyState {
        self.app_proxy.clone()
    }

    pub fn request_detail(&self) -> Arc<RequestDetailCapture> {
        self.request_detail.clone()
    }

    pub fn token_rate(&self) -> Arc<TokenRateTracker> {
        self.token_rate.clone()
    }

    pub fn kiro_accounts(&self) -> Arc<KiroAccountStore> {
        self.kiro_accounts.clone()
    }

    pub fn codex_accounts(&self) -> Arc<CodexAccountStore> {
        self.codex_accounts.clone()
    }

    pub fn xai_accounts(&self) -> Arc<XaiAccountStore> {
        self.xai_accounts.clone()
    }

    pub fn kiro_login(&self) -> Arc<KiroLoginManager> {
        self.kiro_login.clone()
    }

    pub fn codex_login(&self) -> Arc<CodexLoginManager> {
        self.codex_login.clone()
    }

    pub fn xai_login(&self) -> Arc<XaiLoginManager> {
        self.xai_login.clone()
    }

    /// 返回当前代理生命周期状态。
    pub async fn proxy_status(&self) -> ProxyServiceStatus {
        self.proxy.status().await
    }

    // start_proxy / restart_proxy：见 account_lifecycle（启动前 reconcile）。

    /// 停止代理并等待优雅停机完成。
    pub async fn stop_proxy(&self) -> Result<ProxyServiceStatus, String> {
        self.proxy.stop().await
    }

    /// 热重载当前保存配置；需要换监听地址时内部自动重启。
    pub async fn reload_proxy(&self) -> Result<ProxyServiceStatus, String> {
        self.proxy.reload(&self.proxy_context).await
    }

    /// 判断保存配置需要热重载还是完整重启。
    pub async fn proxy_reload_behavior(&self) -> Result<ProxyConfigApplyBehavior, String> {
        self.proxy.reload_behavior(&self.proxy_context).await
    }

    /// 保存配置后按当前运行状态应用，不会意外启动已停止的代理。
    pub async fn apply_saved_proxy_config(&self) -> ProxyConfigSaveResult {
        self.proxy.apply_saved_config(&self.proxy_context).await
    }

    /// 查询指定 Provider 账户的运行时冷却状态。
    pub async fn cooling_account_ids(
        &self,
        provider: &str,
        account_ids: &[String],
    ) -> HashSet<String> {
        self.proxy.cooling_account_ids(provider, account_ids).await
    }

    /// 读取 Dashboard 快照，并合并运行时模型探测结果。
    pub async fn read_dashboard_snapshot(
        &self,
        range: DashboardRange,
        offset: Option<u32>,
        upstream_id: Option<String>,
        account_id: Option<String>,
        public_only: bool,
        model: Option<String>,
    ) -> Result<DashboardSnapshot, String> {
        let pool =
            token_proxy_storage::sqlite::open_read_pool(&self.paths.sqlite_db_path()).await?;
        let mut snapshot = token_proxy_storage::dashboard::read_snapshot(
            &pool,
            range,
            offset,
            upstream_id,
            account_id,
            public_only,
            model,
        )
        .await?;
        snapshot.model_probes = self.proxy.model_discovery_snapshot().await;
        Ok(snapshot)
    }

    /// 立即刷新所有上游的模型目录探测缓存。
    pub async fn refresh_model_discovery(&self) -> Vec<UpstreamModelProbe> {
        self.proxy.refresh_model_discovery().await
    }

    /// 读取单条请求日志详情。
    pub async fn read_request_log_detail(&self, id: u64) -> Result<RequestLogDetail, String> {
        let pool =
            token_proxy_storage::sqlite::open_read_pool(&self.paths.sqlite_db_path()).await?;
        token_proxy_storage::logs::read_request_log_detail(&pool, id).await
    }

    /// 读取当前临时请求详情捕获状态。
    pub fn request_detail_capture(&self) -> RequestDetailCaptureState {
        self.request_detail.snapshot()
    }

    /// 开启或关闭临时请求详情捕获。
    pub fn set_request_detail_capture(&self, enabled: bool) -> RequestDetailCaptureState {
        if enabled {
            self.request_detail.arm()
        } else {
            self.request_detail.disarm()
        }
    }

    /// 读取模型价格设置。
    pub async fn read_model_pricing_settings(
        &self,
    ) -> Result<ModelPricingSettingsSnapshot, String> {
        let pool =
            token_proxy_storage::sqlite::open_read_pool(&self.paths.sqlite_db_path()).await?;
        token_proxy_storage::pricing::read_model_pricing_settings_snapshot(&pool).await
    }

    /// 保存模型价格设置。
    pub async fn save_model_pricing_settings(
        &self,
        settings: ModelPricingSettingsInput,
    ) -> Result<ModelPricingSettingsSnapshot, String> {
        let pool =
            token_proxy_storage::sqlite::open_write_pool(&self.paths.sqlite_db_path()).await?;
        token_proxy_storage::pricing::save_model_pricing_settings(&pool, settings).await
    }

    /// 重置模型价格设置。
    pub async fn reset_model_pricing_settings(
        &self,
    ) -> Result<ModelPricingSettingsSnapshot, String> {
        let pool =
            token_proxy_storage::sqlite::open_write_pool(&self.paths.sqlite_db_path()).await?;
        token_proxy_storage::pricing::reset_model_pricing_settings(&pool).await
    }

    /// 刷新远端模型价格目录；失败时调用方继续使用缓存或内置目录。
    pub async fn refresh_model_pricing_catalog(
        &self,
        proxy_url: Option<&str>,
    ) -> Result<RemoteCatalogRefresh, String> {
        crate::pricing_refresh::refresh_remote_model_pricing_catalog(self.paths.as_ref(), proxy_url)
            .await
    }

    /// 订阅 token-rate 活动变化，供托盘空闲等待使用。
    pub fn subscribe_token_rate_activity(&self) -> tokio::sync::watch::Receiver<u64> {
        self.token_rate.subscribe_activity()
    }

    /// 唤醒 token-rate 展示循环。
    pub fn notify_token_rate_activity(&self) {
        self.token_rate.notify_activity();
    }

    /// 开关 token-rate 采集。
    pub async fn set_token_rate_enabled(&self, enabled: bool) {
        self.token_rate.set_enabled(enabled).await;
    }

    /// 返回最近滑动窗口内的 token-rate 快照。
    pub async fn token_rate_snapshot(&self) -> TokenRateSnapshot {
        self.token_rate.snapshot().await
    }

    /// 当前是否仍有活跃代理请求。
    pub fn has_active_proxy_requests(&self) -> bool {
        self.token_rate.has_active_requests()
    }
}
