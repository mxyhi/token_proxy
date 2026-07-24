use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Arc, Mutex as StdMutex};

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::Notify;
use tokio::sync::{Mutex, MutexGuard, RwLock};

use crate::persistence as provider_accounts;
use token_proxy_account_store::app_proxy::AppProxyState;
use token_proxy_account_store::paths::TokenProxyPaths;

use super::oauth;
use super::sso_oidc;
use super::types::{KiroAccountStatus, KiroAccountSummary, KiroTokenRecord};
use super::util::{expires_at_from_seconds, extract_email_from_jwt, now_rfc3339, sanitize_id_part};

const KIRO_AUTH_DIR_NAME: &str = "kiro-auth";

pub struct KiroAccountStore {
    dir: PathBuf,
    paths: TokenProxyPaths,
    cache: RwLock<HashMap<String, KiroTokenRecord>>,
    app_proxy: AppProxyState,
    quota_refreshing: Mutex<HashSet<String>>,
    /// Provider 级 mutation gate：所有持久化写、cache 更新、snapshot/restore 共享。
    /// 阻止 lifecycle 全量 restore 与并发 refresh/save 互相覆盖。
    provider_mutation: Mutex<()>,
    /// 测试探针：即将 acquire provider_mutation 时通知（不含 secret）。
    #[cfg(any(test, feature = "test-support"))]
    gate_probe: StdMutex<Option<Arc<ProviderGateProbe>>>,
}

/// provider gate 测试探针：about_to_lock 在阻塞前，acquired 在拿到锁后。
/// 用于无 sleep 的并发断言；禁止携带 secret。
#[cfg(any(test, feature = "test-support"))]
pub struct ProviderGateProbe {
    pub about_to_lock: Notify,
    pub acquired: Notify,
}

/// Provider import/restore 事务会话：持有 `provider_mutation` 直至 drop。
/// 不暴露裸 MutexGuard；事务内只走 unlocked 路径，避免 Tokio Mutex 自重入。
pub struct KiroProviderMutation<'a> {
    store: &'a KiroAccountStore,
    _gate: MutexGuard<'a, ()>,
}

impl Drop for KiroProviderMutation<'_> {
    fn drop(&mut self) {
        // 会话结束即释放 gate；不记录任何 credential 字段。
        tracing::debug!("kiro provider mutation session end");
    }
}

impl KiroProviderMutation<'_> {
    /// 已持 gate：全量快照（含 secret，仅内存短时持有，禁止日志）。
    pub async fn snapshot_all_records(&self) -> Result<HashMap<String, KiroTokenRecord>, String> {
        self.store.snapshot_all_records_unlocked().await
    }

    /// 已持 gate：SQLite 事务全量恢复后替换 cache。
    pub async fn restore_all_records(
        &self,
        snapshot: HashMap<String, KiroTokenRecord>,
    ) -> Result<(), String> {
        self.store.restore_all_records_unlocked(snapshot).await
    }

    /// 已持 gate：IDE 目录导入（内部 save 走 unlocked）。
    pub async fn import_ide_tokens(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        self.store.import_ide_tokens_unlocked(directory).await
    }

    /// 已持 gate：KAM 导出文件导入（内部 save 走 unlocked）。
    pub async fn import_kam_export(
        &self,
        path: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        self.store.import_kam_export_unlocked(path).await
    }
}

impl KiroAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        let dir = paths.data_dir().join(KIRO_AUTH_DIR_NAME);
        Ok(Self {
            dir,
            paths: paths.clone(),
            cache: RwLock::new(HashMap::new()),
            app_proxy,
            quota_refreshing: Mutex::new(HashSet::new()),
            provider_mutation: Mutex::new(()),
            #[cfg(any(test, feature = "test-support"))]
            gate_probe: StdMutex::new(None),
        })
    }

    /// 开启 provider mutation 事务；session 存活期间独占 provider_mutation。
    pub async fn begin_provider_mutation(&self) -> KiroProviderMutation<'_> {
        tracing::debug!("kiro provider mutation session begin");
        let _gate = self.acquire_provider_mutation().await;
        KiroProviderMutation { store: self, _gate }
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
                // 仅通知「即将等锁」；不携带 account_id / token。
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
        *self.gate_probe.lock().expect("kiro gate probe lock") = Some(Arc::clone(&probe));
        probe
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn clear_provider_gate_probe(&self) {
        *self.gate_probe.lock().expect("kiro gate probe lock") = None;
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub async fn import_ide_tokens(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        // 直接调用自动取 gate；lifecycle 事务内走 unlocked。
        let _gate = self.acquire_provider_mutation().await;
        self.import_ide_tokens_unlocked(directory).await
    }

    pub async fn import_kam_export(
        &self,
        path: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.import_kam_export_unlocked(path).await
    }

    pub async fn list_accounts(&self) -> Result<Vec<KiroAccountSummary>, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let mut items: Vec<KiroAccountSummary> = cache
            .iter()
            .map(|(account_id, record)| KiroAccountSummary {
                account_id: account_id.clone(),
                provider: record.provider.clone(),
                auth_method: record.auth_method.clone(),
                email: record.email.clone(),
                expires_at: record.expires_at().map(|value| {
                    value
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| record.expires_at.clone())
                }),
                status: record.effective_status(),
            })
            .collect();
        // Account store 不再按 priority 排序；路由顺序只看 Upstream.priority。
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub async fn get_account_record(&self, account_id: &str) -> Result<KiroTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub async fn refresh_account(&self, account_id: &str) -> Result<(), String> {
        let record = self.load_account(account_id).await?;
        let refreshed = self.refresh_record(account_id, record).await?;
        let summary = self.save_record(account_id.to_string(), refreshed).await?;
        if matches!(summary.status, KiroAccountStatus::Expired) {
            return Err("Kiro token refresh failed.".to_string());
        }
        Ok(())
    }

    pub async fn refresh_quota_cache(
        &self,
        account_ids: Option<&[String]>,
    ) -> Result<Vec<String>, String> {
        let targets = self.resolve_quota_targets(account_ids).await?;
        let mut refreshed = Vec::new();
        for account_id in targets {
            if self.refresh_quota_if_stale(&account_id).await? {
                refreshed.push(account_id);
            }
        }
        Ok(refreshed)
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.save_record_unlocked(account_id, record).await
    }

    async fn save_record_unlocked(
        &self,
        account_id: String,
        record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        provider_accounts::upsert_kiro_account(&self.paths, &account_id, &record).await?;
        let mut cache = self.cache.write().await;
        cache.insert(account_id.clone(), record.clone());
        Ok(KiroAccountSummary {
            account_id,
            provider: record.provider.clone(),
            auth_method: record.auth_method.clone(),
            email: record.email.clone(),
            expires_at: record.expires_at().map(|value| {
                value
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| record.expires_at.clone())
            }),
            status: record.effective_status(),
        })
    }

    /// Seeds an exact record for cross-crate integration tests.
    #[cfg(feature = "test-support")]
    pub async fn save_record_for_test(
        &self,
        account_id: String,
        record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        self.save_record(account_id, record).await
    }

    pub(crate) async fn persist_quota_cache(
        &self,
        account_id: &str,
        record: KiroTokenRecord,
    ) -> Result<KiroTokenRecord, String> {
        self.save_record(account_id.to_string(), record.clone())
            .await?;
        Ok(record)
    }

    /// 自动取 gate 的新建/覆盖入口；事务内请用 unlocked。
    #[allow(dead_code)]
    pub(crate) async fn save_new_account(
        &self,
        record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.save_new_account_unlocked(record).await
    }

    /// 调用方必须已持 `provider_mutation`：身份匹配 + 分配 ID + 落库。
    async fn save_new_account_unlocked(
        &self,
        mut record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        if record.email.is_none() {
            record.email = extract_email_from_jwt(&record.access_token);
        }
        // 稳定身份：同 provider + email（或 profile_arn）覆盖，避免重复 credential。
        if let Some((account_id, existing)) = self.find_existing_account_unlocked(&record).await? {
            tracing::debug!(
                account_id = %account_id,
                "kiro save reuses existing account by identity"
            );
            record.quota = existing.quota;
            record.status = KiroAccountStatus::Active;
            return self.save_record_unlocked(account_id, record).await;
        }
        let provider = record.provider.trim().to_ascii_lowercase();
        let id_part_source = record
            .email
            .as_deref()
            .or(record.profile_arn.as_deref())
            .unwrap_or_default();
        let mut id_part = sanitize_id_part(id_part_source);
        if id_part.is_empty() {
            id_part = format!("{}", OffsetDateTime::now_utc().unix_timestamp());
        }
        let account_id = self.unique_account_id_unlocked(&provider, &id_part).await?;
        self.save_record_unlocked(account_id, record).await
    }

    /// 编排层删除账户凭据；刷新内存 cache。禁止在日志中打印 token。
    pub async fn delete_account(&self, account_id: &str) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        provider_accounts::delete_account(&self.paths, account_id).await?;
        let mut cache = self.cache.write().await;
        cache.remove(account_id);
        tracing::debug!(account_id, "kiro account deleted for orchestration");
        Ok(())
    }

    /// 编排层快照：导出全部凭据（含 secret，仅内存短时持有，禁止日志）。
    pub async fn snapshot_all_records(&self) -> Result<HashMap<String, KiroTokenRecord>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.snapshot_all_records_unlocked().await
    }

    /// 编排层恢复：单 SQLite 事务替换该 provider 全量快照，提交后再替换 cache。
    pub async fn restore_all_records(
        &self,
        snapshot: HashMap<String, KiroTokenRecord>,
    ) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        self.restore_all_records_unlocked(snapshot).await
    }

    /// 单账户快照；不存在返回 None。
    pub async fn snapshot_account_record(
        &self,
        account_id: &str,
    ) -> Result<Option<KiroTokenRecord>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.reload_cache_unlocked().await?;
        Ok(self.cache.read().await.get(account_id).cloned())
    }

    /// 单账户原子恢复；`None` 表示删除。DB 提交成功后再改 cache。
    pub async fn restore_account_record(
        &self,
        account_id: &str,
        record: Option<KiroTokenRecord>,
    ) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        provider_accounts::replace_kiro_account(&self.paths, account_id, record.as_ref()).await?;
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
        tracing::debug!(account_id, "kiro account record restored for orchestration");
        Ok(())
    }

    /// 登录编排提交：持久化 credential，返回 summary 与覆盖前旧 record。
    pub async fn commit_login_record(
        &self,
        record: KiroTokenRecord,
    ) -> Result<(KiroAccountSummary, Option<KiroTokenRecord>), String> {
        let _gate = self.acquire_provider_mutation().await;
        let mut record = record;
        if record.email.is_none() {
            record.email = extract_email_from_jwt(&record.access_token);
        }
        let previous = self
            .find_existing_account_unlocked(&record)
            .await?
            .map(|(_, old)| old);
        // 已持 gate：走 unlocked 路径，避免重入死锁。
        let summary = self.save_new_account_unlocked(record).await?;
        Ok((summary, previous))
    }

    async fn refresh_if_needed(
        &self,
        account_id: &str,
        record: KiroTokenRecord,
    ) -> Result<KiroTokenRecord, String> {
        if !record.is_expired() {
            return Ok(record);
        }
        tracing::debug!(
            account_id,
            "kiro token expired; refreshing pinned credential"
        );
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: KiroTokenRecord,
    ) -> Result<KiroTokenRecord, String> {
        // OAuth/refresh 网络走 app 级 proxy，账户不再持有 proxy_url。
        let proxy_url = self.app_proxy_url().await;
        let refreshed = match record.auth_method.as_str() {
            "builder-id" => sso_oidc::refresh_builder_token(&record, proxy_url.as_deref()).await?,
            "idc" => sso_oidc::refresh_idc_token(&record, proxy_url.as_deref()).await?,
            "social" => oauth::refresh_social_token(&record, proxy_url.as_deref()).await?,
            _ => return Err("Unsupported Kiro auth method.".to_string()),
        };
        // 保留本地邮箱与 quota 缓存，避免 refresh 响应丢失元数据。
        let refreshed = KiroTokenRecord {
            email: record.email.clone().or(refreshed.email),
            quota: record.quota.clone(),
            ..refreshed
        };
        let summary = self
            .save_record(account_id.to_string(), refreshed.clone())
            .await?;
        if matches!(summary.status, KiroAccountStatus::Expired) {
            return Err("Kiro token refresh failed.".to_string());
        }
        Ok(refreshed)
    }

    pub(crate) async fn load_account(&self, account_id: &str) -> Result<KiroTokenRecord, String> {
        if let Some(record) = self.cache.read().await.get(account_id).cloned() {
            return Ok(record);
        }
        self.refresh_cache().await?;
        self.cache
            .read()
            .await
            .get(account_id)
            .cloned()
            .ok_or_else(|| format!("Kiro account not found: {account_id}"))
    }

    pub async fn app_proxy_url(&self) -> Option<String> {
        self.app_proxy.read().await.clone()
    }

    pub async fn refresh_quota_if_stale(&self, account_id: &str) -> Result<bool, String> {
        if !self.start_quota_refresh(account_id).await {
            return Ok(false);
        }
        let result = self.refresh_quota_if_stale_inner(account_id).await;
        self.finish_quota_refresh(account_id).await;
        result
    }

    pub async fn refresh_quota_cache_now(&self, account_id: &str) -> Result<(), String> {
        if !self.start_quota_refresh(account_id).await {
            return Ok(());
        }
        let result = super::quota::refresh_quota_cache(self, account_id).await;
        self.finish_quota_refresh(account_id).await;
        result.map(|_| ())
    }

    /// 解析 Upstream 固定引用的账户凭据。无 pool/unpinned 选号。
    pub async fn resolve_pinned_account_record(
        &self,
        account_id: &str,
    ) -> Result<(String, KiroTokenRecord), String> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err("Kiro account_id is required for account credential upstream.".to_string());
        }
        let record = self.get_account_record(account_id).await?;
        if !record.is_usable() {
            tracing::warn!(
                account_id,
                status = ?record.effective_status(),
                "pinned kiro account is not usable"
            );
            return Err(format!(
                "Kiro account is {}: {account_id}",
                record.effective_status().as_label()
            ));
        }
        tracing::debug!(account_id, "resolved pinned kiro account credential");
        Ok((account_id.to_string(), record))
    }

    async fn resolve_quota_targets(
        &self,
        account_ids: Option<&[String]>,
    ) -> Result<Vec<String>, String> {
        if let Some(account_ids) = account_ids {
            let mut targets = account_ids
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            targets.sort();
            targets.dedup();
            return Ok(targets);
        }

        self.refresh_cache().await?;
        let mut targets = self.cache.read().await.keys().cloned().collect::<Vec<_>>();
        targets.sort();
        Ok(targets)
    }

    async fn start_quota_refresh(&self, account_id: &str) -> bool {
        let mut refreshing = self.quota_refreshing.lock().await;
        if refreshing.contains(account_id) {
            return false;
        }
        refreshing.insert(account_id.to_string());
        true
    }

    async fn finish_quota_refresh(&self, account_id: &str) {
        let mut refreshing = self.quota_refreshing.lock().await;
        refreshing.remove(account_id);
    }

    async fn refresh_quota_if_stale_inner(&self, account_id: &str) -> Result<bool, String> {
        let record = self.load_account(account_id).await?;
        if !quota_refresh_is_due(record.quota.checked_at.as_deref()) {
            return Ok(false);
        }
        super::quota::refresh_quota_cache_if_stale(self, account_id).await?;
        Ok(true)
    }

    async fn refresh_cache(&self) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        self.reload_cache_unlocked().await
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn reload_cache_unlocked(&self) -> Result<(), String> {
        let cache = provider_accounts::list_kiro_records(&self.paths).await?;
        let mut guard = self.cache.write().await;
        *guard = cache;
        Ok(())
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn snapshot_all_records_unlocked(
        &self,
    ) -> Result<HashMap<String, KiroTokenRecord>, String> {
        self.reload_cache_unlocked().await?;
        Ok(self.cache.read().await.clone())
    }

    /// 调用方必须已持 `provider_mutation`。DB 原子替换成功后再换 cache。
    async fn restore_all_records_unlocked(
        &self,
        snapshot: HashMap<String, KiroTokenRecord>,
    ) -> Result<(), String> {
        provider_accounts::replace_all_kiro_records(&self.paths, &snapshot).await?;
        *self.cache.write().await = snapshot;
        tracing::debug!("kiro account store restored from orchestration snapshot");
        Ok(())
    }

    /// 调用方必须已持 `provider_mutation`：IDE 目录导入。
    async fn import_ide_tokens_unlocked(
        &self,
        directory: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        if directory.as_os_str().is_empty() {
            return Err("Directory is required.".to_string());
        }
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err("Selected directory not found.".to_string());
            }
            Err(err) => {
                return Err(format!("Failed to read selected directory: {err}"));
            }
        };
        let mut imported = Vec::new();
        // 仅扫描所选目录本层的 JSON 文件，忽略无效内容。
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read directory entry: {err}"))?
        {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .map_err(|err| format!("Failed to read entry type: {err}"))?;
            if !file_type.is_file() || !is_json_file(&path) {
                continue;
            }
            let Some(record) = load_ide_token_record(&path).await else {
                continue;
            };
            if let Ok(summary) = self.save_new_account_unlocked(record).await {
                imported.push(summary);
            }
        }
        if imported.is_empty() {
            return Err("No valid Kiro token JSON files found.".to_string());
        }
        Ok(imported)
    }

    /// 调用方必须已持 `provider_mutation`：KAM 导出文件导入。
    async fn import_kam_export_unlocked(
        &self,
        path: PathBuf,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        if path.as_os_str().is_empty() {
            return Err("File path is required.".to_string());
        }
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err("Selected file not found.".to_string());
        }
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|err| format!("Failed to read JSON file: {err}"))?;
        let data: KamExportData = serde_json::from_str(&contents)
            .map_err(|err| format!("Invalid Kiro account JSON file: {err}"))?;
        let mut imported = Vec::new();
        for account in data.accounts {
            let Some(record) = kam_account_to_record(account) else {
                continue;
            };
            if let Ok(summary) = self.save_new_account_unlocked(record).await {
                imported.push(summary);
            }
        }
        if imported.is_empty() {
            return Err("No valid Kiro accounts found in JSON file.".to_string());
        }
        Ok(imported)
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn unique_account_id_unlocked(
        &self,
        provider: &str,
        id_part: &str,
    ) -> Result<String, String> {
        self.reload_cache_unlocked().await?;
        let cache = self.cache.read().await;
        let mut suffix = 0u32;
        loop {
            let candidate = if suffix == 0 {
                format!("kiro-{provider}-{id_part}.json")
            } else {
                format!("kiro-{provider}-{id_part}-{suffix}.json")
            };
            if !cache.contains_key(&candidate) {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }

    /// 稳定身份匹配：provider + email；无 email 时 provider + profile_arn。
    /// 调用方必须已持 `provider_mutation`。
    async fn find_existing_account_unlocked(
        &self,
        record: &KiroTokenRecord,
    ) -> Result<Option<(String, KiroTokenRecord)>, String> {
        self.reload_cache_unlocked().await?;
        let provider = record.provider.trim().to_ascii_lowercase();
        let email = record
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        let profile_arn = record
            .profile_arn
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let cache = self.cache.read().await;
        if let Some(email) = email.as_ref() {
            if let Some((account_id, existing)) = cache.iter().find(|(_, existing)| {
                existing.provider.trim().eq_ignore_ascii_case(&provider)
                    && existing
                        .email
                        .as_deref()
                        .map(str::trim)
                        .map(|value| value.to_ascii_lowercase())
                        .as_ref()
                        == Some(email)
            }) {
                return Ok(Some((account_id.clone(), existing.clone())));
            }
        }
        if email.is_none() {
            if let Some(profile_arn) = profile_arn {
                if let Some((account_id, existing)) = cache.iter().find(|(_, existing)| {
                    existing.provider.trim().eq_ignore_ascii_case(&provider)
                        && existing
                            .profile_arn
                            .as_deref()
                            .map(str::trim)
                            .is_some_and(|value| value == profile_arn)
                }) {
                    return Ok(Some((account_id.clone(), existing.clone())));
                }
            }
        }
        Ok(None)
    }
}

const QUOTA_REFRESH_INTERVAL_SECONDS: i64 = 30;

fn quota_refresh_is_due(checked_at: Option<&str>) -> bool {
    let Some(checked_at) = checked_at.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Ok(checked_at) = OffsetDateTime::parse(checked_at, &Rfc3339) else {
        return true;
    };
    OffsetDateTime::now_utc() - checked_at >= Duration::seconds(QUOTA_REFRESH_INTERVAL_SECONDS)
}

fn is_json_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

async fn load_ide_token_record(path: &Path) -> Option<KiroTokenRecord> {
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    let token: KiroIdeTokenFile = serde_json::from_str(&contents).ok()?;
    token.into_record().ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KamExportData {
    accounts: Vec<KamAccount>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KamAccount {
    email: Option<String>,
    idp: Option<String>,
    credentials: Option<KamCredentials>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KamCredentials {
    access_token: Option<String>,
    refresh_token: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    region: Option<String>,
    start_url: Option<String>,
    expires_at: Option<i64>,
    auth_method: Option<String>,
    provider: Option<String>,
}

fn kam_account_to_record(account: KamAccount) -> Option<KiroTokenRecord> {
    let credentials = account.credentials?;
    let access_token = credentials.access_token?.trim().to_string();
    let refresh_token = credentials.refresh_token?.trim().to_string();
    if access_token.is_empty() || refresh_token.is_empty() {
        return None;
    }
    let provider = credentials
        .provider
        .filter(|value| !value.trim().is_empty())
        .or(account.idp.filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| "AWS".to_string());
    let auth_method =
        normalize_auth_method(credentials.auth_method.as_deref(), Some(provider.as_str()));
    let expires_at = credentials
        .expires_at
        .and_then(format_expires_at)
        .unwrap_or_else(|| expires_at_from_seconds(3600));
    Some(KiroTokenRecord {
        access_token,
        refresh_token,
        profile_arn: None,
        expires_at,
        auth_method,
        provider,
        client_id: credentials.client_id,
        client_secret: credentials.client_secret,
        email: account.email.filter(|value| !value.trim().is_empty()),
        last_refresh: Some(now_rfc3339()),
        start_url: credentials.start_url,
        region: credentials.region,
        status: KiroAccountStatus::Active,
        quota: super::types::KiroQuotaCache::default(),
    })
}

fn normalize_auth_method(raw: Option<&str>, provider: Option<&str>) -> String {
    let raw_value = raw.unwrap_or("").trim().to_ascii_lowercase();
    if matches!(raw_value.as_str(), "idc") {
        return "idc".to_string();
    }
    if matches!(raw_value.as_str(), "social") {
        return "social".to_string();
    }
    if matches!(raw_value.as_str(), "builder-id" | "builder_id") {
        return "builder-id".to_string();
    }
    let provider_value = provider.unwrap_or("").trim().to_ascii_lowercase();
    if provider_value.contains("google") || provider_value.contains("github") {
        return "social".to_string();
    }
    if provider_value.contains("idc")
        || provider_value.contains("enterprise")
        || provider_value.contains("iam")
    {
        return "idc".to_string();
    }
    "builder-id".to_string()
}

fn format_expires_at(value: i64) -> Option<String> {
    let (seconds, nanos) = if value >= 10_000_000_000 {
        let secs = value / 1000;
        let ms = value % 1000;
        (secs, ms * 1_000_000)
    } else {
        (value, 0)
    };
    let nanos_total = i128::from(seconds)
        .checked_mul(1_000_000_000)?
        .checked_add(i128::from(nanos))?;
    OffsetDateTime::from_unix_timestamp_nanos(nanos_total)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroIdeTokenFile {
    access_token: String,
    refresh_token: String,
    profile_arn: Option<String>,
    expires_at: Option<String>,
    auth_method: Option<String>,
    provider: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    email: Option<String>,
    start_url: Option<String>,
    region: Option<String>,
    last_refresh: Option<String>,
}

impl KiroIdeTokenFile {
    fn into_record(self) -> Result<KiroTokenRecord, String> {
        if self.access_token.trim().is_empty() {
            return Err("Missing access token.".to_string());
        }
        if self.refresh_token.trim().is_empty() {
            return Err("Missing refresh token.".to_string());
        }
        let provider = self
            .provider
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "AWS".to_string());
        // Default to Builder ID when metadata is missing in IDE token files.
        let auth_method = self
            .auth_method
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if provider.eq_ignore_ascii_case("google") {
                    "social".to_string()
                } else {
                    "builder-id".to_string()
                }
            });
        let expires_at = match self.expires_at.as_deref() {
            Some(value) if !value.trim().is_empty() => value.to_string(),
            _ => expires_at_from_seconds(3600),
        };
        let last_refresh = self
            .last_refresh
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(now_rfc3339);
        Ok(KiroTokenRecord {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            profile_arn: self.profile_arn,
            expires_at,
            auth_method,
            provider,
            client_id: self.client_id,
            client_secret: self.client_secret,
            email: self.email.filter(|value| !value.trim().is_empty()),
            last_refresh: Some(last_refresh),
            start_url: self.start_url,
            region: self.region,
            status: KiroAccountStatus::Active,
            quota: super::types::KiroQuotaCache::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::random;
    use serde_json::json;
    use std::future::Future;
    use time::Duration;
    use token_proxy_account_store::app_proxy;
    use token_proxy_account_store::paths::TokenProxyPaths;

    fn run_async(test: impl Future<Output = ()>) {
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(test);
    }

    fn create_test_store() -> (KiroAccountStore, PathBuf) {
        let data_dir =
            std::env::temp_dir().join(format!("token-proxy-kiro-store-test-{}", random::<u64>()));
        std::fs::create_dir_all(&data_dir).expect("create test data dir");
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
        let store = KiroAccountStore::new(&paths, app_proxy::new_state()).expect("kiro store");
        (store, data_dir)
    }

    fn future_rfc3339(hours: i64) -> String {
        (OffsetDateTime::now_utc() + Duration::hours(hours))
            .format(&Rfc3339)
            .expect("format expires_at")
    }

    #[test]
    fn quota_refresh_waits_for_30_second_interval() {
        let within_window = (OffsetDateTime::now_utc() - Duration::seconds(29))
            .format(&Rfc3339)
            .expect("format checked_at");
        assert!(!quota_refresh_is_due(Some(within_window.as_str())));

        let outside_window = (OffsetDateTime::now_utc() - Duration::seconds(31))
            .format(&Rfc3339)
            .expect("format checked_at");
        assert!(quota_refresh_is_due(Some(outside_window.as_str())));
    }

    #[test]
    fn list_accounts_reads_from_sqlite_after_legacy_files_are_removed() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let saved = store
                .save_new_account(KiroTokenRecord {
                    access_token: "access-token".to_string(),
                    refresh_token: "refresh-token".to_string(),
                    profile_arn: Some("arn:aws:iam::123456789012:user/test".to_string()),
                    expires_at: future_rfc3339(6),
                    auth_method: "google".to_string(),
                    provider: "kiro".to_string(),
                    client_id: None,
                    client_secret: None,
                    email: Some("kiro-db@example.com".to_string()),
                    last_refresh: None,
                    start_url: None,
                    region: None,
                    status: KiroAccountStatus::Active,
                    quota: crate::KiroQuotaCache::default(),
                })
                .await
                .expect("save kiro account");
            let legacy_dir = data_dir.join("kiro-auth");
            if legacy_dir.exists() {
                std::fs::remove_dir_all(&legacy_dir).expect("remove legacy auth dir");
            }

            let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
            let reloaded_store =
                KiroAccountStore::new(&paths, app_proxy::new_state()).expect("kiro store");
            let accounts = reloaded_store
                .list_accounts()
                .await
                .expect("list accounts should read sqlite data");

            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0].account_id, saved.account_id);
            assert_eq!(accounts[0].email.as_deref(), Some("kiro-db@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn list_accounts_orders_by_account_id_ascending() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            // Phase B: 列表稳定按 account_id 升序，不再按 priority。
            store
                .save_record(
                    "kiro-z.json".to_string(),
                    KiroTokenRecord {
                        access_token: "access-z".to_string(),
                        refresh_token: "refresh-z".to_string(),
                        profile_arn: Some("arn:aws:iam::123456789012:user/z".to_string()),
                        expires_at: future_rfc3339(6),
                        auth_method: "google".to_string(),
                        provider: "kiro".to_string(),
                        client_id: None,
                        client_secret: None,
                        email: Some("z@example.com".to_string()),
                        last_refresh: None,
                        start_url: None,
                        region: None,
                        status: KiroAccountStatus::Active,
                        quota: crate::KiroQuotaCache::default(),
                    },
                )
                .await
                .expect("save z");
            store
                .save_record(
                    "kiro-a.json".to_string(),
                    KiroTokenRecord {
                        access_token: "access-a".to_string(),
                        refresh_token: "refresh-a".to_string(),
                        profile_arn: Some("arn:aws:iam::123456789012:user/a".to_string()),
                        expires_at: future_rfc3339(6),
                        auth_method: "google".to_string(),
                        provider: "kiro".to_string(),
                        client_id: None,
                        client_secret: None,
                        email: Some("a@example.com".to_string()),
                        last_refresh: None,
                        start_url: None,
                        region: None,
                        status: KiroAccountStatus::Active,
                        quota: crate::KiroQuotaCache::default(),
                    },
                )
                .await
                .expect("save a");

            let ids = store
                .list_accounts()
                .await
                .expect("list")
                .into_iter()
                .map(|item| item.account_id)
                .collect::<Vec<_>>();
            assert_eq!(
                ids,
                vec!["kiro-a.json".to_string(), "kiro-z.json".to_string()]
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn get_account_record_returns_unexpired_without_refresh() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            // 未过期账户：读取路径不发起 refresh 网络。
            store
                .save_record(
                    "kiro-builder-active.json".to_string(),
                    KiroTokenRecord {
                        access_token: "active-access".to_string(),
                        refresh_token: "refresh-token".to_string(),
                        profile_arn: None,
                        expires_at: future_rfc3339(6),
                        auth_method: "builder-id".to_string(),
                        provider: "AWS".to_string(),
                        client_id: Some("client-id".to_string()),
                        client_secret: Some("client-secret".to_string()),
                        email: Some("active@example.com".to_string()),
                        last_refresh: None,
                        start_url: None,
                        region: None,
                        status: KiroAccountStatus::Active,
                        quota: crate::KiroQuotaCache::default(),
                    },
                )
                .await
                .expect("save active account");

            let record = store
                .get_account_record("kiro-builder-active.json")
                .await
                .expect("active account should load");

            assert!(matches!(
                record.effective_status(),
                KiroAccountStatus::Active
            ));
            assert!(record.is_usable());
            assert_eq!(record.access_token, "active-access");

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn list_accounts_does_not_load_legacy_directory_records() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let legacy_dir = data_dir.join("kiro-auth");
            tokio::fs::create_dir_all(&legacy_dir)
                .await
                .expect("create legacy kiro dir");
            tokio::fs::write(
                legacy_dir.join("kiro-legacy.json"),
                serde_json::to_string_pretty(&json!({
                    "access_token": "legacy-access-token",
                    "refresh_token": "legacy-refresh-token",
                    "profile_arn": "arn:aws:iam::123456789012:user/legacy",
                    "expires_at": future_rfc3339(6),
                    "auth_method": "google",
                    "provider": "kiro",
                    "client_id": null,
                    "client_secret": null,
                    "email": "legacy-kiro@example.com",
                    "last_refresh": null,
                    "start_url": null,
                    "region": null
                }))
                .expect("serialize legacy kiro json"),
            )
            .await
            .expect("write legacy kiro json");

            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should only use sqlite");
            assert!(accounts.is_empty());

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    /// 无 sleep：transaction 持 gate 期间并发 save 阻塞；release 后写入不被旧 snapshot 覆盖。
    #[test]
    fn provider_mutation_session_serializes_concurrent_save() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            store
                .save_record(
                    "kiro-txn-1".to_string(),
                    KiroTokenRecord {
                        access_token: "v1".to_string(),
                        refresh_token: "r1".to_string(),
                        profile_arn: None,
                        expires_at: future_rfc3339(6),
                        auth_method: "builder-id".to_string(),
                        provider: "Google".to_string(),
                        client_id: None,
                        client_secret: None,
                        email: Some("txn@example.com".to_string()),
                        last_refresh: None,
                        start_url: None,
                        region: None,
                        status: KiroAccountStatus::Active,
                        quota: crate::KiroQuotaCache::default(),
                    },
                )
                .await
                .expect("seed");

            let probe = store.install_provider_gate_probe();
            let txn = store.begin_provider_mutation().await;
            let snapshot = txn.snapshot_all_records().await.expect("snapshot");
            assert_eq!(snapshot.len(), 1);

            // 订阅后 spawn，确保 about_to_lock 不丢失。
            let save_about_to_lock = probe.about_to_lock.notified();
            let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
            let store_save = &store;
            // store 非 Clone：用路径上的并发 save 走同一引用需 spawn 作用域内完成。
            // 这里用局部 async block + select 模拟：先 park 在 about_to_lock。
            let save_fut = async {
                let result = store
                    .save_record(
                        "kiro-txn-1".to_string(),
                        KiroTokenRecord {
                            access_token: "v2".to_string(),
                            refresh_token: "r2".to_string(),
                            profile_arn: None,
                            expires_at: future_rfc3339(6),
                            auth_method: "builder-id".to_string(),
                            provider: "Google".to_string(),
                            client_id: None,
                            client_secret: None,
                            email: Some("txn@example.com".to_string()),
                            last_refresh: None,
                            start_url: None,
                            region: None,
                            status: KiroAccountStatus::Active,
                            quota: crate::KiroQuotaCache::default(),
                        },
                    )
                    .await;
                let _ = done_tx.send(result);
            };
            tokio::pin!(save_fut);

            // 推进 save 至 about_to_lock；txn 仍持 gate。
            tokio::select! {
                _ = save_about_to_lock => {}
                _ = &mut save_fut => panic!("save finished before blocking on gate"),
            }
            assert!(
                done_rx.try_recv().is_err(),
                "save must remain incomplete while transaction holds gate"
            );

            // 模拟 binding 失败 rollback（restore 旧 snapshot）。
            txn.restore_all_records(snapshot).await.expect("restore");
            drop(txn);

            save_fut.await;
            done_rx.await.expect("save result").expect("save ok");

            let final_record = store
                .snapshot_account_record("kiro-txn-1")
                .await
                .expect("snap")
                .expect("exists");
            assert_eq!(final_record.access_token, "v2");
            let _ = store_save;
            let _ = std::fs::remove_dir_all(data_dir);
        });
    }
}
