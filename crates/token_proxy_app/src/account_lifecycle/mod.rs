//! Account-as-Upstream Phase C2：账户/配置生命周期编排。
//!
//! 跨 SQLite 与 config 文件无法单事务时，使用「先快照 → 变更 → 失败补偿」状态机。
//! 关键 mutation 提交到受控 MutationWorker：调用方 await 可被取消，worker 仍跑完。
//! 禁止持锁跨 OAuth 用户等待；禁止依赖「调用方不会取消」。

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use token_proxy_account_codex::{
    CodexAccountSummary, CodexLoginPollClaim, CodexLoginPollResponse, CodexLoginStatus,
    CodexRefreshTokenClient, CodexTokenRecord,
};
use token_proxy_account_kiro::{
    KiroAccountSummary, KiroLoginPollClaim, KiroLoginPollResponse, KiroLoginStatus, KiroTokenRecord,
};
use token_proxy_account_xai::{
    XaiAccountSummary, XaiLoginPollClaim, XaiLoginPollResponse, XaiLoginStatus, XaiTokenRecord,
};
use token_proxy_config::{AccountProvider, LogLevel, ProxyConfigFile, TrayTokenRateConfig};
use token_proxy_runtime::proxy::service::{ProxyConfigSaveResult, ProxyServiceStatus};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::account_upstreams::{
    reconcile_account_upstreams, removed_account_bindings, AccountUpstreamRef,
};
use crate::app::TokenProxyApp;

/// 保存配置成功后需由 Tauri 应用的侧效应（tray/log/app_proxy）。
/// 仅在 config 写盘与 runtime apply 均成功后返回，避免半应用。
#[derive(Clone)]
pub struct ConfigSideEffects {
    pub log_level: LogLevel,
    pub app_proxy_url: Option<String>,
    pub tray_token_rate: TrayTokenRateConfig,
}

/// `save_proxy_config` 编排成功结果。
#[derive(Clone)]
pub struct SaveProxyConfigOutcome {
    pub proxy: ProxyConfigSaveResult,
    pub side_effects: ConfigSideEffects,
    /// 本次级联删除的 credential 数量（不含 rename 保留绑定）。
    pub removed_credentials: usize,
    pub config_changed: bool,
}

// expect_err 需要 Ok 类型实现 Debug；不依赖嵌套类型的完整 Debug。
impl std::fmt::Debug for SaveProxyConfigOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveProxyConfigOutcome")
            .field("removed_credentials", &self.removed_credentials)
            .field("config_changed", &self.config_changed)
            .field("has_apply_error", &self.proxy.apply_error.is_some())
            .finish_non_exhaustive()
    }
}

/// reconcile / import 后的配置变更摘要（不含 secret）。
#[derive(Clone, Debug, Default)]
pub struct AccountBindingMutationSummary {
    pub config_changed: bool,
    pub added_bindings: usize,
    pub applied: bool,
}

/// 可测性 fault-injection：只拦截编排边界，不复制业务逻辑。
#[derive(Default)]
pub struct LifecycleFaults {
    pub fail_config_write: AtomicBool,
    pub fail_credential_delete: AtomicBool,
    pub fail_apply: AtomicBool,
    /// 成功完成的 config 写次数（测试断言 apply 次数用）。
    pub config_write_count: AtomicUsize,
    pub apply_count: AtomicUsize,
    /// 关键 mutation worker 在途任务数（取消等待后仍可 >0，直至完成）。
    pub critical_in_flight: AtomicUsize,
    /// 关键 mutation 完成次数（含失败结果）。
    pub critical_completed: AtomicUsize,
    /// 持 mutation_lock 后额外 sleep（毫秒），便于取消窗口测试；0 表示关闭。
    pub delay_after_lock_ms: AtomicU64,
}

impl LifecycleFaults {
    pub fn reset(&self) {
        self.fail_config_write.store(false, Ordering::SeqCst);
        self.fail_credential_delete.store(false, Ordering::SeqCst);
        self.fail_apply.store(false, Ordering::SeqCst);
        self.config_write_count.store(0, Ordering::SeqCst);
        self.apply_count.store(0, Ordering::SeqCst);
        self.critical_in_flight.store(0, Ordering::SeqCst);
        self.critical_completed.store(0, Ordering::SeqCst);
        self.delay_after_lock_ms.store(0, Ordering::SeqCst);
    }
}

type MutationJob = Pin<Box<dyn Future<Output = ()> + Send>>;

/// 受控 mutation worker：单队列串行执行关键编排。
///
/// 调用方只 await oneshot 回复；取消/abort 等待 **不会** abort 队列内任务。
/// 与 ad-hoc detached task 不同：唯一 worker、可计数、可测 in_flight。
struct MutationWorker {
    tx: mpsc::UnboundedSender<MutationJob>,
    rx: StdMutex<Option<mpsc::UnboundedReceiver<MutationJob>>>,
    started: AtomicBool,
}

impl MutationWorker {
    fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: StdMutex::new(Some(rx)),
            started: AtomicBool::new(false),
        }
    }

    fn ensure_started(&self) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let rx = self
            .rx
            .lock()
            .expect("mutation worker rx lock")
            .take()
            .expect("mutation worker rx already taken");
        // 唯一长期 worker；任务内不跨 OAuth 用户等待。
        tokio::spawn(async move {
            tracing::debug!("account lifecycle mutation worker started");
            let mut rx = rx;
            while let Some(job) = rx.recv().await {
                job.await;
            }
            tracing::debug!("account lifecycle mutation worker stopped");
        });
    }
}

/// 挂在 TokenProxyApp 上的编排运行时状态。
pub struct AccountLifecycle {
    /// Arc 以便 worker 任务先 clone 锁再 move app，避免持锁借用与 move 冲突。
    pub(crate) mutation_lock: Arc<Mutex<()>>,
    worker: MutationWorker,
    pub(crate) faults: Arc<LifecycleFaults>,
}

impl AccountLifecycle {
    pub fn new() -> Self {
        Self {
            mutation_lock: Arc::new(Mutex::new(())),
            worker: MutationWorker::new(),
            faults: Arc::new(LifecycleFaults::default()),
        }
    }

    pub fn faults(&self) -> Arc<LifecycleFaults> {
        self.faults.clone()
    }

    /// 提交关键 mutation：即使调用方取消等待，worker 仍跑完并更新持久化状态。
    ///
    /// 返回 `Result` 以便 reply 丢失时给出明确错误，而不 panic。
    async fn run_to_completion<T, F, Fut>(&self, work: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        self.worker.ensure_started();
        let (reply_tx, reply_rx) = oneshot::channel();
        let faults = Arc::clone(&self.faults);
        faults.critical_in_flight.fetch_add(1, Ordering::SeqCst);
        let job: MutationJob = Box::pin(async move {
            let result = work().await;
            // 调用方已取消时 send 失败可忽略；mutation 已完成。
            let _ = reply_tx.send(result);
            faults.critical_in_flight.fetch_sub(1, Ordering::SeqCst);
            faults.critical_completed.fetch_add(1, Ordering::SeqCst);
        });
        if self.worker.tx.send(job).is_err() {
            tracing::error!("lifecycle mutation worker channel closed");
            self.faults
                .critical_in_flight
                .fetch_sub(1, Ordering::SeqCst);
            return Err("lifecycle mutation worker channel closed".to_string());
        }
        reply_rx
            .await
            .map_err(|_| "lifecycle mutation worker reply lost".to_string())
    }
}

impl Default for AccountLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

/// 单账户 credential 快照（按 provider 分型，避免跨 provider 混用）。
enum CredentialSnapshot {
    Kiro(Option<token_proxy_account_kiro::KiroTokenRecord>),
    Codex(Option<token_proxy_account_codex::CodexTokenRecord>),
    Xai(Option<token_proxy_account_xai::XaiTokenRecord>),
}

struct RemovedCredentialBackup {
    reference: AccountUpstreamRef,
    snapshot: CredentialSnapshot,
}

impl TokenProxyApp {
    /// 关键 lifecycle mutation：worker 串行 + mutation_lock；调用方取消不中断执行。
    async fn run_critical_mutation<F, Fut, T>(&self, op: F) -> Result<T, String>
    where
        F: FnOnce(TokenProxyApp) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
        T: Send + 'static,
    {
        let app = self.clone();
        let delay_ms = self
            .lifecycle
            .faults
            .delay_after_lock_ms
            .load(Ordering::SeqCst);
        self.lifecycle
            .run_to_completion(move || {
                let app = app;
                async move {
                    // 先 clone Arc 锁，再 move app 进 op，避免 MutexGuard 借用 app。
                    let lock = Arc::clone(&app.lifecycle.mutation_lock);
                    let _guard = lock.lock().await;
                    if delay_ms > 0 {
                        // 仅测试注入取消窗口；生产 delay_ms 恒为 0。
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    op(app).await
                }
            })
            .await?
    }

    /// 启动代理前先 reconcile 三类现存账户；缺失绑定一次写 config 再 start。
    pub async fn start_proxy(&self) -> Result<ProxyServiceStatus, String> {
        self.run_critical_mutation(|app| async move {
            tracing::debug!("start_proxy: reconciling account upstreams before start");
            app.reconcile_all_accounts_locked().await?;
            app.proxy.start(&app.proxy_context).await
        })
        .await
    }

    /// 重启前同样补齐绑定，避免缺失 Upstream 配置进入 runtime。
    pub async fn restart_proxy(&self) -> Result<ProxyServiceStatus, String> {
        self.run_critical_mutation(|app| async move {
            tracing::debug!("restart_proxy: reconciling account upstreams before restart");
            app.reconcile_all_accounts_locked().await?;
            app.proxy.restart(&app.proxy_context).await
        })
        .await
    }

    /// 保存新 config：先按 old→new 级联删 credential，再写盘并 apply。
    /// 关键路径：调用方取消后 worker 仍完成 cascade，避免半状态。
    pub async fn save_proxy_config(
        &self,
        config: ProxyConfigFile,
    ) -> Result<SaveProxyConfigOutcome, String> {
        self.run_critical_mutation(
            move |app| async move { app.save_proxy_config_locked(config).await },
        )
        .await
    }

    // --- Kiro 入口 ---

    pub async fn kiro_import_ide(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        self.run_critical_mutation(move |app| async move {
            // snapshot→import→binding/restore 全程持 provider gate，避免并发 save 被旧 snapshot 覆盖。
            let txn = app.kiro_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_ide_tokens(directory).await {
                Ok(items) => items,
                Err(err) => {
                    // 部分导入也可能已写库；全量恢复快照保证一致。
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Kiro,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            // txn drop 释放 gate；成功路径不 restore。
            Ok(imported)
        })
        .await
    }

    pub async fn kiro_import_kam(&self, path: PathBuf) -> Result<Vec<KiroAccountSummary>, String> {
        self.run_critical_mutation(move |app| async move {
            let txn = app.kiro_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_kam_export(path).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Kiro,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn kiro_poll_login(&self, state: &str) -> Result<KiroLoginPollResponse, String> {
        // 登录小事务：OAuth 任务只 prepare；claim+commit 在 worker 内跑完。
        // 调用方取消 poll 等待不丢 credential；下次 poll 可拿到 Success 或可恢复 pending。
        let state = state.to_string();
        self.run_critical_mutation(move |app| async move {
            match app.kiro_login.claim_prepared_login(&state).await? {
                KiroLoginPollClaim::Response(response) => Ok(response),
                KiroLoginPollClaim::Prepared {
                    state: session_state,
                    record,
                } => app.commit_kiro_login_locked(&session_state, record).await,
            }
        })
        .await
    }

    // --- Codex 入口 ---

    pub async fn codex_import_file(
        &self,
        path: PathBuf,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        self.run_critical_mutation(move |app| async move {
            let txn = app.codex_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_file(path).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Codex,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn codex_import_text(
        &self,
        contents: &str,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        let contents = contents.to_string();
        self.run_critical_mutation(move |app| async move {
            let txn = app.codex_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_text(&contents).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Codex,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn codex_import_refresh_tokens(
        &self,
        contents: &str,
        client: CodexRefreshTokenClient,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        let contents = contents.to_string();
        self.run_critical_mutation(move |app| async move {
            let txn = app.codex_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_refresh_tokens(&contents, client).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Codex,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn codex_poll_login(&self, state: &str) -> Result<CodexLoginPollResponse, String> {
        let state = state.to_string();
        self.run_critical_mutation(move |app| async move {
            match app.codex_login.claim_prepared_login(&state).await? {
                CodexLoginPollClaim::Response(response) => Ok(response),
                CodexLoginPollClaim::Prepared {
                    state: session_state,
                    record,
                } => app.commit_codex_login_locked(&session_state, record).await,
            }
        })
        .await
    }

    // --- xAI 入口 ---

    pub async fn xai_import_file(&self, path: PathBuf) -> Result<Vec<XaiAccountSummary>, String> {
        self.run_critical_mutation(move |app| async move {
            let txn = app.xai_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_file(path).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Xai,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn xai_import_text(&self, contents: &str) -> Result<Vec<XaiAccountSummary>, String> {
        let contents = contents.to_string();
        self.run_critical_mutation(move |app| async move {
            let txn = app.xai_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_text(&contents).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Xai,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn xai_import_refresh_tokens(
        &self,
        contents: &str,
    ) -> Result<Vec<XaiAccountSummary>, String> {
        let contents = contents.to_string();
        self.run_critical_mutation(move |app| async move {
            let txn = app.xai_accounts.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await?;
            let imported = match txn.import_refresh_tokens(&contents).await {
                Ok(items) => items,
                Err(err) => {
                    if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                        return Err(join_errors(err, restore_err));
                    }
                    return Err(err);
                }
            };
            if let Err(err) = app
                .ensure_imported_bindings_locked(
                    AccountProvider::Xai,
                    imported.iter().map(|item| item.account_id.as_str()),
                )
                .await
            {
                if let Err(restore_err) = txn.restore_all_records(snapshot).await {
                    return Err(join_errors(err, restore_err));
                }
                return Err(err);
            }
            Ok(imported)
        })
        .await
    }

    pub async fn xai_poll_login(&self, state: &str) -> Result<XaiLoginPollResponse, String> {
        let state = state.to_string();
        self.run_critical_mutation(move |app| async move {
            match app.xai_login.claim_prepared_login(&state).await? {
                XaiLoginPollClaim::Response(response) => Ok(response),
                XaiLoginPollClaim::Prepared {
                    state: session_state,
                    record,
                } => app.commit_xai_login_locked(&session_state, record).await,
            }
        })
        .await
    }

    /// 测试/补偿：在 mutation worker 内确保指定账户绑定存在。
    pub async fn ensure_account_bindings_for_test(
        &self,
        accounts: impl IntoIterator<Item = AccountUpstreamRef>,
    ) -> Result<AccountBindingMutationSummary, String> {
        let refs: Vec<_> = accounts.into_iter().collect();
        self.run_critical_mutation(move |app| async move { app.ensure_bindings_locked(refs).await })
            .await
    }

    /// 测试：显式启动前 reconcile（与 start_proxy 共用逻辑）。
    pub async fn reconcile_existing_accounts_for_test(
        &self,
    ) -> Result<AccountBindingMutationSummary, String> {
        self.run_critical_mutation(|app| async move { app.reconcile_all_accounts_locked().await })
            .await
    }

    // --- 内部编排（调用方必须已持 mutation_lock） ---

    async fn reconcile_all_accounts_locked(&self) -> Result<AccountBindingMutationSummary, String> {
        let mut accounts = Vec::new();
        for item in self.kiro_accounts.list_accounts().await? {
            accounts.push(AccountUpstreamRef::new(
                AccountProvider::Kiro,
                item.account_id,
            ));
        }
        for item in self.codex_accounts.list_accounts().await? {
            accounts.push(AccountUpstreamRef::new(
                AccountProvider::Codex,
                item.account_id,
            ));
        }
        for item in self.xai_accounts.list_accounts().await? {
            accounts.push(AccountUpstreamRef::new(
                AccountProvider::Xai,
                item.account_id,
            ));
        }
        tracing::info!(
            account_count = accounts.len(),
            "reconcile existing accounts for account-as-upstream"
        );
        self.ensure_bindings_locked(accounts).await
    }

    async fn ensure_imported_bindings_locked(
        &self,
        provider: AccountProvider,
        account_ids: impl Iterator<Item = &str>,
    ) -> Result<AccountBindingMutationSummary, String> {
        let accounts: Vec<AccountUpstreamRef> = account_ids
            .map(|id| AccountUpstreamRef::new(provider, id))
            .collect();
        self.ensure_bindings_locked(accounts).await
    }

    async fn ensure_bindings_locked(
        &self,
        accounts: Vec<AccountUpstreamRef>,
    ) -> Result<AccountBindingMutationSummary, String> {
        if accounts.is_empty() {
            return Ok(AccountBindingMutationSummary::default());
        }
        let old_config = token_proxy_config::read_config(self.paths.as_ref())
            .await?
            .config;
        let reconciled = reconcile_account_upstreams(&old_config, accounts)?;
        if !reconciled.changed {
            tracing::debug!("account upstream reconcile: no config change (credential-only path)");
            return Ok(AccountBindingMutationSummary {
                config_changed: false,
                added_bindings: 0,
                applied: false,
            });
        }

        let added = reconciled.added.len();
        tracing::info!(
            added_bindings = added,
            "account upstream reconcile will write config"
        );
        self.write_config_checked(reconciled.config).await?;
        match self.apply_saved_checked().await {
            Ok(()) => Ok(AccountBindingMutationSummary {
                config_changed: true,
                added_bindings: added,
                applied: true,
            }),
            Err(apply_err) => {
                // apply 失败：恢复旧 config 并尽力 re-apply 旧 runtime。
                let rollback_err = self.rollback_config_and_runtime(&old_config).await.err();
                Err(join_optional(apply_err, rollback_err))
            }
        }
    }

    /// 登录提交小事务（调用方已持 mutation_lock）：
    /// 1) commit credential（可恢复旧 record）
    /// 2) ensure binding / write+apply config
    /// 3) 任一步失败恢复该账户旧 credential，不碰其它账户
    async fn commit_kiro_login_locked(
        &self,
        state: &str,
        record: KiroTokenRecord,
    ) -> Result<KiroLoginPollResponse, String> {
        let (summary, previous) = match self.kiro_accounts.commit_login_record(record).await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.kiro_login.fail_login_commit(state, err.clone()).await;
                return Ok(KiroLoginPollResponse {
                    state: state.to_string(),
                    status: KiroLoginStatus::Error,
                    error: Some(err),
                    account: None,
                });
            }
        };
        let account_id = summary.account_id.clone();
        if let Err(err) = self
            .ensure_imported_bindings_locked(
                AccountProvider::Kiro,
                std::iter::once(account_id.as_str()),
            )
            .await
        {
            // config/apply 失败：恢复该账户旧 record（新登录则删除）。
            if let Err(restore_err) = self
                .kiro_accounts
                .restore_account_record(&account_id, previous)
                .await
            {
                let joined = join_errors(err, restore_err);
                self.kiro_login
                    .fail_login_commit(state, joined.clone())
                    .await;
                return Err(joined);
            }
            self.kiro_login.fail_login_commit(state, err.clone()).await;
            return Err(err);
        }
        self.kiro_login
            .complete_login_commit(state, summary.clone())
            .await;
        Ok(KiroLoginPollResponse {
            state: state.to_string(),
            status: KiroLoginStatus::Success,
            error: None,
            account: Some(summary),
        })
    }

    async fn commit_codex_login_locked(
        &self,
        state: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexLoginPollResponse, String> {
        let (summary, previous) = match self.codex_accounts.commit_login_record(record).await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.codex_login.fail_login_commit(state, err.clone()).await;
                return Ok(CodexLoginPollResponse {
                    state: state.to_string(),
                    status: CodexLoginStatus::Error,
                    error: Some(err),
                    account: None,
                });
            }
        };
        let account_id = summary.account_id.clone();
        if let Err(err) = self
            .ensure_imported_bindings_locked(
                AccountProvider::Codex,
                std::iter::once(account_id.as_str()),
            )
            .await
        {
            if let Err(restore_err) = self
                .codex_accounts
                .restore_account_record(&account_id, previous)
                .await
            {
                let joined = join_errors(err, restore_err);
                self.codex_login
                    .fail_login_commit(state, joined.clone())
                    .await;
                return Err(joined);
            }
            self.codex_login.fail_login_commit(state, err.clone()).await;
            return Err(err);
        }
        self.codex_login
            .complete_login_commit(state, summary.clone())
            .await;
        Ok(CodexLoginPollResponse {
            state: state.to_string(),
            status: CodexLoginStatus::Success,
            error: None,
            account: Some(summary),
        })
    }

    async fn commit_xai_login_locked(
        &self,
        state: &str,
        record: XaiTokenRecord,
    ) -> Result<XaiLoginPollResponse, String> {
        let (summary, previous) = match self.xai_accounts.commit_login_record(record).await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.xai_login.fail_login_commit(state, err.clone()).await;
                return Ok(XaiLoginPollResponse {
                    state: state.to_string(),
                    status: XaiLoginStatus::Error,
                    error: Some(err),
                    account: None,
                });
            }
        };
        let account_id = summary.account_id.clone();
        if let Err(err) = self
            .ensure_imported_bindings_locked(
                AccountProvider::Xai,
                std::iter::once(account_id.as_str()),
            )
            .await
        {
            if let Err(restore_err) = self
                .xai_accounts
                .restore_account_record(&account_id, previous)
                .await
            {
                let joined = join_errors(err, restore_err);
                self.xai_login
                    .fail_login_commit(state, joined.clone())
                    .await;
                return Err(joined);
            }
            self.xai_login.fail_login_commit(state, err.clone()).await;
            return Err(err);
        }
        self.xai_login
            .complete_login_commit(state, summary.clone())
            .await;
        Ok(XaiLoginPollResponse {
            state: state.to_string(),
            status: XaiLoginStatus::Success,
            error: None,
            account: Some(summary),
        })
    }

    async fn save_proxy_config_locked(
        &self,
        new_config: ProxyConfigFile,
    ) -> Result<SaveProxyConfigOutcome, String> {
        let old_config = token_proxy_config::read_config(self.paths.as_ref())
            .await?
            .config;

        // 语义完全相同：不删 credential、不写盘、不 apply。
        if configs_semantically_equal(&old_config, &new_config) {
            tracing::debug!("save_proxy_config no-op: config semantically unchanged");
            let status = self.proxy.status().await;
            return Ok(SaveProxyConfigOutcome {
                proxy: ProxyConfigSaveResult {
                    status,
                    apply_error: None,
                },
                side_effects: ConfigSideEffects {
                    log_level: old_config.log_level,
                    app_proxy_url: token_proxy_config::app_proxy_url_from_config(&old_config)
                        .ok()
                        .flatten(),
                    tray_token_rate: old_config.tray_token_rate.clone(),
                },
                removed_credentials: 0,
                config_changed: false,
            });
        }

        let removed = removed_account_bindings(&old_config, &new_config);
        tracing::info!(
            removed_bindings = removed.len(),
            "save_proxy_config cascade plan"
        );

        // 写 config 前先快照并删除 removed credentials。
        let mut backups = Vec::with_capacity(removed.len());
        for reference in &removed {
            let snapshot = self.snapshot_credential(reference).await?;
            backups.push(RemovedCredentialBackup {
                reference: reference.clone(),
                snapshot,
            });
        }

        // 先删 credential；任一步失败则恢复已删并禁止写 config。
        let mut deleted = Vec::new();
        for backup in &backups {
            if self
                .lifecycle
                .faults
                .fail_credential_delete
                .load(Ordering::SeqCst)
            {
                let restore_err = self.restore_credential_backups(&deleted).await.err();
                return Err(join_optional(
                    "injected credential delete failure".to_string(),
                    restore_err,
                ));
            }
            match self.delete_credential(&backup.reference).await {
                Ok(()) => deleted.push(backup),
                Err(err) => {
                    let restore_err = self.restore_credential_backups(&deleted).await.err();
                    return Err(join_optional(err, restore_err));
                }
            }
        }

        let side_effects = ConfigSideEffects {
            log_level: new_config.log_level,
            app_proxy_url: token_proxy_config::app_proxy_url_from_config(&new_config)
                .ok()
                .flatten(),
            tray_token_rate: new_config.tray_token_rate.clone(),
        };

        if let Err(err) = self.write_config_checked(new_config).await {
            // config 写失败：恢复 credential；旧 config 保持。
            let restore_err = self.restore_credential_backups(&deleted).await.err();
            return Err(join_optional(err, restore_err));
        }

        match self.apply_saved_checked().await {
            Ok(()) => {
                let status = self.proxy.status().await;
                Ok(SaveProxyConfigOutcome {
                    proxy: ProxyConfigSaveResult {
                        status,
                        apply_error: None,
                    },
                    side_effects,
                    removed_credentials: deleted.len(),
                    config_changed: true,
                })
            }
            Err(apply_err) => {
                // apply 失败：恢复 credential + 旧 config + 尽力 re-apply。
                let mut rollback_parts = Vec::new();
                if let Err(e) = self.restore_credential_backups(&deleted).await {
                    rollback_parts.push(e);
                }
                if let Err(e) = self.rollback_config_and_runtime(&old_config).await {
                    rollback_parts.push(e);
                }
                Err(join_optional(
                    apply_err,
                    if rollback_parts.is_empty() {
                        None
                    } else {
                        Some(rollback_parts.join("; "))
                    },
                ))
            }
        }
    }

    async fn write_config_checked(&self, config: ProxyConfigFile) -> Result<(), String> {
        if self
            .lifecycle
            .faults
            .fail_config_write
            .load(Ordering::SeqCst)
        {
            return Err("injected config write failure".to_string());
        }
        self.write_config_raw(config).await?;
        self.lifecycle
            .faults
            .config_write_count
            .fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 写盘入口（含 normalize）；补偿路径直接调用，不计入业务写计数、不注入故障。
    async fn write_config_raw(&self, config: ProxyConfigFile) -> Result<(), String> {
        // write_config 内部走 normalize / unique binding 硬校验。
        token_proxy_config::write_config(self.paths.as_ref(), config).await
    }

    async fn apply_saved_checked(&self) -> Result<(), String> {
        if self.lifecycle.faults.fail_apply.load(Ordering::SeqCst) {
            self.lifecycle
                .faults
                .apply_count
                .fetch_add(1, Ordering::SeqCst);
            return Err("injected apply failure".to_string());
        }
        self.apply_saved_raw().await?;
        self.lifecycle
            .faults
            .apply_count
            .fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn apply_saved_raw(&self) -> Result<(), String> {
        let result = self.proxy.apply_saved_config(&self.proxy_context).await;
        // 不吞 apply_error：视为失败，由调用方补偿。
        if let Some(error) = result.apply_error {
            return Err(error);
        }
        Ok(())
    }

    async fn rollback_config_and_runtime(
        &self,
        old_config: &ProxyConfigFile,
    ) -> Result<(), String> {
        // 补偿路径绕过 fault-injection，避免注入失败拖死回滚本身。
        if let Err(err) = self.write_config_raw(old_config.clone()).await {
            return Err(format!("rollback config failed: {err}"));
        }
        if let Err(err) = self.apply_saved_raw().await {
            return Err(format!("rollback apply failed: {err}"));
        }
        tracing::info!("rolled back proxy config and re-applied previous runtime");
        Ok(())
    }

    async fn snapshot_credential(
        &self,
        reference: &AccountUpstreamRef,
    ) -> Result<CredentialSnapshot, String> {
        match reference.provider {
            AccountProvider::Kiro => Ok(CredentialSnapshot::Kiro(
                self.kiro_accounts
                    .snapshot_account_record(&reference.account_id)
                    .await?,
            )),
            AccountProvider::Codex => Ok(CredentialSnapshot::Codex(
                self.codex_accounts
                    .snapshot_account_record(&reference.account_id)
                    .await?,
            )),
            AccountProvider::Xai => Ok(CredentialSnapshot::Xai(
                self.xai_accounts
                    .snapshot_account_record(&reference.account_id)
                    .await?,
            )),
        }
    }

    async fn delete_credential(&self, reference: &AccountUpstreamRef) -> Result<(), String> {
        tracing::info!(
            provider = reference.provider.as_str(),
            // account_id 是引用标识，非 token；仍避免与 secret 字段并排打印。
            account_id = %reference.account_id,
            "cascading delete account credential"
        );
        match reference.provider {
            AccountProvider::Kiro => {
                self.kiro_accounts
                    .delete_account(&reference.account_id)
                    .await
            }
            AccountProvider::Codex => {
                self.codex_accounts
                    .delete_account(&reference.account_id)
                    .await
            }
            AccountProvider::Xai => {
                self.xai_accounts
                    .delete_account(&reference.account_id)
                    .await
            }
        }
    }

    async fn restore_credential_backups(
        &self,
        backups: &[&RemovedCredentialBackup],
    ) -> Result<(), String> {
        // 逆序恢复，贴近删除顺序的补偿。
        for backup in backups.iter().rev() {
            match &backup.snapshot {
                CredentialSnapshot::Kiro(record) => {
                    self.kiro_accounts
                        .restore_account_record(&backup.reference.account_id, record.clone())
                        .await?;
                }
                CredentialSnapshot::Codex(record) => {
                    self.codex_accounts
                        .restore_account_record(&backup.reference.account_id, record.clone())
                        .await?;
                }
                CredentialSnapshot::Xai(record) => {
                    self.xai_accounts
                        .restore_account_record(&backup.reference.account_id, record.clone())
                        .await?;
                }
            }
        }
        tracing::info!(
            restored = backups.len(),
            "restored account credentials after orchestration failure"
        );
        Ok(())
    }
}

fn join_errors(primary: String, secondary: String) -> String {
    format!("{primary}; rollback: {secondary}")
}

fn join_optional(primary: String, secondary: Option<String>) -> String {
    match secondary {
        Some(sec) if !sec.is_empty() => join_errors(primary, sec),
        _ => primary,
    }
}

/// 新旧 config 语义相等（JSON 规范化比较），用于 save no-op。
fn configs_semantically_equal(left: &ProxyConfigFile, right: &ProxyConfigFile) -> bool {
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(l), Ok(r)) => l == r,
        _ => false,
    }
}

// 供测试断言 config 中 account binding 数量。
#[cfg(test)]
pub(crate) fn count_account_bindings(config: &ProxyConfigFile) -> usize {
    config
        .upstreams
        .iter()
        .filter(|upstream| {
            matches!(
                upstream.credential,
                token_proxy_config::UpstreamCredential::Account { .. }
            )
        })
        .count()
}

#[cfg(test)]
mod tests;
