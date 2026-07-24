use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, MutexGuard, RwLock};

#[cfg(any(test, feature = "test-support"))]
use tokio::sync::Notify;

use crate::persistence as provider_accounts;
use token_proxy_account_store::app_proxy::AppProxyState;
use token_proxy_account_store::oauth_util::{
    decode_jwt_payload, expires_at_from_seconds, now_rfc3339, sanitize_id_part,
};
use token_proxy_account_store::paths::TokenProxyPaths;

use super::oauth::{validate_oauth_endpoint, XaiOAuthClient, XaiTokenResponse};
use super::quota;
use super::types::{XaiAccountStatus, XaiAccountSummary, XaiQuotaCache, XaiTokenRecord};

const TOKEN_REFRESH_WINDOW: Duration = Duration::minutes(5);
const TOKEN_REFRESH_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(50);
const TOKEN_REFRESH_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);
const QUOTA_PERSIST_INTERVAL: Duration = Duration::seconds(30);

/// xAI 账户存储把 SQLite 作为真值源，并以小型内存快照服务高频调度读取。
pub struct XaiAccountStore {
    paths: TokenProxyPaths,
    cache: RwLock<HashMap<String, XaiTokenRecord>>,
    app_proxy: AppProxyState,
    token_refreshing: StdMutex<HashSet<String>>,
    quota_refreshing: StdMutex<HashSet<String>>,
    quota_persisting: StdMutex<HashSet<String>>,
    mutation_locks: StdMutex<HashMap<String, Weak<Mutex<()>>>>,
    account_index_mutation: Mutex<()>,
    cache_sync: Mutex<()>,
    /// Provider 级 mutation gate：snapshot/restore 与 refresh/save/delete 共享，
    /// 防止全量补偿覆盖并发 token/quota 写。锁序：provider → index → account → cache_sync。
    provider_mutation: Mutex<()>,
    /// 测试探针：即将 acquire provider_mutation 时通知（不含 secret）。
    #[cfg(any(test, feature = "test-support"))]
    gate_probe: StdMutex<Option<Arc<ProviderGateProbe>>>,
    #[cfg(test)]
    persistence_test_hook: StdMutex<Option<Arc<PersistenceTestHook>>>,
}

/// provider gate 测试探针：about_to_lock 在阻塞前，acquired 在拿到锁后。
#[cfg(any(test, feature = "test-support"))]
pub struct ProviderGateProbe {
    pub about_to_lock: Notify,
    pub acquired: Notify,
}

/// Provider import/restore 事务会话：持有 `provider_mutation` 直至 drop。
/// 不暴露裸 MutexGuard；事务内只走 unlocked 路径，避免 Tokio Mutex 自重入。
pub struct XaiProviderMutation<'a> {
    store: &'a XaiAccountStore,
    _gate: MutexGuard<'a, ()>,
}

impl Drop for XaiProviderMutation<'_> {
    fn drop(&mut self) {
        tracing::debug!("xai provider mutation session end");
    }
}

impl XaiProviderMutation<'_> {
    /// 已持 gate：全量快照（含 secret，仅内存短时持有，禁止日志）。
    pub async fn snapshot_all_records(&self) -> Result<HashMap<String, XaiTokenRecord>, String> {
        self.store.snapshot_all_records_unlocked().await
    }

    /// 已持 gate：SQLite 事务全量恢复后替换 cache。
    pub async fn restore_all_records(
        &self,
        snapshot: HashMap<String, XaiTokenRecord>,
    ) -> Result<(), String> {
        self.store.restore_all_records_unlocked(snapshot).await
    }

    /// 已持 gate：文件/目录导入。
    pub async fn import_file(&self, path: PathBuf) -> Result<Vec<XaiAccountSummary>, String> {
        self.store.import_file_unlocked(path).await
    }

    /// 已持 gate：文本导入。
    pub async fn import_text(&self, contents: &str) -> Result<Vec<XaiAccountSummary>, String> {
        self.store.import_text_unlocked(contents).await
    }

    /// 已持 gate：refresh token 导入（含网络 refresh）。
    pub async fn import_refresh_tokens(
        &self,
        contents: &str,
    ) -> Result<Vec<XaiAccountSummary>, String> {
        self.store.import_refresh_tokens_unlocked(contents).await
    }
}

#[cfg(test)]
#[derive(Default)]
struct PersistenceTestHook {
    committed: Notify,
    resume: Notify,
}

struct TokenRefreshPermit<'a> {
    refreshing: &'a StdMutex<HashSet<String>>,
    account_id: String,
}

impl Drop for TokenRefreshPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut refreshing) = self.refreshing.lock() {
            refreshing.remove(&self.account_id);
        }
    }
}

struct AccountOperationPermit<'a> {
    active: &'a StdMutex<HashSet<String>>,
    account_id: String,
}

impl Drop for AccountOperationPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.account_id);
        }
    }
}

impl XaiAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        Ok(Self {
            paths: paths.clone(),
            cache: RwLock::new(HashMap::new()),
            app_proxy,
            token_refreshing: StdMutex::new(HashSet::new()),
            quota_refreshing: StdMutex::new(HashSet::new()),
            quota_persisting: StdMutex::new(HashSet::new()),
            mutation_locks: StdMutex::new(HashMap::new()),
            account_index_mutation: Mutex::new(()),
            cache_sync: Mutex::new(()),
            provider_mutation: Mutex::new(()),
            #[cfg(any(test, feature = "test-support"))]
            gate_probe: StdMutex::new(None),
            #[cfg(test)]
            persistence_test_hook: StdMutex::new(None),
        })
    }

    /// 开启 provider mutation 事务；session 存活期间独占 provider_mutation。
    pub async fn begin_provider_mutation(&self) -> XaiProviderMutation<'_> {
        tracing::debug!("xai provider mutation session begin");
        let _gate = self.acquire_provider_mutation().await;
        XaiProviderMutation { store: self, _gate }
    }

    /// 统一 acquire：外部写路径自动取 gate；测试探针在阻塞前/获锁后通知。
    async fn acquire_provider_mutation(&self) -> MutexGuard<'_, ()> {
        #[cfg(any(test, feature = "test-support"))]
        self.notify_about_to_acquire_provider_gate();
        let guard = self.provider_mutation.lock().await;
        #[cfg(any(test, feature = "test-support"))]
        self.notify_provider_gate_acquired();
        guard
    }

    #[cfg(any(test, feature = "test-support"))]
    fn notify_about_to_acquire_provider_gate(&self) {
        if let Ok(guard) = self.gate_probe.lock() {
            if let Some(probe) = guard.as_ref() {
                probe.about_to_lock.notify_one();
            }
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn notify_provider_gate_acquired(&self) {
        if let Ok(guard) = self.gate_probe.lock() {
            if let Some(probe) = guard.as_ref() {
                probe.acquired.notify_one();
            }
        }
    }

    /// 安装 gate 探针；返回 Arc 供测试 wait about_to_lock / acquired。
    #[cfg(any(test, feature = "test-support"))]
    pub fn install_provider_gate_probe(&self) -> Arc<ProviderGateProbe> {
        let probe = Arc::new(ProviderGateProbe {
            about_to_lock: Notify::new(),
            acquired: Notify::new(),
        });
        *self.gate_probe.lock().expect("xai gate probe lock") = Some(Arc::clone(&probe));
        probe
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn clear_provider_gate_probe(&self) {
        *self.gate_probe.lock().expect("xai gate probe lock") = None;
    }

    pub async fn list_accounts(&self) -> Result<Vec<XaiAccountSummary>, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let mut items = cache
            .iter()
            .map(|(account_id, record)| account_summary(account_id.clone(), record))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub async fn import_file(&self, path: PathBuf) -> Result<Vec<XaiAccountSummary>, String> {
        let _provider = self.acquire_provider_mutation().await;
        self.import_file_unlocked(path).await
    }

    pub async fn import_text(&self, contents: &str) -> Result<Vec<XaiAccountSummary>, String> {
        let _provider = self.acquire_provider_mutation().await;
        self.import_text_unlocked(contents).await
    }

    pub async fn import_refresh_tokens(
        &self,
        contents: &str,
    ) -> Result<Vec<XaiAccountSummary>, String> {
        let _provider = self.acquire_provider_mutation().await;
        self.import_refresh_tokens_unlocked(contents).await
    }

    pub async fn get_account_record(&self, account_id: &str) -> Result<XaiTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub async fn refresh_account(&self, account_id: &str) -> Result<(), String> {
        let record = self.load_account(account_id).await?;
        if record.refresh_token.trim().is_empty() {
            return Err("xAI account has no refresh token. Please sign in again.".to_string());
        }
        self.refresh_record_guarded(account_id, record).await?;
        Ok(())
    }

    pub async fn refresh_due_accounts(&self) -> Result<Vec<String>, String> {
        self.refresh_cache().await?;
        let account_ids = {
            let cache = self.cache.read().await;
            sorted_account_ids(&cache)
        };
        let mut refreshed = Vec::new();
        let mut last_error = None;
        for account_id in account_ids {
            let record = self.load_account(&account_id).await?;
            if !record_can_auto_refresh(&record) || !record_needs_refresh(&record) {
                continue;
            }
            match self.refresh_record_guarded(&account_id, record).await {
                Ok(_) => refreshed.push(account_id),
                Err(error) => {
                    tracing::warn!(account_id, error = %error, "xai due account refresh failed");
                    last_error = Some(error);
                }
            }
        }
        if refreshed.is_empty() {
            if let Some(error) = last_error {
                return Err(error);
            }
        }
        Ok(refreshed)
    }

    pub async fn refresh_quota_cache(
        &self,
        account_ids: Option<&[String]>,
    ) -> Result<Vec<String>, String> {
        let targets = self.resolve_targets(account_ids).await?;
        let mut refreshed = Vec::new();
        for account_id in targets {
            match self.refresh_quota_cache_guarded(&account_id).await {
                Ok(_) => refreshed.push(account_id),
                Err(error) => {
                    tracing::warn!(account_id, error = %error, "xai quota refresh failed")
                }
            }
        }
        Ok(refreshed)
    }

    /// 手动刷新单个账户，保留真实错误供 UI 展示；同账户已有刷新时避免重复请求。
    pub async fn refresh_quota_cache_now(&self, account_id: &str) -> Result<(), String> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err("xAI account id is required.".to_string());
        }
        self.refresh_quota_cache_guarded(account_id)
            .await
            .map(|_| ())
    }

    /// 所有主动 quota 网络入口共用同一账户 singleflight，重复请求直接读取当前快照。
    pub(crate) async fn refresh_quota_cache_guarded(
        &self,
        account_id: &str,
    ) -> Result<XaiQuotaCache, String> {
        let Some(_permit) = start_account_operation(
            &self.quota_refreshing,
            account_id,
            "xAI quota refresh state is unavailable.",
        )?
        else {
            tracing::debug!(account_id, "xai quota refresh already in progress");
            return self
                .load_account(account_id)
                .await
                .map(|record| record.quota);
        };
        quota::refresh_quota_cache(self, account_id).await
    }

    /// 被动 quota header 快照在请求结束后异步调用，并按账户限频落 SQLite。
    pub async fn record_quota_headers(
        &self,
        account_id: &str,
        headers: &HeaderMap,
        status: u16,
    ) -> Result<(), String> {
        let record = self.load_account(account_id).await?;
        if quota::observe_quota_headers(&record.quota, headers, status).is_none() {
            return Ok(());
        }
        if !quota_persist_is_due(record.quota.checked_at.as_deref()) {
            return Ok(());
        }
        let _permit = {
            let mut persisting = self
                .quota_persisting
                .lock()
                .map_err(|_| "xAI quota persistence state is unavailable.".to_string())?;
            if !persisting.insert(account_id.to_string()) {
                return Ok(());
            }
            AccountOperationPermit {
                active: &self.quota_persisting,
                account_id: account_id.to_string(),
            }
        };
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _mutation_guard = mutation.lock().await;
        let mut latest = self.load_account(account_id).await?;
        if !quota_persist_is_due(latest.quota.checked_at.as_deref()) {
            return Ok(());
        }
        let Some(next_quota) = quota::observe_quota_headers(&latest.quota, headers, status) else {
            return Ok(());
        };
        latest.quota = next_quota;
        self.save_record_locked(account_id.to_string(), latest)
            .await
            .map(|_| ())
    }

    pub async fn set_auto_refresh(
        &self,
        account_id: &str,
        enabled: bool,
    ) -> Result<XaiAccountSummary, String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _guard = mutation.lock().await;
        let mut record = self.load_account(account_id).await?;
        record.auto_refresh_enabled = enabled;
        self.save_record_locked(account_id.to_string(), record)
            .await
    }

    /// 带 provider/account gate 的完整写路径；test-support 与未来 crate 内写入口复用。
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: XaiTokenRecord,
    ) -> Result<XaiAccountSummary, String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(&account_id)?;
        let _guard = mutation.lock().await;
        self.save_record_locked(account_id, record).await
    }

    /// Seeds an exact record for cross-crate integration tests.
    #[cfg(feature = "test-support")]
    pub async fn save_record_for_test(
        &self,
        account_id: String,
        record: XaiTokenRecord,
    ) -> Result<XaiAccountSummary, String> {
        self.save_record(account_id, record).await
    }

    async fn save_record_locked(
        &self,
        account_id: String,
        record: XaiTokenRecord,
    ) -> Result<XaiAccountSummary, String> {
        let _cache_guard = self.cache_sync.lock().await;
        // 先制造 cache miss；即使数据库 await 在提交后被取消，也不会继续返回旧快照。
        let invalidated = self.cache.write().await.remove(&account_id).is_some();
        tracing::debug!(
            account_id,
            invalidated,
            "xai account cache invalidated before database upsert"
        );
        provider_accounts::upsert_xai_account(&self.paths, &account_id, &record).await?;
        #[cfg(test)]
        self.pause_after_persistence_for_test().await;
        self.cache
            .write()
            .await
            .insert(account_id.clone(), record.clone());
        Ok(account_summary(account_id, &record))
    }

    pub(crate) async fn persist_quota_cache(
        &self,
        account_id: &str,
        quota: XaiQuotaCache,
    ) -> Result<XaiTokenRecord, String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _guard = mutation.lock().await;
        let mut latest = self.load_account(account_id).await?;
        latest.quota = quota::merge_quota_cache(&latest.quota, quota);
        self.save_record_locked(account_id.to_string(), latest.clone())
            .await?;
        Ok(latest)
    }

    /// 自动取 gate 的新建/覆盖入口；事务内请用 unlocked。
    #[allow(dead_code)]
    pub(crate) async fn save_new_account(
        &self,
        record: XaiTokenRecord,
    ) -> Result<XaiAccountSummary, String> {
        // 身份匹配、ID 分配和首次保存必须原子化，避免并发导入复用同一空闲 ID。
        let _provider = self.acquire_provider_mutation().await;
        self.save_new_account_unlocked(record).await
    }

    /// 调用方必须已持 `provider_mutation`；仍取 index/account 锁。
    async fn save_new_account_unlocked(
        &self,
        mut record: XaiTokenRecord,
    ) -> Result<XaiAccountSummary, String> {
        let _index_guard = self.account_index_mutation.lock().await;
        fill_record_identity(&mut record);
        if let Some((account_id, _)) = self.find_existing_account(&record).await? {
            let mutation = self.account_mutation_lock(&account_id)?;
            let _guard = mutation.lock().await;
            let existing = self.load_account(&account_id).await?;
            record.auto_refresh_enabled = existing.auto_refresh_enabled;
            // 成功重导入会修复 Expired/Invalid 健康状态。
            record.status = XaiAccountStatus::Active;
            record.quota = existing.quota;
            return self.save_record_locked(account_id, record).await;
        }
        let id_source = record
            .email
            .as_deref()
            .or(record.subject.as_deref())
            .unwrap_or_default();
        let mut id_part = sanitize_id_part(id_source);
        if id_part.is_empty() {
            id_part = OffsetDateTime::now_utc().unix_timestamp().to_string();
        }
        let account_id = self.unique_account_id(&id_part).await?;
        // 已持 provider gate：直接 account lock + save_record_locked，避免重入。
        let mutation = self.account_mutation_lock(&account_id)?;
        let _guard = mutation.lock().await;
        self.save_record_locked(account_id, record).await
    }

    pub async fn delete_account(&self, account_id: &str) -> Result<(), String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _guard = mutation.lock().await;
        let _cache_guard = self.cache_sync.lock().await;
        // 删除前先失效快照；取消时由下一次 load_account 按 SQLite 实际结果恢复。
        let invalidated = self.cache.write().await.remove(account_id).is_some();
        tracing::debug!(
            account_id,
            invalidated,
            "xai account cache invalidated before database delete"
        );
        provider_accounts::delete_account(&self.paths, account_id).await?;
        #[cfg(test)]
        self.pause_after_persistence_for_test().await;
        Ok(())
    }

    /// 编排层快照：导出全部凭据（含 secret，仅内存短时持有，禁止日志）。
    pub async fn snapshot_all_records(&self) -> Result<HashMap<String, XaiTokenRecord>, String> {
        let _provider = self.acquire_provider_mutation().await;
        self.snapshot_all_records_unlocked().await
    }

    /// 编排层恢复：单 SQLite 事务替换该 provider 全量快照，提交后再替换 cache。
    pub async fn restore_all_records(
        &self,
        snapshot: HashMap<String, XaiTokenRecord>,
    ) -> Result<(), String> {
        // provider gate 阻止并发 save/refresh 在 restore 中途写回。
        let _provider = self.acquire_provider_mutation().await;
        self.restore_all_records_unlocked(snapshot).await
    }

    /// 单账户快照；不存在返回 None。
    pub async fn snapshot_account_record(
        &self,
        account_id: &str,
    ) -> Result<Option<XaiTokenRecord>, String> {
        let _provider = self.acquire_provider_mutation().await;
        self.refresh_cache_unlocked().await?;
        Ok(self.cache.read().await.get(account_id).cloned())
    }

    /// 单账户原子恢复；`None` 表示删除。DB 提交成功后再改 cache。
    pub async fn restore_account_record(
        &self,
        account_id: &str,
        record: Option<XaiTokenRecord>,
    ) -> Result<(), String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _guard = mutation.lock().await;
        let _cache_guard = self.cache_sync.lock().await;
        provider_accounts::replace_xai_account(&self.paths, account_id, record.as_ref()).await?;
        match record {
            Some(record) => {
                self.cache
                    .write()
                    .await
                    .insert(account_id.to_string(), record);
            }
            None => {
                self.cache.write().await.remove(account_id);
            }
        }
        tracing::debug!(account_id, "xai account record restored for orchestration");
        Ok(())
    }

    /// 登录编排提交：持久化 credential，返回 summary 与覆盖前旧 record。
    pub async fn commit_login_record(
        &self,
        mut record: XaiTokenRecord,
    ) -> Result<(XaiAccountSummary, Option<XaiTokenRecord>), String> {
        // 身份填充后再 snapshot 旧 record，避免 re-login 覆盖后无法恢复。
        // 整段持 provider gate，避免与 restore/refresh 交错。
        let _provider = self.acquire_provider_mutation().await;
        fill_record_identity(&mut record);
        let previous = self
            .find_existing_account(&record)
            .await?
            .map(|(_, old)| old);
        let summary = self.save_new_account_unlocked(record).await?;
        Ok((summary, previous))
    }

    #[cfg(test)]
    fn install_persistence_test_hook(&self) -> Arc<PersistenceTestHook> {
        let hook = Arc::new(PersistenceTestHook::default());
        *self
            .persistence_test_hook
            .lock()
            .expect("xai persistence test hook lock") = Some(Arc::clone(&hook));
        hook
    }

    #[cfg(test)]
    async fn pause_after_persistence_for_test(&self) {
        let hook = self
            .persistence_test_hook
            .lock()
            .expect("xai persistence test hook lock")
            .clone();
        if let Some(hook) = hook {
            hook.committed.notify_one();
            hook.resume.notified().await;
        }
    }

    /// 解析 Upstream 固定引用的账户凭据。无 pool/unpinned 选号。
    pub async fn resolve_pinned_account_record(
        &self,
        account_id: &str,
    ) -> Result<(String, XaiTokenRecord), String> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err("xAI account_id is required for account credential upstream.".to_string());
        }
        let record = self.get_account_record(account_id).await?;
        if !record.is_usable() {
            tracing::warn!(
                account_id,
                status = ?record.effective_status(),
                "pinned xai account is not usable"
            );
            return Err(format!(
                "xAI account is {}: {account_id}",
                match record.effective_status() {
                    XaiAccountStatus::Active => "active",
                    XaiAccountStatus::Expired => "expired",
                    XaiAccountStatus::Invalid => "invalid",
                }
            ));
        }
        tracing::debug!(account_id, "resolved pinned xai account credential");
        Ok((account_id.to_string(), record))
    }

    pub async fn app_proxy_url(&self) -> Option<String> {
        self.app_proxy.read().await.clone()
    }

    pub(crate) async fn effective_proxy_url(&self, _account_proxy: Option<&str>) -> Option<String> {
        // 账户不再持有 proxy_url；OAuth/HTTP 仅使用 app 级 proxy。
        self.app_proxy_url().await
    }

    pub(crate) async fn load_account(&self, account_id: &str) -> Result<XaiTokenRecord, String> {
        if let Some(record) = self.cache.read().await.get(account_id).cloned() {
            return Ok(record);
        }
        self.refresh_cache().await?;
        self.cache
            .read()
            .await
            .get(account_id)
            .cloned()
            .ok_or_else(|| format!("xAI account not found: {account_id}"))
    }

    async fn refresh_cache(&self) -> Result<(), String> {
        // 不持 provider_mutation：load_account 常在已持 gate 的写路径中调用，避免重入死锁。
        // 与 restore/save 的互斥靠 cache_sync；全量 restore 另持 provider 挡并发写。
        self.refresh_cache_unlocked().await
    }

    async fn refresh_cache_unlocked(&self) -> Result<(), String> {
        let _guard = self.cache_sync.lock().await;
        let snapshot = provider_accounts::list_xai_records(&self.paths).await?;
        *self.cache.write().await = snapshot;
        Ok(())
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn snapshot_all_records_unlocked(
        &self,
    ) -> Result<HashMap<String, XaiTokenRecord>, String> {
        self.refresh_cache_unlocked().await?;
        Ok(self.cache.read().await.clone())
    }

    /// 调用方必须已持 `provider_mutation`；仍取 index/cache_sync。
    async fn restore_all_records_unlocked(
        &self,
        snapshot: HashMap<String, XaiTokenRecord>,
    ) -> Result<(), String> {
        let _index_guard = self.account_index_mutation.lock().await;
        let _cache_guard = self.cache_sync.lock().await;
        provider_accounts::replace_all_xai_records(&self.paths, &snapshot).await?;
        *self.cache.write().await = snapshot;
        tracing::debug!("xai account store restored from orchestration snapshot");
        Ok(())
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_file_unlocked(&self, path: PathBuf) -> Result<Vec<XaiAccountSummary>, String> {
        if path.as_os_str().is_empty() {
            return Err("Import path is required.".to_string());
        }
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err("Selected import path not found.".to_string());
        }
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|error| format!("Failed to read import path metadata: {error}"))?;
        let files = if metadata.is_dir() {
            collect_json_files(&path).await?
        } else {
            vec![path]
        };
        let mut imported = Vec::new();
        let mut first_error = None;
        for file in files {
            let contents = match tokio::fs::read_to_string(&file).await {
                Ok(contents) => contents,
                Err(error) if metadata.is_dir() => {
                    tracing::debug!(path = %file.display(), error = %error, "skip unreadable xai import file");
                    first_error.get_or_insert_with(|| {
                        format!("Failed to read xAI JSON file {}: {error}", file.display())
                    });
                    continue;
                }
                Err(error) => return Err(format!("Failed to read xAI JSON file: {error}")),
            };
            let records = match parse_import_records(&contents) {
                Ok(records) => records,
                Err(error) if metadata.is_dir() => {
                    tracing::debug!(path = %file.display(), error = %error, "skip non-xai import file");
                    first_error.get_or_insert(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            for record in records {
                if metadata.is_dir() {
                    match self.prepare_import_record(record).await {
                        Ok(record) => match self.save_new_account_unlocked(record).await {
                            Ok(summary) => imported.push(summary),
                            Err(error) => {
                                first_error.get_or_insert(error.clone());
                                tracing::warn!(
                                    path = %file.display(),
                                    error = %error,
                                    "xai account import save failed"
                                );
                            }
                        },
                        Err(error) => {
                            first_error.get_or_insert(error.clone());
                            tracing::warn!(
                                path = %file.display(),
                                error = %error,
                                "xai account import record rejected"
                            );
                        }
                    }
                    continue;
                }
                let record = self.prepare_import_record(record).await?;
                imported.push(self.save_new_account_unlocked(record).await?);
            }
        }
        if imported.is_empty() {
            return Err(first_error.unwrap_or_else(|| {
                if metadata.is_dir() {
                    "No valid xAI OAuth accounts found in selected directory.".to_string()
                } else {
                    "No valid xAI OAuth accounts found in JSON file.".to_string()
                }
            }));
        }
        tracing::info!(
            imported = imported.len(),
            "xai account file import finished"
        );
        Ok(imported)
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_text_unlocked(&self, contents: &str) -> Result<Vec<XaiAccountSummary>, String> {
        let records = parse_import_records(contents)?;
        let mut imported = Vec::new();
        let mut errors = Vec::new();
        for record in records {
            match self.prepare_import_record(record).await {
                Ok(record) => match self.save_new_account_unlocked(record).await {
                    Ok(summary) => imported.push(summary),
                    Err(error) => errors.push(error),
                },
                Err(error) => errors.push(error),
            }
        }
        tracing::info!(
            imported = imported.len(),
            failed = errors.len(),
            "xai account text import finished"
        );
        if imported.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .unwrap_or_else(|| "No valid xAI OAuth accounts found in input.".to_string()));
        }
        Ok(imported)
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_refresh_tokens_unlocked(
        &self,
        contents: &str,
    ) -> Result<Vec<XaiAccountSummary>, String> {
        let refresh_tokens = parse_refresh_token_lines(contents);
        if refresh_tokens.is_empty() {
            return Err("Refresh token is required.".to_string());
        }
        let mut imported = Vec::new();
        let mut errors = Vec::new();
        for refresh_token in refresh_tokens {
            match self.import_refresh_token_unlocked(&refresh_token).await {
                Ok(summary) => imported.push(summary),
                Err(error) => errors.push(error),
            }
        }
        tracing::info!(
            imported = imported.len(),
            failed = errors.len(),
            "xai refresh token import finished"
        );
        if imported.is_empty() {
            return Err(errors.into_iter().next().unwrap_or_else(|| {
                "No valid xAI accounts found in refresh token input.".to_string()
            }));
        }
        Ok(imported)
    }

    async fn prepare_import_record(
        &self,
        mut record: XaiTokenRecord,
    ) -> Result<XaiTokenRecord, String> {
        if let Some(endpoint) = record.token_endpoint.as_deref() {
            record.token_endpoint = Some(
                validate_oauth_endpoint(endpoint, "token_endpoint")
                    .map_err(|error| error.to_string())?,
            );
        }
        fill_record_identity(&mut record);
        if record_needs_refresh(&record) && !record.refresh_token.trim().is_empty() {
            return self.refresh_import_record(record).await;
        }
        if record_needs_refresh(&record) {
            return Err(
                "Expired xAI access-token-only credential cannot be imported without refresh_token."
                    .to_string(),
            );
        }
        if record.access_token.trim().is_empty() {
            return Err("xAI OAuth import requires access_token or refresh_token.".to_string());
        }
        Ok(record)
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_refresh_token_unlocked(
        &self,
        refresh_token: &str,
    ) -> Result<XaiAccountSummary, String> {
        let proxy_url = self.app_proxy_url().await;
        let client = XaiOAuthClient::new(proxy_url.as_deref())?;
        let response = client
            .refresh_token(refresh_token, None)
            .await
            .map_err(|error| error.to_string())?;
        let record = record_from_token_response(response, refresh_token, None);
        self.save_new_account_unlocked(record).await
    }

    async fn refresh_import_record(
        &self,
        record: XaiTokenRecord,
    ) -> Result<XaiTokenRecord, String> {
        let proxy_url = self.effective_proxy_url(None).await;
        let client = XaiOAuthClient::new(proxy_url.as_deref())?;
        let response = client
            .refresh_token(&record.refresh_token, record.token_endpoint.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        Ok(merge_token_response(record, response))
    }

    async fn refresh_if_needed(
        &self,
        account_id: &str,
        record: XaiTokenRecord,
    ) -> Result<XaiTokenRecord, String> {
        if matches!(record.status, XaiAccountStatus::Invalid)
            || !record_needs_refresh(&record)
            || !record.auto_refresh_enabled
            || record.refresh_token.trim().is_empty()
        {
            return Ok(record);
        }
        self.refresh_record_guarded(account_id, record).await
    }

    async fn refresh_record_guarded(
        &self,
        account_id: &str,
        record: XaiTokenRecord,
    ) -> Result<XaiTokenRecord, String> {
        let Some(_permit) = self.start_token_refresh(account_id) else {
            tracing::debug!(
                account_id,
                "xai account refresh already in progress; waiting"
            );
            return self.wait_for_token_refresh(account_id, &record).await;
        };
        self.refresh_record_inner(account_id, record).await
    }

    async fn refresh_record_inner(
        &self,
        account_id: &str,
        record: XaiTokenRecord,
    ) -> Result<XaiTokenRecord, String> {
        let proxy_url = self.effective_proxy_url(None).await;
        let client = XaiOAuthClient::new(proxy_url.as_deref())?;
        tracing::debug!(account_id, "xai account refresh start");
        let response = match client
            .refresh_token(&record.refresh_token, record.token_endpoint.as_deref())
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if error.is_invalid_grant() {
                    let _ = self
                        .mark_invalid_if_credentials_match(account_id, &record)
                        .await;
                }
                return Err(error.to_string());
            }
        };
        let refreshed = self
            .persist_token_response(account_id, &record, response)
            .await?;
        tracing::info!(account_id, "xai account refresh completed");
        Ok(refreshed)
    }

    /// 网络刷新完成后重新读取最新记录，只合并 token 字段，保留并发状态与设置更新。
    async fn persist_token_response(
        &self,
        account_id: &str,
        attempted: &XaiTokenRecord,
        response: XaiTokenResponse,
    ) -> Result<XaiTokenRecord, String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _guard = mutation.lock().await;
        let latest = self.load_account(account_id).await?;
        if credential_basis_changed(attempted, &latest) {
            tracing::info!(
                account_id,
                "xai account credentials changed during refresh; discarding stale token response"
            );
            return Ok(latest);
        }
        let refreshed = merge_token_response(latest, response);
        self.save_record_locked(account_id.to_string(), refreshed.clone())
            .await?;
        Ok(refreshed)
    }

    async fn mark_invalid_if_credentials_match(
        &self,
        account_id: &str,
        attempted: &XaiTokenRecord,
    ) -> Result<(), String> {
        let _provider = self.acquire_provider_mutation().await;
        let mutation = self.account_mutation_lock(account_id)?;
        let _guard = mutation.lock().await;
        let mut latest = self.load_account(account_id).await?;
        if credential_basis_changed(attempted, &latest) {
            tracing::info!(
                account_id,
                "xai account credentials changed during refresh; ignoring stale invalid_grant"
            );
            return Ok(());
        }

        latest.status = XaiAccountStatus::Invalid;
        tracing::warn!(
            account_id,
            "xai account marked invalid after refresh rejection"
        );
        self.save_record_locked(account_id.to_string(), latest)
            .await?;
        Ok(())
    }

    fn account_mutation_lock(&self, account_id: &str) -> Result<Arc<Mutex<()>>, String> {
        let mut locks = self
            .mutation_locks
            .lock()
            .map_err(|_| "xAI account mutation state is unavailable.".to_string())?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(account_id).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(account_id.to_string(), Arc::downgrade(&lock));
        Ok(lock)
    }

    fn start_token_refresh(&self, account_id: &str) -> Option<TokenRefreshPermit<'_>> {
        let mut refreshing = self.token_refreshing.lock().ok()?;
        if !refreshing.insert(account_id.to_string()) {
            return None;
        }
        Some(TokenRefreshPermit {
            refreshing: &self.token_refreshing,
            account_id: account_id.to_string(),
        })
    }

    async fn wait_for_token_refresh(
        &self,
        account_id: &str,
        previous: &XaiTokenRecord,
    ) -> Result<XaiTokenRecord, String> {
        let deadline = tokio::time::Instant::now() + TOKEN_REFRESH_WAIT_TIMEOUT;
        loop {
            tokio::time::sleep(TOKEN_REFRESH_WAIT_STEP).await;
            let refreshing = self
                .token_refreshing
                .lock()
                .map(|items| items.contains(account_id))
                .unwrap_or(false);
            let current = self.load_account(account_id).await?;
            if token_record_was_refreshed(previous, &current) {
                return Ok(current);
            }
            if !refreshing {
                return Err("xAI token refresh did not update credentials.".to_string());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Timed out waiting for xAI token refresh.".to_string());
            }
        }
    }

    async fn find_existing_account(
        &self,
        imported: &XaiTokenRecord,
    ) -> Result<Option<(String, XaiTokenRecord)>, String> {
        self.refresh_cache().await?;
        let subject = normalize_identity(imported.subject.as_deref());
        let email = normalize_email(imported.email.as_deref());
        let cache = self.cache.read().await;
        for (account_id, existing) in cache.iter() {
            let same_subject =
                subject.is_some() && normalize_identity(existing.subject.as_deref()) == subject;
            let same_email = email.is_some() && normalize_email(existing.email.as_deref()) == email;
            if same_subject || same_email {
                tracing::debug!(
                    identity = if same_subject { "subject" } else { "email" },
                    "xai import reuses existing local account"
                );
                return Ok(Some((account_id.clone(), existing.clone())));
            }
        }
        Ok(None)
    }

    async fn unique_account_id(&self, id_part: &str) -> Result<String, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let base = format!("xai-{id_part}");
        if !cache.contains_key(&base) {
            return Ok(base);
        }
        for suffix in 2..=10_000 {
            let candidate = format!("{base}-{suffix}");
            if !cache.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err("Unable to allocate a unique xAI account id.".to_string())
    }

    async fn resolve_targets(&self, account_ids: Option<&[String]>) -> Result<Vec<String>, String> {
        if let Some(account_ids) = account_ids {
            return Ok(account_ids
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect());
        }
        Ok(self
            .list_accounts()
            .await?
            .into_iter()
            .map(|account| account.account_id)
            .collect())
    }
}

fn account_summary(account_id: String, record: &XaiTokenRecord) -> XaiAccountSummary {
    XaiAccountSummary {
        account_id,
        email: record.email.clone(),
        expires_at: record.expires_at().map(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| record.expires_at.clone())
        }),
        status: record.effective_status(),
        auto_refresh_enabled: record.auto_refresh_enabled,
    }
}

fn record_from_token_response(
    response: XaiTokenResponse,
    fallback_refresh_token: &str,
    token_endpoint: Option<String>,
) -> XaiTokenRecord {
    let refresh_token = if response.refresh_token.trim().is_empty() {
        fallback_refresh_token.to_string()
    } else {
        response.refresh_token.clone()
    };
    let mut record = XaiTokenRecord {
        access_token: response.access_token,
        refresh_token,
        id_token: response.id_token,
        token_type: if response.token_type.trim().is_empty() {
            "Bearer".to_string()
        } else {
            response.token_type
        },
        expires_at: expires_at_from_seconds(response.expires_in),
        last_refresh: Some(now_rfc3339()),
        email: None,
        subject: None,
        token_endpoint,
        auto_refresh_enabled: true,
        status: XaiAccountStatus::Active,
        quota: XaiQuotaCache::default(),
    };
    fill_record_identity(&mut record);
    record
}

fn merge_token_response(mut record: XaiTokenRecord, response: XaiTokenResponse) -> XaiTokenRecord {
    record.access_token = response.access_token;
    if !response.refresh_token.trim().is_empty() {
        record.refresh_token = response.refresh_token;
    }
    if !response.id_token.trim().is_empty() {
        record.id_token = response.id_token;
    }
    if !response.token_type.trim().is_empty() {
        record.token_type = response.token_type;
    }
    record.expires_at = expires_at_from_seconds(response.expires_in);
    record.last_refresh = Some(now_rfc3339());
    // 成功 refresh 恢复健康态；人工 Disabled 已删除，不再保留双模型。
    record.status = XaiAccountStatus::Active;
    fill_record_identity(&mut record);
    record
}

fn fill_record_identity(record: &mut XaiTokenRecord) {
    let claims =
        decode_jwt_payload(&record.id_token).or_else(|| decode_jwt_payload(&record.access_token));
    let Some(claims) = claims else {
        return;
    };
    if let Some(email) = claims.get("email").and_then(Value::as_str) {
        let email = email.trim();
        if !email.is_empty() {
            record.email = Some(email.to_string());
        }
    }
    if let Some(subject) = claims.get("sub").and_then(Value::as_str) {
        let subject = subject.trim();
        if !subject.is_empty() {
            record.subject = Some(subject.to_string());
        }
    }
}

fn record_needs_refresh(record: &XaiTokenRecord) -> bool {
    record
        .expires_at()
        .is_none_or(|expires_at| OffsetDateTime::now_utc() + TOKEN_REFRESH_WINDOW >= expires_at)
}

fn record_can_auto_refresh(record: &XaiTokenRecord) -> bool {
    matches!(record.status, XaiAccountStatus::Active)
        && record.auto_refresh_enabled
        && !record.refresh_token.trim().is_empty()
}

fn token_record_was_refreshed(previous: &XaiTokenRecord, current: &XaiTokenRecord) -> bool {
    current.access_token != previous.access_token
        || current.refresh_token != previous.refresh_token
        || current.expires_at != previous.expires_at
        || current.last_refresh != previous.last_refresh
}

fn credential_basis_changed(attempted: &XaiTokenRecord, latest: &XaiTokenRecord) -> bool {
    attempted.access_token != latest.access_token
        || attempted.refresh_token != latest.refresh_token
        || attempted.token_endpoint != latest.token_endpoint
}

fn sorted_account_ids(cache: &HashMap<String, XaiTokenRecord>) -> Vec<String> {
    let mut account_ids = cache.keys().cloned().collect::<Vec<_>>();
    account_ids.sort();
    account_ids
}

fn quota_persist_is_due(checked_at: Option<&str>) -> bool {
    let Some(checked_at) = checked_at
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| {
            OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        })
    else {
        return true;
    };
    OffsetDateTime::now_utc() - checked_at >= QUOTA_PERSIST_INTERVAL
}

fn start_account_operation<'a>(
    active: &'a StdMutex<HashSet<String>>,
    account_id: &str,
    unavailable_message: &str,
) -> Result<Option<AccountOperationPermit<'a>>, String> {
    let mut active_accounts = active.lock().map_err(|_| unavailable_message.to_string())?;
    if !active_accounts.insert(account_id.to_string()) {
        return Ok(None);
    }
    Ok(Some(AccountOperationPermit {
        active,
        account_id: account_id.to_string(),
    }))
}

fn normalize_identity(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_email(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

async fn collect_json_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .map_err(|error| format!("Failed to read import directory: {error}"))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| format!("Failed to read import directory entry: {error}"))?
        {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| format!("Failed to inspect import path: {error}"))?;
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("json"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn parse_import_records(contents: &str) -> Result<Vec<XaiTokenRecord>, String> {
    let value = serde_json::from_str::<Value>(contents.trim())
        .map_err(|error| format!("Invalid xAI account JSON: {error}"))?;
    let mut imported = Vec::new();
    collect_import_records(&value, &mut imported)?;
    if imported.is_empty() {
        return Err("No xAI OAuth credential records found.".to_string());
    }
    Ok(imported)
}

fn collect_import_records(value: &Value, output: &mut Vec<XaiTokenRecord>) -> Result<(), String> {
    if let Some(items) = value.as_array() {
        for item in items {
            collect_import_records(item, output)?;
        }
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for key in ["accounts", "items", "auths"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            for item in items {
                collect_import_records(item, output)?;
            }
            return Ok(());
        }
    }
    let imported = serde_json::from_value::<ImportedXaiRecord>(value.clone())
        .map_err(|error| format!("Invalid xAI OAuth credential record: {error}"))?;
    output.push(imported.try_into()?);
    Ok(())
}

fn parse_refresh_token_lines(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Deserialize)]
struct ImportedXaiRecord {
    #[serde(default, rename = "type")]
    provider_type: Option<String>,
    #[serde(default)]
    auth_kind: Option<String>,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default, alias = "expired")]
    expires_at: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    last_refresh: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "sub")]
    subject: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
}

impl TryFrom<ImportedXaiRecord> for XaiTokenRecord {
    type Error = String;

    fn try_from(imported: ImportedXaiRecord) -> Result<Self, Self::Error> {
        if !imported
            .provider_type
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("xai"))
        {
            return Err("Credential type must be xai.".to_string());
        }
        if !imported
            .auth_kind
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("oauth"))
        {
            return Err("xAI credential auth_kind must be oauth.".to_string());
        }
        if imported.access_token.trim().is_empty() && imported.refresh_token.trim().is_empty() {
            return Err("xAI OAuth credential requires access_token or refresh_token.".to_string());
        }
        let expires_at = imported
            .expires_at
            .as_deref()
            .and_then(normalize_expiry)
            .or_else(|| {
                imported.expires_in.and_then(|expires_in| {
                    imported
                        .last_refresh
                        .as_deref()
                        .and_then(|last_refresh| expiry_from_issued_ttl(last_refresh, expires_in))
                })
            })
            .or_else(|| jwt_expiry(&imported.id_token))
            .or_else(|| jwt_expiry(&imported.access_token))
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
        Ok(XaiTokenRecord {
            access_token: imported.access_token.trim().to_string(),
            refresh_token: imported.refresh_token.trim().to_string(),
            id_token: imported.id_token.trim().to_string(),
            token_type: imported.token_type.trim().to_string(),
            expires_at,
            last_refresh: imported.last_refresh,
            email: imported.email,
            subject: imported.subject,
            token_endpoint: imported.token_endpoint,
            auto_refresh_enabled: !imported.refresh_token.trim().is_empty(),
            status: XaiAccountStatus::Active,
            quota: XaiQuotaCache::default(),
        })
    }
}

/// 导入格式里的 expires_in 是签发时 TTL，不能从导入时刻重新起算。
fn expiry_from_issued_ttl(last_refresh: &str, expires_in: i64) -> Option<String> {
    let issued_at = normalize_expiry(last_refresh)?;
    let issued_at =
        OffsetDateTime::parse(&issued_at, &time::format_description::well_known::Rfc3339).ok()?;
    issued_at
        .checked_add(Duration::seconds(expires_in))?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn normalize_expiry(value: &str) -> Option<String> {
    let value = value.trim();
    if OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).is_ok() {
        return Some(value.to_string());
    }
    let timestamp = value.parse::<i64>().ok()?;
    let timestamp = if timestamp > 1_000_000_000_000 {
        timestamp / 1000
    } else {
        timestamp
    };
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn jwt_expiry(token: &str) -> Option<String> {
    let timestamp = decode_jwt_payload(token)?.get("exp")?.as_i64()?;
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::random;

    fn test_store(label: &str) -> (XaiAccountStore, TokenProxyPaths, PathBuf) {
        let data_dir =
            std::env::temp_dir().join(format!("token-proxy-xai-store-{label}-{}", random::<u64>()));
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
        let store = XaiAccountStore::new(&paths, token_proxy_account_store::app_proxy::new_state())
            .expect("xai store");
        (store, paths, data_dir)
    }

    fn test_record(access_token: &str, email: Option<&str>) -> XaiTokenRecord {
        XaiTokenRecord {
            access_token: access_token.to_string(),
            refresh_token: "refresh-token".to_string(),
            id_token: String::new(),
            token_type: "Bearer".to_string(),
            expires_at: "2999-01-01T00:00:00Z".to_string(),
            last_refresh: None,
            email: email.map(str::to_string),
            subject: email.map(|value| format!("subject-{value}")),
            token_endpoint: None,
            auto_refresh_enabled: true,
            status: XaiAccountStatus::Active,
            quota: XaiQuotaCache::default(),
        }
    }

    fn refreshed_token_response() -> XaiTokenResponse {
        XaiTokenResponse {
            access_token: "rotated-access-token".to_string(),
            refresh_token: "rotated-refresh-token".to_string(),
            id_token: String::new(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
        }
    }

    #[test]
    fn cpa_record_requires_xai_oauth_identity() {
        let error = parse_import_records(
            r#"{"type":"openai","auth_kind":"oauth","access_token":"access","expired":"2999-01-01T00:00:00Z"}"#,
        )
        .unwrap_err();
        assert!(error.contains("type must be xai"));
    }

    #[test]
    fn cpa_record_requires_explicit_type_and_auth_kind() {
        let missing_type = parse_import_records(
            r#"{"auth_kind":"oauth","refresh_token":"refresh","expired":"2999-01-01T00:00:00Z"}"#,
        )
        .unwrap_err();
        assert!(missing_type.contains("type must be xai"));

        let missing_auth_kind = parse_import_records(
            r#"{"type":"xai","refresh_token":"refresh","expired":"2999-01-01T00:00:00Z"}"#,
        )
        .unwrap_err();
        assert!(missing_auth_kind.contains("auth_kind must be oauth"));
    }

    #[test]
    fn cpa_record_maps_canonical_fields() {
        let records = parse_import_records(
            r#"{"type":"xai","auth_kind":"oauth","access_token":"access","refresh_token":"refresh","expired":"2999-01-01T00:00:00Z","email":"user@example.com","sub":"subject","token_endpoint":"https://auth.x.ai/oauth2/token"}"#,
        )
        .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].email.as_deref(), Some("user@example.com"));
        assert_eq!(records[0].subject.as_deref(), Some("subject"));
        assert!(records[0].auto_refresh_enabled);
    }

    #[test]
    fn import_accepts_array_of_cpa_records() {
        let records = parse_import_records(
            r#"[{"type":"xai","auth_kind":"oauth","access_token":"one","expired":"2999-01-01T00:00:00Z"},{"type":"xai","auth_kind":"oauth","access_token":"two","expired":"2999-01-01T00:00:00Z"}]"#,
        )
        .unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn expiry_normalizes_unix_milliseconds() {
        assert_eq!(
            normalize_expiry("32503680000000").as_deref(),
            Some("3000-01-01T00:00:00Z")
        );
    }

    #[test]
    fn imported_expires_in_uses_original_issue_time() {
        let records = parse_import_records(
            r#"{"type":"xai","auth_kind":"oauth","access_token":"access","expires_in":3600,"last_refresh":"2000-01-01T00:00:00Z"}"#,
        )
        .expect("parse import record");

        assert_eq!(records[0].expires_at, "2000-01-01T01:00:00Z");
        assert_eq!(
            records[0].last_refresh.as_deref(),
            Some("2000-01-01T00:00:00Z")
        );
    }

    #[test]
    fn imported_expires_in_without_issue_time_does_not_extend_access_token() {
        let records = parse_import_records(
            r#"{"type":"xai","auth_kind":"oauth","access_token":"access","expires_in":3600}"#,
        )
        .expect("parse import record");

        assert_eq!(records[0].expires_at, "1970-01-01T00:00:00Z");
        assert!(records[0].last_refresh.is_none());
    }

    #[test]
    fn successful_token_refresh_restores_non_manual_status() {
        for status in [XaiAccountStatus::Invalid, XaiAccountStatus::Expired] {
            let mut record = test_record("old-access-token", Some("restore@example.com"));
            record.status = status;
            let refreshed = merge_token_response(record, refreshed_token_response());
            assert_eq!(refreshed.status, XaiAccountStatus::Active);
        }
    }

    #[test]
    fn account_operation_permit_releases_account_on_drop() {
        let active = StdMutex::new(HashSet::new());
        let permit = start_account_operation(&active, "xai-a", "unavailable")
            .unwrap()
            .expect("first operation should start");
        assert!(start_account_operation(&active, "xai-a", "unavailable")
            .unwrap()
            .is_none());

        drop(permit);

        assert!(start_account_operation(&active, "xai-a", "unavailable")
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn concurrent_imports_reuse_one_identity_and_account_id() {
        let (store, paths, data_dir) = test_store("concurrent-import");
        let first = test_record("first-access-token", Some("same@example.com"));
        let second = test_record("second-access-token", Some("same@example.com"));

        let (first, second) = tokio::join!(
            store.save_new_account(first),
            store.save_new_account(second)
        );
        let first = first.expect("first import");
        let second = second.expect("second import");
        let records = provider_accounts::list_xai_records(&paths)
            .await
            .expect("persisted xai records");

        assert_eq!(first.account_id, second.account_id);
        assert_eq!(records.len(), 1);
        assert!(records.contains_key(&first.account_id));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn successful_reimport_restores_invalid_to_active() {
        // Phase B: 无人工 Disabled；重导入成功一律恢复 Active。
        let (store, _paths, data_dir) = test_store("reimport-invalid");
        let account_id = "xai-invalid@example.com";
        let mut existing = test_record("old-access-token", Some("invalid@example.com"));
        existing.status = XaiAccountStatus::Invalid;
        store
            .save_record(account_id.to_string(), existing)
            .await
            .expect("seed account");

        let imported = store
            .save_new_account(test_record("new-access-token", Some("invalid@example.com")))
            .await
            .expect("reimport account");

        assert_eq!(imported.status, XaiAccountStatus::Active);
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn invalid_grant_keeps_invalid_status() {
        let (store, _paths, data_dir) = test_store("invalid-grant");
        let account_id = "xai-invalid-grant@example.com";
        let mut record = test_record("access-token", Some("invalid-grant@example.com"));
        record.status = XaiAccountStatus::Invalid;
        store
            .save_record(account_id.to_string(), record.clone())
            .await
            .expect("seed invalid account");

        store
            .mark_invalid_if_credentials_match(account_id, &record)
            .await
            .expect("handle invalid grant");

        let persisted = store.load_account(account_id).await.expect("load account");
        assert_eq!(persisted.status, XaiAccountStatus::Invalid);
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn refresh_follower_rejects_unchanged_still_unexpired_token() {
        let (store, _paths, data_dir) = test_store("refresh-follower-error");
        let account_id = "xai-refresh-follower@example.com";
        let record = test_record("unchanged-access-token", Some("follower@example.com"));
        store
            .save_record(account_id.to_string(), record.clone())
            .await
            .expect("seed account");
        let permit = store
            .start_token_refresh(account_id)
            .expect("start leader refresh");
        drop(permit);

        let error = store
            .wait_for_token_refresh(account_id, &record)
            .await
            .expect_err("unchanged token must not report refresh success");

        assert!(error.contains("did not update credentials"));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn token_and_quota_updates_only_merge_owned_fields() {
        let (store, paths, data_dir) = test_store("field-merge");
        let account_id = "xai-merge@example.com";
        let mut record = test_record("initial-access-token", Some("merge@example.com"));
        record.auto_refresh_enabled = false;
        record.status = XaiAccountStatus::Invalid;
        record.quota = XaiQuotaCache {
            plan_type: Some("old-plan".to_string()),
            quotas: vec![crate::types::XaiQuotaItem {
                name: "xai-requests".to_string(),
                percentage: 80.0,
                used: Some(20.0),
                limit: Some(100.0),
                reset_at: None,
            }],
            checked_at: Some("2026-01-01T00:00:00Z".to_string()),
            ..XaiQuotaCache::default()
        };
        store
            .save_record(account_id.to_string(), record.clone())
            .await
            .expect("seed account");

        let refreshed = store
            .persist_token_response(account_id, &record, refreshed_token_response())
            .await
            .expect("persist token response");
        // 成功 token merge 恢复 Active；auto_refresh / quota 不被 token 响应覆盖。
        assert_eq!(refreshed.status, XaiAccountStatus::Active);
        assert!(!refreshed.auto_refresh_enabled);
        assert_eq!(refreshed.quota.plan_type.as_deref(), Some("old-plan"));

        let quota = XaiQuotaCache {
            plan_type: Some("new-plan".to_string()),
            quotas: vec![crate::types::XaiQuotaItem {
                name: "xai-weekly".to_string(),
                percentage: 42.0,
                used: Some(58.0),
                limit: Some(100.0),
                reset_at: None,
            }],
            error: None,
            checked_at: Some(now_rfc3339()),
        };
        let persisted = store
            .persist_quota_cache(account_id, quota)
            .await
            .expect("persist quota cache");
        assert_eq!(persisted.access_token, "rotated-access-token");
        assert_eq!(persisted.refresh_token, "rotated-refresh-token");
        assert_eq!(persisted.status, XaiAccountStatus::Active);
        assert!(!persisted.auto_refresh_enabled);
        assert_eq!(persisted.quota.plan_type.as_deref(), Some("new-plan"));
        assert_eq!(persisted.quota.quotas.len(), 2);
        assert!(persisted
            .quota
            .quotas
            .iter()
            .any(|item| item.name == "xai-requests" && item.percentage == 80.0));
        assert!(persisted
            .quota
            .quotas
            .iter()
            .any(|item| item.name == "xai-weekly" && item.percentage == 42.0));

        let database_record = provider_accounts::list_xai_records(&paths)
            .await
            .expect("persisted xai records")
            .remove(account_id)
            .expect("database account");
        assert_eq!(database_record.access_token, persisted.access_token);
        assert_eq!(database_record.status, XaiAccountStatus::Active);
        assert_eq!(database_record.quota.plan_type.as_deref(), Some("new-plan"));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn refresh_cache_and_late_writes_do_not_revive_deleted_account() {
        let (store, paths, data_dir) = test_store("delete-race");
        let account_id = "xai-delete@example.com";
        let record = test_record("initial-access-token", Some("delete@example.com"));
        store
            .save_record(account_id.to_string(), record.clone())
            .await
            .expect("seed account");

        // 强制丢弃内存快照，证明 refresh_cache 能从 SQLite 恢复同一真值。
        store.cache.write().await.clear();
        store.refresh_cache().await.expect("refresh cache");
        assert!(store.cache.read().await.contains_key(account_id));

        store
            .delete_account(account_id)
            .await
            .expect("delete account");
        let token_error = store
            .persist_token_response(account_id, &record, refreshed_token_response())
            .await
            .expect_err("late token response must not recreate account");
        let quota_error = store
            .persist_quota_cache(account_id, XaiQuotaCache::default())
            .await
            .expect_err("late quota response must not recreate account");
        store
            .refresh_cache()
            .await
            .expect("refresh cache after delete");
        let records = provider_accounts::list_xai_records(&paths)
            .await
            .expect("persisted xai records");

        assert!(token_error.contains("account not found"));
        assert!(quota_error.contains("account not found"));
        assert!(!store.cache.read().await.contains_key(account_id));
        assert!(!records.contains_key(account_id));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn cancelled_save_after_database_commit_does_not_leave_stale_cache() {
        let (store, paths, data_dir) = test_store("cancelled-save");
        let store = Arc::new(store);
        let account_id = "xai-cancelled-save@example.com";
        let original = test_record("old-access-token", Some("cancelled-save@example.com"));
        store
            .save_record(account_id.to_string(), original)
            .await
            .expect("seed account");
        let hook = store.install_persistence_test_hook();
        let task_store = Arc::clone(&store);
        let updated = test_record("new-access-token", Some("cancelled-save@example.com"));
        let save_task = tokio::spawn(async move {
            task_store
                .save_record(account_id.to_string(), updated)
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), hook.committed.notified())
            .await
            .expect("save should reach the post-commit cancellation point");
        let persisted = provider_accounts::list_xai_records(&paths)
            .await
            .expect("persisted xai records")
            .remove(account_id)
            .expect("database account");
        assert_eq!(persisted.access_token, "new-access-token");

        save_task.abort();
        assert!(save_task
            .await
            .expect_err("save task should be cancelled")
            .is_cancelled());

        // SQLite 已提交时，取消只能留下 cache miss，后续读取必须从真值源恢复。
        assert!(!store.cache.read().await.contains_key(account_id));
        let loaded = store
            .load_account(account_id)
            .await
            .expect("reload committed account");
        assert_eq!(loaded.access_token, "new-access-token");
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn cancelled_delete_after_database_commit_does_not_leave_cached_account() {
        let (store, paths, data_dir) = test_store("cancelled-delete");
        let store = Arc::new(store);
        let account_id = "xai-cancelled-delete@example.com";
        store
            .save_record(
                account_id.to_string(),
                test_record("access-token", Some("cancelled-delete@example.com")),
            )
            .await
            .expect("seed account");
        let hook = store.install_persistence_test_hook();
        let task_store = Arc::clone(&store);
        let delete_task = tokio::spawn(async move { task_store.delete_account(account_id).await });

        tokio::time::timeout(std::time::Duration::from_secs(2), hook.committed.notified())
            .await
            .expect("delete should reach the post-commit cancellation point");
        let persisted = provider_accounts::list_xai_records(&paths)
            .await
            .expect("persisted xai records");
        assert!(!persisted.contains_key(account_id));

        delete_task.abort();
        assert!(delete_task
            .await
            .expect_err("delete task should be cancelled")
            .is_cancelled());

        // 已删除账户不能继续由旧缓存供给，也不能被后续延迟写重新复活。
        assert!(!store.cache.read().await.contains_key(account_id));
        let error = store
            .load_account(account_id)
            .await
            .expect_err("deleted account must stay absent");
        assert!(error.contains("account not found"));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn expired_access_token_only_import_is_rejected() {
        let (store, _paths, data_dir) = test_store("expired-import");
        let mut record = test_record("expired-access-token", Some("expired@example.com"));
        record.refresh_token.clear();
        record.expires_at = "2000-01-01T00:00:00Z".to_string();

        let error = store
            .prepare_import_record(record)
            .await
            .expect_err("expired access-only credential must be rejected");

        assert!(error.contains("cannot be imported without refresh_token"));
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn single_file_import_propagates_persistence_error() {
        let root = std::env::temp_dir().join(format!(
            "token-proxy-xai-store-import-error-{}",
            random::<u64>()
        ));
        tokio::fs::create_dir_all(&root)
            .await
            .expect("create test root");
        let blocker = root.join("not-a-directory");
        tokio::fs::write(&blocker, "block database directory")
            .await
            .expect("create blocker file");
        let import_file = root.join("xai-account.json");
        tokio::fs::write(
            &import_file,
            r#"{"type":"xai","auth_kind":"oauth","access_token":"access","expired":"2999-01-01T00:00:00Z"}"#,
        )
        .await
        .expect("write import file");
        let paths =
            TokenProxyPaths::from_app_data_dir(blocker.join("nested")).expect("blocked test paths");
        let store = XaiAccountStore::new(&paths, token_proxy_account_store::app_proxy::new_state())
            .expect("xai store");

        let error = store
            .import_file(import_file)
            .await
            .expect_err("single file persistence failure must propagate");

        assert!(error.contains("Failed to create db directory"));
        assert!(!error.contains("No valid xAI OAuth accounts"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn quota_entrypoints_share_one_singleflight_guard() {
        let (store, _paths, data_dir) = test_store("quota-singleflight");
        let account_id = "xai-quota@example.com";
        let mut record = test_record("quota-access-token", Some("quota@example.com"));
        record.quota = XaiQuotaCache {
            plan_type: Some("cached-plan".to_string()),
            quotas: vec![crate::types::XaiQuotaItem {
                name: "xai-weekly".to_string(),
                percentage: 73.0,
                used: Some(27.0),
                limit: Some(100.0),
                reset_at: None,
            }],
            error: None,
            checked_at: Some(now_rfc3339()),
        };
        store
            .save_record(account_id.to_string(), record)
            .await
            .expect("seed quota account");
        let permit = start_account_operation(
            &store.quota_refreshing,
            account_id,
            "quota state unavailable",
        )
        .expect("singleflight state")
        .expect("start in-flight refresh");

        let bulk = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.refresh_quota_cache(None),
        )
        .await
        .expect("bulk refresh must not touch network")
        .expect("bulk quota refresh");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.refresh_quota_cache_now(account_id),
        )
        .await
        .expect("single refresh must not touch network")
        .expect("single quota refresh");
        let summaries = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::fetch_quotas(&store),
        )
        .await
        .expect("quota fetch must not touch network")
        .expect("quota summaries");
        drop(permit);

        assert_eq!(bulk, vec![account_id.to_string()]);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].plan_type.as_deref(), Some("cached-plan"));
        assert_eq!(summaries[0].quotas[0].percentage, 73.0);
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
