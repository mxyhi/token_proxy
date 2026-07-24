use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use serde::Deserialize;
use serde_json::Value;
use time::{Duration, OffsetDateTime};
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::Notify;
use tokio::sync::{Mutex, MutexGuard, RwLock};

use crate::persistence as provider_accounts;
use token_proxy_account_store::app_proxy::AppProxyState;
use token_proxy_account_store::oauth_util::{
    decode_jwt_payload, expires_at_from_seconds, extract_chatgpt_account_id_from_jwt,
    extract_chatgpt_user_id_from_jwt, extract_email_from_jwt, normalize_proxy_url, now_rfc3339,
    sanitize_id_part,
};
use token_proxy_account_store::paths::TokenProxyPaths;

use super::error::error_requires_relogin;
use super::oauth::{CodexOAuthClient, CodexRefreshTokenClient};
use super::types::{CodexAccountStatus, CodexAccountSummary, CodexCredential, CodexTokenRecord};

pub struct CodexAccountStore {
    paths: TokenProxyPaths,
    cache: RwLock<HashMap<String, CodexTokenRecord>>,
    app_proxy: AppProxyState,
    quota_refreshing: Mutex<HashSet<String>>,
    token_refreshing: StdMutex<HashSet<String>>,
    agent_task_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Provider 级 mutation gate：所有持久化写、cache 更新、snapshot/restore 共享。
    provider_mutation: Mutex<()>,
    /// 测试探针：即将 acquire provider_mutation 时通知（不含 secret）。
    #[cfg(any(test, feature = "test-support"))]
    gate_probe: StdMutex<Option<Arc<ProviderGateProbe>>>,
    #[cfg(any(test, feature = "test-support"))]
    token_url_override: RwLock<Option<String>>,
    #[cfg(any(test, feature = "test-support"))]
    agent_auth_url_override: RwLock<Option<String>>,
    #[cfg(any(test, feature = "test-support"))]
    agent_jwks_url_override: RwLock<Option<String>>,
}

/// provider gate 测试探针：about_to_lock 在阻塞前，acquired 在拿到锁后。
#[cfg(any(test, feature = "test-support"))]
pub struct ProviderGateProbe {
    pub about_to_lock: Notify,
    pub acquired: Notify,
}

/// Provider import/restore 事务会话：持有 `provider_mutation` 直至 drop。
/// 不暴露裸 MutexGuard；事务内只走 unlocked 路径，避免 Tokio Mutex 自重入。
pub struct CodexProviderMutation<'a> {
    store: &'a CodexAccountStore,
    _gate: MutexGuard<'a, ()>,
}

impl Drop for CodexProviderMutation<'_> {
    fn drop(&mut self) {
        tracing::debug!("codex provider mutation session end");
    }
}

impl CodexProviderMutation<'_> {
    /// 已持 gate：全量快照（含 secret，仅内存短时持有，禁止日志）。
    pub async fn snapshot_all_records(&self) -> Result<HashMap<String, CodexTokenRecord>, String> {
        self.store.snapshot_all_records_unlocked().await
    }

    /// 已持 gate：SQLite 事务全量恢复后替换 cache。
    pub async fn restore_all_records(
        &self,
        snapshot: HashMap<String, CodexTokenRecord>,
    ) -> Result<(), String> {
        self.store.restore_all_records_unlocked(snapshot).await
    }

    /// 已持 gate：文件/目录导入。
    pub async fn import_file(&self, path: PathBuf) -> Result<Vec<CodexAccountSummary>, String> {
        self.store.import_file_unlocked(path).await
    }

    /// 已持 gate：文本导入。
    pub async fn import_text(&self, contents: &str) -> Result<Vec<CodexAccountSummary>, String> {
        self.store.import_text_unlocked(contents).await
    }

    /// 已持 gate：refresh token 导入（含网络 refresh）。
    pub async fn import_refresh_tokens(
        &self,
        contents: &str,
        client: CodexRefreshTokenClient,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        self.store
            .import_refresh_tokens_unlocked(contents, client)
            .await
    }
}

const CODEX_TOKEN_REFRESH_WINDOW: Duration = Duration::minutes(15);
const CODEX_TOKEN_REFRESH_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(50);
const CODEX_TOKEN_REFRESH_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);

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

impl CodexAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        Ok(Self {
            paths: paths.clone(),
            cache: RwLock::new(HashMap::new()),
            app_proxy,
            quota_refreshing: Mutex::new(HashSet::new()),
            token_refreshing: StdMutex::new(HashSet::new()),
            agent_task_locks: Mutex::new(HashMap::new()),
            provider_mutation: Mutex::new(()),
            #[cfg(any(test, feature = "test-support"))]
            gate_probe: StdMutex::new(None),
            #[cfg(any(test, feature = "test-support"))]
            token_url_override: RwLock::new(None),
            #[cfg(any(test, feature = "test-support"))]
            agent_auth_url_override: RwLock::new(None),
            #[cfg(any(test, feature = "test-support"))]
            agent_jwks_url_override: RwLock::new(None),
        })
    }

    /// 开启 provider mutation 事务；session 存活期间独占 provider_mutation。
    pub async fn begin_provider_mutation(&self) -> CodexProviderMutation<'_> {
        tracing::debug!("codex provider mutation session begin");
        let _gate = self.acquire_provider_mutation().await;
        CodexProviderMutation { store: self, _gate }
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
        *self.gate_probe.lock().expect("codex gate probe lock") = Some(Arc::clone(&probe));
        probe
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn clear_provider_gate_probe(&self) {
        *self.gate_probe.lock().expect("codex gate probe lock") = None;
    }

    pub async fn list_accounts(&self) -> Result<Vec<CodexAccountSummary>, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let mut items: Vec<CodexAccountSummary> = cache
            .iter()
            .map(|(account_id, record)| account_summary(account_id.clone(), record))
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub async fn import_file(&self, path: PathBuf) -> Result<Vec<CodexAccountSummary>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.import_file_unlocked(path).await
    }

    pub async fn import_text(&self, contents: &str) -> Result<Vec<CodexAccountSummary>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.import_text_unlocked(contents).await
    }

    async fn parse_import_records(&self, contents: &str) -> Result<Vec<CodexTokenRecord>, String> {
        let candidates = parse_agent_identity_candidates(contents)?;
        let Some(candidates) = candidates else {
            return parse_oauth_import_records(contents);
        };
        let proxy_url = self.effective_proxy_url(None).await;
        let client = token_proxy_account_store::oauth_util::build_reqwest_client(
            proxy_url.as_deref(),
            std::time::Duration::from_secs(30),
        )
        .map_err(|_| "Failed to build Agent Identity import client.".to_string())?;
        let mut jwks = None;
        let mut records = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let raw = match candidate {
                AgentIdentityImport::Record(raw) => raw,
                AgentIdentityImport::Jwt(jwt) => {
                    if jwks.is_none() {
                        let jwks_base_url = self.agent_jwks_base_url().await;
                        jwks =
                            Some(super::agent_identity::fetch_jwks(&client, &jwks_base_url).await?);
                    }
                    let claims = super::agent_identity::verify_jwt(
                        &jwt,
                        jwks.as_ref().expect("JWKS loaded before JWT verification"),
                    )?;
                    RawAgentIdentity {
                        agent_runtime_id: claims.agent_runtime_id,
                        agent_private_key: claims.agent_private_key,
                        task_id: None,
                        account_id: claims.account_id,
                        chatgpt_user_id: claims.chatgpt_user_id,
                        email: optional_nonempty(claims.email),
                        plan_type: super::agent_identity::normalize_plan_type(&claims.plan_type),
                        chatgpt_account_is_fedramp: claims.chatgpt_account_is_fedramp,
                    }
                }
            };
            records.push(raw.into_record()?);
        }
        tracing::info!(
            count = records.len(),
            "agent identity import credentials verified"
        );
        Ok(records)
    }

    pub async fn import_refresh_tokens(
        &self,
        contents: &str,
        client: CodexRefreshTokenClient,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.import_refresh_tokens_unlocked(contents, client).await
    }

    pub async fn get_account_record(&self, account_id: &str) -> Result<CodexTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    /// Builds a fresh upstream Authorization value without exposing credential material to logs.
    pub async fn authorization_header(&self, account_id: &str) -> Result<String, String> {
        let record = self.ensure_agent_identity_task(account_id, None).await?;
        if let Some(oauth) = record.oauth() {
            if oauth.access_token.trim().is_empty() {
                return Err("Codex OAuth access token is missing.".to_string());
            }
            return Ok(format!("Bearer {}", oauth.access_token));
        }
        let identity = record
            .agent_identity()
            .ok_or_else(|| "Codex credential type is unsupported.".to_string())?;
        super::agent_identity::authorization_header(identity)
    }

    /// Re-registers only when the persisted task still matches the failed request.
    pub async fn recover_agent_identity_task(
        &self,
        account_id: &str,
        expected_task_id: &str,
    ) -> Result<(), String> {
        let record = self
            .ensure_agent_identity_task(account_id, Some(expected_task_id))
            .await?;
        if record.agent_identity().is_none() {
            return Err("Codex account is not an Agent Identity account.".to_string());
        }
        Ok(())
    }

    pub fn is_agent_identity_task_invalid(status: u16, body: &[u8]) -> bool {
        super::agent_identity::is_task_invalid_response(status, body)
    }

    async fn ensure_agent_identity_task(
        &self,
        account_id: &str,
        expected_task_id: Option<&str>,
    ) -> Result<CodexTokenRecord, String> {
        let current = self.load_account(account_id).await?;
        let Some(identity) = current.agent_identity() else {
            return Ok(current);
        };
        if task_is_current(identity.task_id, expected_task_id) {
            return Ok(current);
        }

        let task_lock = {
            let mut locks = self.agent_task_locks.lock().await;
            locks
                .entry(account_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = task_lock.lock().await;
        let mut record = self.load_account(account_id).await?;
        let identity = record
            .agent_identity()
            .ok_or_else(|| "Codex account is not an Agent Identity account.".to_string())?;
        if task_is_current(identity.task_id, expected_task_id) {
            return Ok(record);
        }

        let proxy_url = self.effective_proxy_url(None).await;
        let client = token_proxy_account_store::oauth_util::build_reqwest_client(
            proxy_url.as_deref(),
            std::time::Duration::from_secs(30),
        )
        .map_err(|_| "Failed to build Agent Identity task registration client.".to_string())?;
        let auth_base_url = self.agent_auth_base_url().await;
        let task_id =
            super::agent_identity::register_task(&client, &auth_base_url, identity).await?;
        let CodexCredential::AgentIdentity {
            task_id: stored_task_id,
            ..
        } = &mut record.credential
        else {
            return Err("Codex account is not an Agent Identity account.".to_string());
        };
        *stored_task_id = Some(task_id);
        self.save_record(account_id.to_string(), record.clone())
            .await?;
        tracing::info!(account_id, "agent identity task registered and persisted");
        Ok(record)
    }

    async fn agent_auth_base_url(&self) -> String {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(url) = self.agent_auth_url_override.read().await.clone() {
            return url;
        }
        super::agent_identity::DEFAULT_AGENT_IDENTITY_AUTH_URL.to_string()
    }

    async fn agent_jwks_base_url(&self) -> String {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(url) = self.agent_jwks_url_override.read().await.clone() {
            return url;
        }
        super::agent_identity::DEFAULT_CHATGPT_BACKEND_URL.to_string()
    }

    pub async fn refresh_account(&self, account_id: &str) -> Result<(), String> {
        let record = self.load_account(account_id).await?;
        let oauth = record
            .oauth()
            .ok_or_else(|| "Agent Identity accounts do not use OAuth token refresh.".to_string())?;
        if oauth.refresh_token.trim().is_empty() {
            return Err("Codex account has no refresh token. Please sign in again.".to_string());
        }
        let refreshed = self.refresh_record_guarded(account_id, record).await?;
        let summary = self.save_record(account_id.to_string(), refreshed).await?;
        if matches!(summary.status, CodexAccountStatus::Expired) {
            return Err("Codex token refresh failed.".to_string());
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

    pub async fn set_auto_refresh(
        &self,
        account_id: &str,
        enabled: bool,
    ) -> Result<CodexAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        let oauth = record.oauth_mut().ok_or_else(|| {
            "Agent Identity accounts do not support automatic token refresh.".to_string()
        })?;
        *oauth.auto_refresh_enabled = enabled;
        self.save_record(account_id.to_string(), record).await
    }

    pub(crate) async fn mark_invalid(
        &self,
        account_id: &str,
    ) -> Result<CodexAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        record.status = CodexAccountStatus::Invalid;
        tracing::warn!(account_id, "codex account marked invalid");
        self.save_record(account_id.to_string(), record).await
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.save_record_unlocked(account_id, record).await
    }

    async fn save_record_unlocked(
        &self,
        account_id: String,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        provider_accounts::upsert_codex_account(&self.paths, &account_id, &record).await?;
        let mut cache = self.cache.write().await;
        cache.insert(account_id.clone(), record.clone());
        Ok(account_summary(account_id, &record))
    }

    /// Seeds an exact record for cross-crate integration tests.
    #[cfg(feature = "test-support")]
    pub async fn save_record_for_test(
        &self,
        account_id: String,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        self.save_record(account_id, record).await
    }

    /// Removes a seeded account for cross-crate integration tests (e.g. missing pin).
    #[cfg(feature = "test-support")]
    pub async fn delete_account_for_test(&self, account_id: &str) -> Result<(), String> {
        self.delete_account(account_id).await
    }

    pub(crate) async fn persist_quota_cache(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        self.save_record(account_id.to_string(), record.clone())
            .await?;
        Ok(record)
    }

    /// 自动取 gate 的新建/覆盖入口；事务内请用 unlocked。
    #[allow(dead_code)]
    pub(crate) async fn save_new_account(
        &self,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.save_new_account_unlocked(record).await
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn save_new_account_unlocked(
        &self,
        mut record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        fill_record_from_jwt(&mut record);
        if let Some((existing_local_account_id, existing_record)) =
            self.find_existing_import_target_unlocked(&record).await?
        {
            // Re-importing the same real Codex account should refresh credentials in place
            // instead of creating duplicate local entries. Keep app-local settings.
            record.status = existing_record.status;
            if let (Some(imported), Some(existing)) = (record.oauth_mut(), existing_record.oauth())
            {
                *imported.auto_refresh_enabled = existing.auto_refresh_enabled;
                if imported.client_id.is_none() {
                    *imported.client_id = existing.client_id.map(str::to_string);
                }
                if imported.openai_device_id.is_none() {
                    *imported.openai_device_id = existing.openai_device_id.map(str::to_string);
                }
            }
            return self
                .save_record_unlocked(existing_local_account_id, record)
                .await;
        }
        let id_part_source = record
            .email
            .as_deref()
            .or(record.account_id.as_deref())
            .unwrap_or_default();
        let mut id_part = sanitize_id_part(id_part_source);
        if id_part.is_empty() {
            id_part = format!("{}", OffsetDateTime::now_utc().unix_timestamp());
        }
        let account_id = self.unique_account_id_unlocked(&id_part).await?;
        self.save_record_unlocked(account_id, record).await
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_records_unlocked(
        &self,
        records: Vec<CodexTokenRecord>,
        source: &str,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        let mut imported = Vec::new();
        for record in records {
            if let Ok(summary) = self.save_new_account_unlocked(record).await {
                imported.push(summary);
            }
        }
        tracing::info!(
            source,
            imported = imported.len(),
            "codex account import finished"
        );
        if imported.is_empty() {
            return Err("No valid Codex accounts found in input.".to_string());
        }
        Ok(imported)
    }

    /// 调用方必须已持 `provider_mutation`（网络 refresh 期间也持锁，防并发 save 插入中间态）。
    async fn import_refresh_token_unlocked(
        &self,
        refresh_token: &str,
        client: CodexRefreshTokenClient,
    ) -> Result<CodexAccountSummary, String> {
        let proxy_url = self.effective_proxy_url(None).await;
        let oauth = CodexOAuthClient::new(proxy_url.as_deref())?;
        let response = oauth
            .refresh_token_with_client(refresh_token, client)
            .await?;
        let mut record = CodexTokenRecord {
            credential: CodexCredential::Oauth {
                access_token: response.access_token,
                refresh_token: if response.refresh_token.trim().is_empty() {
                    refresh_token.to_string()
                } else {
                    response.refresh_token
                },
                client_id: Some(client.client_id().to_string()),
                id_token: response.id_token,
                auto_refresh_enabled: true,
                openai_device_id: None,
                expires_at: expires_at_from_seconds(response.expires_in),
                last_refresh: Some(now_rfc3339()),
            },
            status: CodexAccountStatus::Active,
            account_id: None,
            user_id: None,
            email: None,
            quota: super::types::CodexQuotaCache::default(),
        };
        fill_record_from_jwt(&mut record);
        self.save_new_account_unlocked(record).await
    }

    /// 编排层删除账户凭据；刷新内存 cache。禁止在日志中打印 token。
    pub async fn delete_account(&self, account_id: &str) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        provider_accounts::delete_account(&self.paths, account_id).await?;
        let mut cache = self.cache.write().await;
        cache.remove(account_id);
        tracing::debug!(account_id, "codex account deleted for orchestration");
        Ok(())
    }

    /// 编排层快照：导出全部凭据（含 secret，仅内存短时持有，禁止日志）。
    pub async fn snapshot_all_records(&self) -> Result<HashMap<String, CodexTokenRecord>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.snapshot_all_records_unlocked().await
    }

    /// 编排层恢复：单 SQLite 事务替换该 provider 全量快照，提交后再替换 cache。
    pub async fn restore_all_records(
        &self,
        snapshot: HashMap<String, CodexTokenRecord>,
    ) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        self.restore_all_records_unlocked(snapshot).await
    }

    /// 单账户快照；不存在返回 None。
    pub async fn snapshot_account_record(
        &self,
        account_id: &str,
    ) -> Result<Option<CodexTokenRecord>, String> {
        let _gate = self.acquire_provider_mutation().await;
        self.reload_cache_unlocked().await?;
        Ok(self.cache.read().await.get(account_id).cloned())
    }

    /// 单账户原子恢复；`None` 表示删除。DB 提交成功后再改 cache。
    pub async fn restore_account_record(
        &self,
        account_id: &str,
        record: Option<CodexTokenRecord>,
    ) -> Result<(), String> {
        let _gate = self.acquire_provider_mutation().await;
        provider_accounts::replace_codex_account(&self.paths, account_id, record.as_ref()).await?;
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
        tracing::debug!(
            account_id,
            "codex account record restored for orchestration"
        );
        Ok(())
    }

    /// 登录编排提交：持久化 credential，返回 summary 与覆盖前旧 record。
    pub async fn commit_login_record(
        &self,
        mut record: CodexTokenRecord,
    ) -> Result<(CodexAccountSummary, Option<CodexTokenRecord>), String> {
        // 先补全 JWT 身份，再查旧 record，保证 re-login 补偿能命中同一账户。
        // 整段持 provider gate，避免与 restore/refresh 交错。
        let _gate = self.acquire_provider_mutation().await;
        fill_record_from_jwt(&mut record);
        let previous = self
            .find_existing_import_target_unlocked(&record)
            .await?
            .map(|(_, old)| old);
        let summary = self.save_new_account_unlocked(record).await?;
        Ok((summary, previous))
    }

    pub async fn refresh_due_accounts(&self) -> Result<Vec<String>, String> {
        self.refresh_due_accounts_with_token_url(None).await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn set_test_token_url(&self, token_url: &str) {
        let mut guard = self.token_url_override.write().await;
        *guard = Some(token_url.to_string());
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn set_test_agent_identity_urls(&self, auth_url: &str, jwks_url: &str) {
        *self.agent_auth_url_override.write().await = Some(auth_url.to_string());
        *self.agent_jwks_url_override.write().await = Some(jwks_url.to_string());
    }

    async fn refresh_if_needed(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        // 禁用账号不参与调度，读取路径也不应触发 token refresh。
        if matches!(record.status, CodexAccountStatus::Invalid) {
            tracing::debug!(account_id, "skip codex token refresh for disabled account");
            return Ok(record);
        }
        if !record_needs_refresh(&record) {
            return Ok(record);
        }
        let Some(oauth) = record.oauth() else {
            return Ok(record);
        };
        if !oauth.auto_refresh_enabled {
            return Ok(record);
        }
        // Allow imported access-token-only records to stay usable until expiry.
        // When refresh_token is missing we should not fail reads/listing by forcing refresh.
        if oauth.refresh_token.trim().is_empty() {
            return Ok(record);
        }
        self.refresh_record_guarded(account_id, record).await
    }

    async fn refresh_due_accounts_with_token_url(
        &self,
        token_url: Option<&str>,
    ) -> Result<Vec<String>, String> {
        self.refresh_cache().await?;
        let candidates = {
            let cache = self.cache.read().await;
            sorted_account_ids(&cache)
        };

        let mut refreshed = Vec::new();
        let mut last_error = None;
        for account_id in candidates {
            let record = self.load_account(&account_id).await?;
            if !record_can_auto_refresh(&record) || !record_needs_refresh(&record) {
                continue;
            }
            let result = match token_url {
                Some(token_url) => {
                    self.refresh_record_with_token_url(&account_id, record, token_url)
                        .await
                }
                None => self.refresh_record_guarded(&account_id, record).await,
            };
            match result {
                Ok(_) => refreshed.push(account_id),
                Err(err) => {
                    tracing::warn!(
                        account_id,
                        error = %err,
                        "codex due account refresh failed"
                    );
                    last_error = Some(err);
                }
            }
        }

        if refreshed.is_empty() {
            if let Some(err) = last_error {
                return Err(err);
            }
        }
        Ok(refreshed)
    }

    async fn refresh_record_guarded(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        let Some(_permit) = self.start_token_refresh(account_id) else {
            tracing::debug!(
                account_id,
                "codex account refresh already in progress; waiting for refreshed token"
            );
            return self.wait_for_token_refresh(account_id, &record).await;
        };
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record_with_token_url(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
        token_url: &str,
    ) -> Result<CodexTokenRecord, String> {
        let Some(_permit) = self.start_token_refresh(account_id) else {
            tracing::debug!(
                account_id,
                "codex account refresh already in progress; waiting for refreshed token"
            );
            return self.wait_for_token_refresh(account_id, &record).await;
        };
        let result = self
            .refresh_record_inner(account_id, record, Some(token_url))
            .await;
        result
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        self.refresh_record_inner(account_id, record, None).await
    }

    async fn refresh_record_inner(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
        token_url: Option<&str>,
    ) -> Result<CodexTokenRecord, String> {
        let oauth = record
            .oauth()
            .ok_or_else(|| "Agent Identity accounts do not use OAuth token refresh.".to_string())?;
        let refresh_token = oauth.refresh_token.to_string();
        let id_token = oauth.id_token.to_string();
        let auto_refresh_enabled = oauth.auto_refresh_enabled;
        let openai_device_id = oauth.openai_device_id.map(str::to_string);
        let proxy_url = self.effective_proxy_url(None).await;
        let client = self.oauth_client(proxy_url.as_deref(), token_url).await?;
        let refresh_client = refresh_token_client_for_record(&record)?;
        tracing::debug!(
            account_id,
            client = refresh_client.as_str(),
            "codex account refresh start"
        );
        let response = match client
            .refresh_token_with_client(&refresh_token, refresh_client)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if error_requires_relogin(&err) {
                    if let Err(mark_err) = self.mark_invalid(account_id).await {
                        tracing::warn!(
                            account_id,
                            error = %mark_err,
                            "codex account invalid mark failed"
                        );
                    }
                }
                return Err(err);
            }
        };
        let mut refreshed = CodexTokenRecord {
            credential: CodexCredential::Oauth {
                access_token: response.access_token,
                refresh_token: if response.refresh_token.trim().is_empty() {
                    refresh_token
                } else {
                    response.refresh_token
                },
                client_id: Some(refresh_client.client_id().to_string()),
                id_token: if response.id_token.trim().is_empty() {
                    id_token
                } else {
                    response.id_token
                },
                auto_refresh_enabled,
                openai_device_id,
                expires_at: expires_at_from_seconds(response.expires_in),
                last_refresh: Some(now_rfc3339()),
            },
            status: record.status,
            account_id: record.account_id.clone(),
            user_id: record.user_id.clone(),
            email: record.email.clone(),
            quota: record.quota.clone(),
        };
        fill_record_from_jwt(&mut refreshed);
        let summary = self
            .save_record(account_id.to_string(), refreshed.clone())
            .await?;
        if matches!(summary.status, CodexAccountStatus::Expired) {
            return Err("Codex token refresh failed.".to_string());
        }
        Ok(refreshed)
    }

    pub(crate) async fn load_account(&self, account_id: &str) -> Result<CodexTokenRecord, String> {
        if let Some(record) = self.cache.read().await.get(account_id).cloned() {
            return Ok(record);
        }
        self.refresh_cache().await?;
        self.cache
            .read()
            .await
            .get(account_id)
            .cloned()
            .ok_or_else(|| format!("Codex account not found: {account_id}"))
    }

    /// Reads the cached record without refresh for cross-crate integration tests.
    #[cfg(feature = "test-support")]
    pub async fn load_account_for_test(
        &self,
        account_id: &str,
    ) -> Result<CodexTokenRecord, String> {
        self.load_account(account_id).await
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

    #[cfg_attr(not(test), allow(dead_code))]
    /// 解析 Upstream 固定引用的账户凭据。无 pool/unpinned 选号。
    pub async fn resolve_pinned_account_record(
        &self,
        account_id: &str,
    ) -> Result<(String, CodexTokenRecord), String> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err(
                "Codex account_id is required for account credential upstream.".to_string(),
            );
        }
        let record = self.get_account_record(account_id).await?;
        if !record.is_usable() {
            tracing::warn!(
                account_id,
                status = ?record.effective_status(),
                "pinned codex account is not usable"
            );
            return Err(format!(
                "Codex account is {}: {account_id}",
                match record.effective_status() {
                    CodexAccountStatus::Active => "active",
                    CodexAccountStatus::Expired => "expired",
                    CodexAccountStatus::Invalid => "invalid",
                }
            ));
        }
        tracing::debug!(account_id, "resolved pinned codex account credential");
        Ok((account_id.to_string(), record))
    }

    pub(crate) async fn effective_proxy_url(&self, proxy_url: Option<&str>) -> Option<String> {
        match normalize_proxy_url(proxy_url) {
            Ok(Some(proxy_url)) => Some(proxy_url),
            Ok(None) | Err(_) => self.app_proxy_url().await,
        }
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

    fn start_token_refresh(&self, account_id: &str) -> Option<TokenRefreshPermit<'_>> {
        let mut refreshing = self
            .token_refreshing
            .lock()
            .expect("codex token refresh lock poisoned");
        if refreshing.contains(account_id) {
            return None;
        }
        refreshing.insert(account_id.to_string());
        Some(TokenRefreshPermit {
            refreshing: &self.token_refreshing,
            account_id: account_id.to_string(),
        })
    }

    fn token_refresh_in_progress(&self, account_id: &str) -> bool {
        self.token_refreshing
            .lock()
            .expect("codex token refresh lock poisoned")
            .contains(account_id)
    }

    async fn wait_for_token_refresh(
        &self,
        account_id: &str,
        previous: &CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        let deadline = tokio::time::Instant::now() + CODEX_TOKEN_REFRESH_WAIT_TIMEOUT;
        loop {
            if !self.token_refresh_in_progress(account_id) {
                let record = self.load_account(account_id).await?;
                if token_record_was_refreshed(previous, &record) {
                    return Ok(record);
                }
                return Err(format!(
                    "Codex account refresh did not update credentials: {account_id}"
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "Codex account refresh is still in progress: {account_id}"
                ));
            }
            tokio::time::sleep(CODEX_TOKEN_REFRESH_WAIT_STEP).await;
        }
    }

    async fn oauth_client(
        &self,
        proxy_url: Option<&str>,
        token_url: Option<&str>,
    ) -> Result<CodexOAuthClient, String> {
        if let Some(token_url) = token_url {
            return CodexOAuthClient::new_with_token_url(proxy_url, token_url);
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            if let Some(token_url) = self.token_url_override.read().await.as_deref() {
                return CodexOAuthClient::new_with_token_url(proxy_url, token_url);
            }
        }
        CodexOAuthClient::new(proxy_url)
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

    async fn reload_cache_unlocked(&self) -> Result<(), String> {
        let cache = provider_accounts::list_codex_records(&self.paths).await?;
        *self.cache.write().await = cache;
        Ok(())
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn snapshot_all_records_unlocked(
        &self,
    ) -> Result<HashMap<String, CodexTokenRecord>, String> {
        self.reload_cache_unlocked().await?;
        Ok(self.cache.read().await.clone())
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn restore_all_records_unlocked(
        &self,
        snapshot: HashMap<String, CodexTokenRecord>,
    ) -> Result<(), String> {
        provider_accounts::replace_all_codex_records(&self.paths, &snapshot).await?;
        *self.cache.write().await = snapshot;
        tracing::debug!("codex account store restored from orchestration snapshot");
        Ok(())
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_file_unlocked(
        &self,
        path: PathBuf,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        if path.as_os_str().is_empty() {
            return Err("Import path is required.".to_string());
        }
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err("Selected import path not found.".to_string());
        }
        let mut imported = Vec::new();
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|err| format!("Failed to read import path metadata: {err}"))?;
        let candidate_files = if metadata.is_dir() {
            collect_json_files(&path).await?
        } else {
            vec![path.clone()]
        };

        for file_path in candidate_files {
            let contents = match tokio::fs::read_to_string(&file_path).await {
                Ok(contents) => contents,
                Err(err) if metadata.is_dir() => {
                    let _ = err;
                    continue;
                }
                Err(err) => return Err(format!("Failed to read JSON file: {err}")),
            };
            let records = match self.parse_import_records(&contents).await {
                Ok(records) => records,
                Err(err) if metadata.is_dir() => {
                    let _ = err;
                    continue;
                }
                Err(err) => return Err(err),
            };
            for record in records {
                if let Ok(summary) = self.save_new_account_unlocked(record).await {
                    imported.push(summary);
                }
            }
        }
        if imported.is_empty() {
            return Err(if metadata.is_dir() {
                "No valid Codex accounts found in selected directory.".to_string()
            } else {
                "No valid Codex accounts found in JSON file.".to_string()
            });
        }
        Ok(imported)
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_text_unlocked(
        &self,
        contents: &str,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        let records = self.parse_import_records(contents).await?;
        self.import_records_unlocked(records, "text").await
    }

    /// 调用方必须已持 `provider_mutation`。
    async fn import_refresh_tokens_unlocked(
        &self,
        contents: &str,
        client: CodexRefreshTokenClient,
    ) -> Result<Vec<CodexAccountSummary>, String> {
        let refresh_tokens = parse_refresh_token_lines(contents)?;
        if refresh_tokens.is_empty() {
            return Err("Refresh token is required.".to_string());
        }

        let mut imported = Vec::new();
        let mut errors = Vec::new();
        for refresh_token in refresh_tokens {
            match self
                .import_refresh_token_unlocked(refresh_token.as_str(), client)
                .await
            {
                Ok(summary) => imported.push(summary),
                Err(err) => errors.push(err),
            }
        }

        tracing::info!(
            client = client.as_str(),
            imported = imported.len(),
            failed = errors.len(),
            "codex refresh token import finished"
        );
        if imported.is_empty() {
            return Err(errors.into_iter().next().unwrap_or_else(|| {
                "No valid Codex accounts found in refresh token input.".to_string()
            }));
        }
        Ok(imported)
    }

    async fn unique_account_id_unlocked(&self, id_part: &str) -> Result<String, String> {
        self.reload_cache_unlocked().await?;
        let cache = self.cache.read().await;
        let mut suffix = 0u32;
        loop {
            let candidate = if suffix == 0 {
                format!("codex-{id_part}.json")
            } else {
                format!("codex-{id_part}-{suffix}.json")
            };
            if !cache.contains_key(&candidate) {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }

    async fn find_existing_import_target_unlocked(
        &self,
        imported: &CodexTokenRecord,
    ) -> Result<Option<(String, CodexTokenRecord)>, String> {
        self.reload_cache_unlocked().await?;
        let imported_account_id = normalize_optional_id(imported.account_id.as_deref());
        let imported_user_id = normalize_optional_id(imported.user_id.as_deref());
        let imported_email = normalize_optional_email(imported.email.as_deref());
        let cache = self.cache.read().await;

        if let Some(user_id) = imported_user_id.as_ref() {
            if let Some((local_account_id, existing_record)) =
                cache.iter().find(|(_, existing_record)| {
                    if normalize_optional_id(existing_record.user_id.as_deref()).as_deref()
                        != Some(user_id.as_str())
                    {
                        return false;
                    }
                    account_ids_are_compatible(imported_account_id.as_deref(), existing_record)
                })
            {
                tracing::debug!(
                    identity = "user",
                    "codex import reuses existing local account"
                );
                return Ok(Some((local_account_id.clone(), existing_record.clone())));
            }
        }

        if let Some(email) = imported_email.as_ref() {
            if let Some((local_account_id, existing_record)) =
                cache.iter().find(|(_, existing_record)| {
                    let existing_email = normalize_optional_email(existing_record.email.as_deref());
                    if existing_email.as_deref() != Some(email.as_str()) {
                        return false;
                    }
                    let existing_user_id =
                        normalize_optional_id(existing_record.user_id.as_deref());
                    if matches!(
                        (&imported_user_id, existing_user_id.as_deref()),
                        (Some(imported_user_id), Some(existing_user_id))
                            if imported_user_id != existing_user_id
                    ) {
                        return false;
                    }
                    if imported_user_id.is_some() && existing_user_id.is_some() {
                        return false;
                    }
                    // Email fallback is only safe when the workspace context is also unambiguous:
                    // both account ids match, or both sides lack an account id.
                    account_ids_are_compatible(imported_account_id.as_deref(), existing_record)
                })
            {
                tracing::debug!(
                    identity = "email",
                    "codex import reuses existing local account"
                );
                return Ok(Some((local_account_id.clone(), existing_record.clone())));
            }
        }

        Ok(None)
    }
}

fn account_summary(account_id: String, record: &CodexTokenRecord) -> CodexAccountSummary {
    CodexAccountSummary {
        account_id,
        email: record.email.clone(),
        expires_at: record.expires_at().map(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| record.expires_at_str().unwrap_or_default().to_string())
        }),
        status: record.effective_status(),
        auth_method: record.auth_method(),
        auto_refresh_enabled: record.auto_refresh_enabled(),
    }
}

fn task_is_current(current_task_id: Option<&str>, expected_task_id: Option<&str>) -> bool {
    let current = current_task_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match expected_task_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        None => current.is_some(),
        Some(expected) => current.is_some_and(|current| current != expected),
    }
}

fn normalize_optional_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_optional_email(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn account_ids_are_compatible(
    imported_account_id: Option<&str>,
    existing_record: &CodexTokenRecord,
) -> bool {
    let existing_account_id = normalize_optional_id(existing_record.account_id.as_deref());
    match (imported_account_id, existing_account_id.as_deref()) {
        (Some(imported_account_id), Some(existing_account_id)) => {
            imported_account_id == existing_account_id
        }
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

fn sorted_account_ids(cache: &HashMap<String, CodexTokenRecord>) -> Vec<String> {
    let mut account_ids = cache.keys().cloned().collect::<Vec<_>>();
    account_ids.sort();
    account_ids
}

const QUOTA_REFRESH_INTERVAL_SECONDS: i64 = 30;

fn quota_refresh_is_due(checked_at: Option<&str>) -> bool {
    let Some(checked_at) = checked_at.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Ok(checked_at) =
        OffsetDateTime::parse(checked_at, &time::format_description::well_known::Rfc3339)
    else {
        return true;
    };
    OffsetDateTime::now_utc() - checked_at >= Duration::seconds(QUOTA_REFRESH_INTERVAL_SECONDS)
}

async fn collect_json_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();

    // Recursive async traversal keeps directory import compatible with nested auth mirrors.
    while let Some(directory) = directories.pop() {
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .map_err(|err| format!("Failed to read import directory: {err}"))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read import directory entry: {err}"))?
        {
            let entry_path = entry.path();
            let entry_type = entry
                .file_type()
                .await
                .map_err(|err| format!("Failed to inspect import path: {err}"))?;
            if entry_type.is_dir() {
                directories.push(entry_path);
                continue;
            }
            if !entry_type.is_file() {
                continue;
            }
            let Some(extension) = entry_path.extension().and_then(|value| value.to_str()) else {
                continue;
            };
            if extension.eq_ignore_ascii_case("json") {
                files.push(entry_path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn fill_record_from_jwt(record: &mut CodexTokenRecord) {
    let Some(oauth) = record.oauth() else {
        return;
    };
    let id_token = oauth.id_token.to_string();
    let access_token = oauth.access_token.to_string();
    // JWT claims are the authority; wrapper fields from exported files are fallbacks.
    if let Some(account_id) = extract_chatgpt_account_id_from_jwt(&id_token)
        .or_else(|| extract_chatgpt_account_id_from_jwt(&access_token))
    {
        record.account_id = Some(account_id);
    }
    if let Some(user_id) = extract_chatgpt_user_id_from_jwt(&id_token)
        .or_else(|| extract_chatgpt_user_id_from_jwt(&access_token))
    {
        record.user_id = Some(user_id);
    }
    if let Some(email) =
        extract_email_from_jwt(&id_token).or_else(|| extract_email_from_jwt(&access_token))
    {
        record.email = Some(email);
    }
}

fn record_needs_refresh(record: &CodexTokenRecord) -> bool {
    record_expires_within(record, CODEX_TOKEN_REFRESH_WINDOW)
        || paid_quota_disagrees_with_free_access_token_claim(record)
}

fn record_can_auto_refresh(record: &CodexTokenRecord) -> bool {
    let Some(oauth) = record.oauth() else {
        return false;
    };
    matches!(record.status, CodexAccountStatus::Active)
        && oauth.auto_refresh_enabled
        && !oauth.refresh_token.trim().is_empty()
}

fn record_expires_within(record: &CodexTokenRecord, window: Duration) -> bool {
    let Some(expires_at) = record.expires_at() else {
        return true;
    };
    OffsetDateTime::now_utc() + window >= expires_at
}

fn token_record_was_refreshed(previous: &CodexTokenRecord, current: &CodexTokenRecord) -> bool {
    match (previous.oauth(), current.oauth()) {
        (Some(previous), Some(current)) => {
            current.access_token != previous.access_token
                || current.refresh_token != previous.refresh_token
                || current.last_refresh != previous.last_refresh
                || current.expires_at != previous.expires_at
        }
        _ => false,
    }
}

fn refresh_token_client_for_record(
    record: &CodexTokenRecord,
) -> Result<CodexRefreshTokenClient, String> {
    let oauth = record
        .oauth()
        .ok_or_else(|| "Agent Identity accounts do not use an OAuth refresh client.".to_string())?;
    let Some(client_id) = oauth.client_id else {
        return Ok(CodexRefreshTokenClient::Codex);
    };
    CodexRefreshTokenClient::from_client_id(client_id).ok_or_else(|| {
        format!(
            "Unsupported Codex refresh token client_id: {}",
            client_id.trim()
        )
    })
}

fn paid_quota_disagrees_with_free_access_token_claim(record: &CodexTokenRecord) -> bool {
    if !is_paid_plan(record.quota.plan_type.as_deref()) {
        return false;
    }
    record.oauth().is_some_and(|oauth| {
        matches!(
            extract_chatgpt_plan_type_from_jwt(oauth.access_token).as_deref(),
            Some("free")
        )
    })
}

fn is_paid_plan(plan_type: Option<&str>) -> bool {
    let Some(plan_type) = plan_type.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    !plan_type.eq_ignore_ascii_case("free")
}

fn extract_chatgpt_plan_type_from_jwt(token: &str) -> Option<String> {
    let value = decode_jwt_payload(token)?;
    value
        .get("https://api.openai.com/auth")
        .and_then(|value| value.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

enum AgentIdentityImport {
    Jwt(String),
    Record(RawAgentIdentity),
}

#[derive(Deserialize)]
struct RawAgentIdentity {
    agent_runtime_id: String,
    agent_private_key: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(alias = "chatgpt_account_id")]
    account_id: String,
    chatgpt_user_id: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_plan_type")]
    plan_type: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
}

impl RawAgentIdentity {
    fn into_record(self) -> Result<CodexTokenRecord, String> {
        let account_id =
            required_nonempty(self.account_id, "Agent Identity account ID is required.")?;
        let user_id = required_nonempty(
            self.chatgpt_user_id,
            "Agent Identity ChatGPT user ID is required.",
        )?;
        let plan_type = self.plan_type.and_then(optional_nonempty);
        let record = CodexTokenRecord {
            credential: CodexCredential::AgentIdentity {
                agent_runtime_id: required_nonempty(
                    self.agent_runtime_id,
                    "Agent Identity runtime ID is required.",
                )?,
                agent_private_key: self.agent_private_key,
                task_id: self.task_id.and_then(optional_nonempty),
                plan_type: plan_type.clone(),
                chatgpt_account_is_fedramp: self.chatgpt_account_is_fedramp,
            },
            status: CodexAccountStatus::Active,
            account_id: Some(account_id),
            user_id: Some(user_id),
            email: self.email.and_then(optional_nonempty),
            quota: super::types::CodexQuotaCache {
                plan_type,
                ..Default::default()
            },
        };
        super::agent_identity::validate_identity(
            record
                .agent_identity()
                .expect("new Agent Identity record has Agent Identity credential"),
        )?;
        Ok(record)
    }
}

fn parse_agent_identity_candidates(
    contents: &str,
) -> Result<Option<Vec<AgentIdentityImport>>, String> {
    let trimmed = contents.trim();
    if trimmed.is_empty() || !looks_like_json(trimmed) {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(trimmed)
        .map_err(|error| format!("Invalid Codex account JSON file: {error}"))?;
    let mut candidates = Vec::new();
    collect_agent_identity_candidates(&value, &mut candidates)?;
    Ok((!candidates.is_empty()).then_some(candidates))
}

fn collect_agent_identity_candidates(
    value: &Value,
    candidates: &mut Vec<AgentIdentityImport>,
) -> Result<(), String> {
    if let Some(items) = value.as_array() {
        for item in items {
            collect_agent_identity_candidates(item, candidates)?;
        }
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if let Some(identity) = object
        .get("agent_identity")
        .or_else(|| object.get("agentIdentity"))
    {
        match identity {
            Value::String(jwt) if !jwt.trim().is_empty() => {
                candidates.push(AgentIdentityImport::Jwt(jwt.trim().to_string()));
                return Ok(());
            }
            Value::Object(_) => {
                let raw = serde_json::from_value::<RawAgentIdentity>(identity.clone())
                    .map_err(|_| "Agent Identity record is missing required fields.".to_string())?;
                candidates.push(AgentIdentityImport::Record(raw));
                return Ok(());
            }
            Value::Null => {}
            _ => return Err("Agent Identity must be a JWT or record object.".to_string()),
        }
    }
    let is_agent_identity = object
        .get("auth_mode")
        .or_else(|| object.get("authMode"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("agentIdentity"));
    if is_agent_identity {
        let raw = serde_json::from_value::<RawAgentIdentity>(value.clone())
            .map_err(|_| "Agent Identity record is missing required fields.".to_string())?;
        candidates.push(AgentIdentityImport::Record(raw));
        return Ok(());
    }
    for key in ["accounts", "auths", "items", "data"] {
        if let Some(child) = object.get(key) {
            collect_agent_identity_candidates(child, candidates)?;
        }
    }
    Ok(())
}

fn deserialize_optional_plan_type<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(super::agent_identity::normalize_plan_type(&value))
}

fn required_nonempty(value: String, error: &str) -> Result<String, String> {
    optional_nonempty(value).ok_or_else(|| error.to_string())
}

fn optional_nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_oauth_import_records(contents: &str) -> Result<Vec<CodexTokenRecord>, String> {
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Err("Codex account input is required.".to_string());
    }

    if looks_like_json(trimmed) {
        match parse_json_stream_records(trimmed) {
            Ok(records) => return Ok(records),
            Err(err) if trimmed.contains('\n') => {
                if let Ok(records) = parse_import_lines(trimmed, Some(&err)) {
                    return Ok(records);
                }
                return Err(format!("Invalid Codex account JSON file: {err}"));
            }
            Err(err) => return Err(format!("Invalid Codex account JSON file: {err}")),
        }
    }

    parse_import_lines(trimmed, None)
}

fn looks_like_json(value: &str) -> bool {
    let trimmed = value.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

fn parse_json_stream_records(contents: &str) -> Result<Vec<CodexTokenRecord>, serde_json::Error> {
    let mut records = Vec::new();
    let stream = serde_json::Deserializer::from_str(contents).into_iter::<Value>();
    for value in stream {
        collect_import_records(&value?, &mut records);
    }
    Ok(records)
}

fn parse_import_lines(
    contents: &str,
    json_error: Option<&serde_json::Error>,
) -> Result<Vec<CodexTokenRecord>, String> {
    let mut records = Vec::new();
    for (index, line) in contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        if looks_like_json(line) {
            let parsed = serde_json::from_str::<Value>(line).map_err(|err| {
                format!("Invalid Codex account JSON on line {}: {err}", index + 1)
            })?;
            collect_import_records(&parsed, &mut records);
            continue;
        }
        records.push(raw_access_token_record(line, json_error)?);
    }
    Ok(records)
}

fn collect_import_records(value: &Value, records: &mut Vec<CodexTokenRecord>) {
    if let Some(record) = parse_import_record(value) {
        records.push(record);
        return;
    }

    if let Some(object) = value.as_object() {
        if let Some(data) = object.get("data") {
            if data.is_object() {
                collect_import_records(data, records);
            }
        }

        for key in ["key", "credential", "credentials"] {
            let Some(text) = object.get(key).and_then(Value::as_str) else {
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            collect_import_records(&parsed, records);
        }
    }

    if let Some(items) = value.as_array() {
        for item in items {
            collect_import_records(item, records);
        }
        return;
    }

    for key in ["accounts", "auths", "items", "data"] {
        let Some(items) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            collect_import_records(item, records);
        }
    }
}

fn parse_refresh_token_lines(contents: &str) -> Result<Vec<String>, String> {
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect())
}

fn raw_access_token_record(
    access_token: &str,
    json_error: Option<&serde_json::Error>,
) -> Result<CodexTokenRecord, String> {
    let expires_at = jwt_expires_at(access_token).ok_or_else(|| {
        if let Some(json_error) = json_error {
            return format!(
                "Invalid Codex account JSON file: {json_error}. Raw access token input must include a JWT exp claim."
            );
        }
        "Raw access token input must include a JWT exp claim.".to_string()
    })?;
    let mut record = CodexTokenRecord {
        credential: CodexCredential::Oauth {
            access_token: access_token.to_string(),
            refresh_token: String::new(),
            client_id: None,
            id_token: String::new(),
            auto_refresh_enabled: false,
            openai_device_id: None,
            expires_at,
            last_refresh: Some(now_rfc3339()),
        },
        status: CodexAccountStatus::Active,
        account_id: None,
        user_id: None,
        email: None,
        quota: super::types::CodexQuotaCache::default(),
    };
    fill_record_from_jwt(&mut record);
    Ok(record)
}

fn parse_import_record(value: &Value) -> Option<CodexTokenRecord> {
    let provider = find_string(value, &[&["type"], &["provider"], &["kind"]]);
    if let Some(provider) = provider {
        if !provider.eq_ignore_ascii_case("codex") {
            return None;
        }
    }

    let access_token = find_string(
        value,
        &[
            &["access_token"],
            &["accessToken"],
            &["tokens", "access_token"],
            &["tokens", "accessToken"],
            &["token", "access_token"],
            &["token", "accessToken"],
            &["token_data", "access_token"],
            &["token_data", "accessToken"],
        ],
    )?;
    let refresh_token = find_string(
        value,
        &[
            &["refresh_token"],
            &["refreshToken"],
            &["tokens", "refresh_token"],
            &["tokens", "refreshToken"],
            &["token", "refresh_token"],
            &["token", "refreshToken"],
            &["token_data", "refresh_token"],
            &["token_data", "refreshToken"],
        ],
    )
    .unwrap_or_default();
    let id_token = find_string(
        value,
        &[
            &["id_token"],
            &["idToken"],
            &["tokens", "id_token"],
            &["tokens", "idToken"],
            &["token", "id_token"],
            &["token", "idToken"],
            &["token_data", "id_token"],
            &["token_data", "idToken"],
        ],
    )
    .unwrap_or_default();
    let auto_refresh_enabled = find_bool(
        value,
        &[
            &["auto_refresh_enabled"],
            &["auto_refresh"],
            &["token_data", "auto_refresh_enabled"],
        ],
    )
    .unwrap_or(false);
    let expires_at = find_rfc3339_or_unix_timestamp(
        value,
        &[
            &["expires_at"],
            &["expired"],
            &["tokens", "expires_at"],
            &["tokens", "expiresAt"],
            &["tokens", "expired"],
            &["token", "expires_at"],
            &["token", "expiresAt"],
            &["token", "expired"],
            &["token_data", "expires_at"],
            &["token_data", "expiresAt"],
            &["token_data", "expired"],
        ],
    )
    .or_else(|| {
        find_i64(
            value,
            &[
                &["expires_in"],
                &["expiresIn"],
                &["tokens", "expires_in"],
                &["tokens", "expiresIn"],
                &["token", "expires_in"],
                &["token", "expiresIn"],
                &["token_data", "expires_in"],
                &["token_data", "expiresIn"],
            ],
        )
        .map(expires_at_from_seconds)
    })?;

    let account_id = find_string(
        value,
        &[
            &["account_id"],
            &["chatgpt_account_id"],
            &["tokens", "chatgpt_account_id"],
            &["account", "uuid"],
            &["account", "id"],
            &["token_data", "account_id"],
            &["token_data", "chatgpt_account_id"],
            &["data", "account_id"],
            &["data", "chatgpt_account_id"],
        ],
    );
    let user_id = find_string(
        value,
        &[
            &["user_id"],
            &["chatgpt_user_id"],
            &["tokens", "chatgpt_user_id"],
            &["user", "id"],
            &["user", "uuid"],
            &["account", "user_id"],
            &["token_data", "user_id"],
            &["token_data", "chatgpt_user_id"],
            &["data", "user_id"],
            &["data", "chatgpt_user_id"],
        ],
    );
    let email = find_string(
        value,
        &[
            &["email"],
            &["account", "email_address"],
            &["account", "email"],
            &["user", "email"],
            &["token_data", "email"],
            &["data", "email"],
        ],
    );
    let last_refresh = find_string(
        value,
        &[
            &["last_refresh"],
            &["lastRefresh"],
            &["last_refreshed_at"],
            &["lastRefreshedAt"],
            &["data", "last_refresh"],
            &["token_data", "last_refresh"],
        ],
    )
    .or_else(|| Some(now_rfc3339()));
    let client_id = find_string(
        value,
        &[
            &["client_id"],
            &["clientId"],
            &["tokens", "client_id"],
            &["tokens", "clientId"],
            &["token", "client_id"],
            &["token", "clientId"],
            &["token_data", "client_id"],
            &["token_data", "clientId"],
            &["data", "client_id"],
            &["data", "clientId"],
        ],
    );
    let openai_device_id = find_string(
        value,
        &[
            &["openai_device_id"],
            &["openaiDeviceId"],
            &["device_id"],
            &["deviceId"],
            &["tokens", "openai_device_id"],
            &["tokens", "openaiDeviceId"],
            &["tokens", "device_id"],
            &["tokens", "deviceId"],
            &["token", "openai_device_id"],
            &["token", "openaiDeviceId"],
            &["token", "device_id"],
            &["token", "deviceId"],
            &["token_data", "openai_device_id"],
            &["token_data", "openaiDeviceId"],
            &["token_data", "device_id"],
            &["token_data", "deviceId"],
            &["data", "openai_device_id"],
            &["data", "openaiDeviceId"],
            &["data", "device_id"],
            &["data", "deviceId"],
        ],
    );

    Some(CodexTokenRecord {
        credential: CodexCredential::Oauth {
            access_token,
            refresh_token,
            client_id,
            id_token,
            auto_refresh_enabled,
            openai_device_id,
            expires_at,
            last_refresh,
        },
        status: CodexAccountStatus::Active,
        account_id,
        user_id,
        email,
        quota: super::types::CodexQuotaCache::default(),
    })
}

fn find_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let Some(candidate) = value_at_path(value, path) else {
            continue;
        };
        let Some(text) = candidate.as_str() else {
            continue;
        };
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn find_i64(value: &Value, paths: &[&[&str]]) -> Option<i64> {
    for path in paths {
        let Some(candidate) = value_at_path(value, path) else {
            continue;
        };
        if let Some(number) = candidate.as_i64() {
            return Some(number);
        }
        if let Some(text) = candidate.as_str() {
            if let Ok(number) = text.trim().parse::<i64>() {
                return Some(number);
            }
        }
    }
    None
}

fn find_bool(value: &Value, paths: &[&[&str]]) -> Option<bool> {
    for path in paths {
        let Some(candidate) = value_at_path(value, path) else {
            continue;
        };
        if let Some(flag) = candidate.as_bool() {
            return Some(flag);
        }
        if let Some(text) = candidate.as_str() {
            let normalized = text.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "true" | "1" => return Some(true),
                "false" | "0" => return Some(false),
                _ => {}
            }
        }
    }
    None
}

fn find_rfc3339_or_unix_timestamp(value: &Value, paths: &[&[&str]]) -> Option<String> {
    if let Some(text) = find_string(value, paths) {
        return Some(text);
    }
    find_i64(value, paths).and_then(format_unix_timestamp)
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn format_unix_timestamp(value: i64) -> Option<String> {
    let (seconds, nanos) = if value >= 10_000_000_000 {
        let secs = value / 1000;
        let ms = value % 1000;
        (secs, ms * 1_000_000)
    } else {
        (value, 0)
    };
    let total_nanos = i128::from(seconds)
        .checked_mul(1_000_000_000)?
        .checked_add(i128::from(nanos))?;
    OffsetDateTime::from_unix_timestamp_nanos(total_nanos)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn jwt_expires_at(token: &str) -> Option<String> {
    let exp = decode_jwt_payload(token)?
        .get("exp")
        .and_then(Value::as_i64)?;
    format_unix_timestamp(exp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodexQuotaCache;
    use axum::response::IntoResponse;
    use base64::engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD};
    use base64::Engine;
    use ed25519_dalek::pkcs8::EncodePrivateKey as _;
    use ed25519_dalek::SigningKey;
    use rand::random;
    use serde_json::json;
    use std::future::Future;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use time::format_description::well_known::Rfc3339;
    use token_proxy_account_store::app_proxy;
    use token_proxy_account_store::paths::TokenProxyPaths;

    fn run_async(test: impl Future<Output = ()>) {
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(test);
    }

    fn create_test_store() -> (CodexAccountStore, PathBuf) {
        let data_dir =
            std::env::temp_dir().join(format!("token-proxy-codex-store-test-{}", random::<u64>()));
        std::fs::create_dir_all(&data_dir).expect("create test data dir");
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
        let store = CodexAccountStore::new(&paths, app_proxy::new_state()).expect("codex store");
        (store, data_dir)
    }

    fn build_id_token(email: &str, account_id: &str) -> String {
        build_id_token_with_user(email, account_id, None)
    }

    fn build_id_token_with_user(email: &str, account_id: &str, user_id: Option<&str>) -> String {
        build_id_token_with_user_claim(email, account_id, "chatgpt_user_id", user_id)
    }

    fn build_id_token_with_user_claim(
        email: &str,
        account_id: &str,
        user_claim_name: &str,
        user_id: Option<&str>,
    ) -> String {
        let mut auth = serde_json::Map::new();
        auth.insert(
            "chatgpt_account_id".to_string(),
            Value::String(account_id.to_string()),
        );
        if let Some(user_id) = user_id {
            auth.insert(
                user_claim_name.to_string(),
                Value::String(user_id.to_string()),
            );
        }
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": Value::Object(auth),
        });
        let encoded =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
        format!("header.{encoded}.signature")
    }

    fn build_id_token_with_profile_email(email: &str, account_id: &str, user_id: &str) -> String {
        let payload = json!({
            "https://api.openai.com/profile": {
                "email": email,
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_user_id": user_id,
            },
        });
        let encoded =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
        format!("header.{encoded}.signature")
    }

    fn build_id_token_with_user_without_account(email: &str, user_id: &str) -> String {
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_user_id": user_id,
            },
        });
        let encoded =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
        format!("header.{encoded}.signature")
    }

    fn build_access_token_with_plan(plan_type: &str) -> String {
        let payload = json!({
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": plan_type,
            }
        });
        let encoded =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
        format!("header.{encoded}.signature")
    }

    fn build_access_token_with_identity(email: &str, account_id: &str, user_id: &str) -> String {
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_user_id": user_id,
            },
        });
        let encoded =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
        format!("header.{encoded}.signature")
    }

    async fn spawn_token_endpoint(
        access_token: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn handler(
            axum::extract::State(access_token): axum::extract::State<&'static str>,
            body: axum::body::Bytes,
        ) -> axum::response::Response {
            let body = String::from_utf8_lossy(&body);
            assert!(
                body.contains("grant_type=refresh_token"),
                "refresh grant missing: {body}"
            );
            (
                axum::http::StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                json!({
                    "access_token": access_token,
                    "refresh_token": "refreshed-token",
                    "id_token": build_id_token("refreshed@example.com", "acct-refreshed"),
                    "expires_in": 7200,
                })
                .to_string(),
            )
                .into_response()
        }

        let app = axum::Router::new()
            .route("/oauth/token", axum::routing::post(handler))
            .with_state(access_token);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind token endpoint");
        let addr = listener.local_addr().expect("token endpoint addr");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("token endpoint should run");
        });
        (format!("http://{addr}/oauth/token"), task)
    }

    async fn spawn_relogin_required_token_endpoint() -> (String, tokio::task::JoinHandle<()>) {
        async fn handler() -> axum::response::Response {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                json!({
                    "error": {
                        "message": "Refresh token is invalid.",
                        "type": "invalid_request_error",
                        "code": "invalid_grant",
                        "param": null
                    }
                })
                .to_string(),
            )
                .into_response()
        }

        let app = axum::Router::new().route("/oauth/token", axum::routing::post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind token endpoint");
        let addr = listener.local_addr().expect("token endpoint addr");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("token endpoint should run");
        });
        (format!("http://{addr}/oauth/token"), task)
    }

    async fn spawn_usage_relogin_then_ok_endpoint() -> (
        String,
        Arc<Mutex<Vec<(String, String)>>>,
        tokio::task::JoinHandle<()>,
    ) {
        async fn handler(
            axum::extract::State(request_headers): axum::extract::State<
                Arc<Mutex<Vec<(String, String)>>>,
            >,
            headers: axum::http::HeaderMap,
        ) -> axum::response::Response {
            let authorization = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let user_agent = headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            // 同时记录鉴权与身份头，验证重试不会退回旧 Codex 客户端版本。
            request_headers
                .lock()
                .expect("usage request headers lock")
                .push((authorization.clone(), user_agent));

            let (status, body) = match authorization.as_str() {
                "Bearer access-old" => (
                    axum::http::StatusCode::UNAUTHORIZED,
                    json!({
                        "error": {
                            "message": "Encountered invalidated oauth token for user.",
                            "type": "invalid_request_error",
                            "code": "token_revoked",
                            "param": null
                        }
                    }),
                ),
                "Bearer access-new" => (
                    axum::http::StatusCode::OK,
                    json!({
                        "plan_type": "pro",
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 25.0,
                                "reset_at": 1780477059
                            }
                        }
                    }),
                ),
                _ => (
                    axum::http::StatusCode::UNAUTHORIZED,
                    json!({
                        "error": {
                            "message": "unexpected usage token",
                            "type": "invalid_request_error",
                            "code": "token_invalidated",
                            "param": null
                        }
                    }),
                ),
            };

            (
                status,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body.to_string(),
            )
                .into_response()
        }

        let request_headers = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new()
            .route("/backend-api/wham/usage", axum::routing::get(handler))
            .with_state(request_headers.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind usage endpoint");
        let addr = listener.local_addr().expect("usage endpoint addr");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("usage endpoint should run");
        });
        (
            format!("http://{addr}/backend-api/wham/usage"),
            request_headers,
            task,
        )
    }

    async fn spawn_agent_identity_quota_endpoint() -> (
        String,
        String,
        Arc<Mutex<Vec<String>>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        #[derive(Clone)]
        struct State {
            task_ids: Arc<Mutex<Vec<String>>>,
            registrations: Arc<AtomicUsize>,
        }

        async fn usage_handler(
            axum::extract::State(state): axum::extract::State<State>,
            headers: axum::http::HeaderMap,
        ) -> axum::response::Response {
            let task_id = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("AgentAssertion "))
                .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
                .and_then(|value| serde_json::from_slice::<Value>(&value).ok())
                .and_then(|value| value["task_id"].as_str().map(str::to_string))
                .unwrap_or_default();
            state
                .task_ids
                .lock()
                .expect("quota Agent Identity task IDs lock")
                .push(task_id.clone());
            let (status, body) = if task_id == "task-quota-old" {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    json!({ "error": { "code": "invalid_task_id" } }),
                )
            } else {
                (
                    axum::http::StatusCode::OK,
                    json!({
                        "plan_type": "team",
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 20.0,
                                "reset_at": 1780477059
                            }
                        }
                    }),
                )
            };
            (
                status,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body.to_string(),
            )
                .into_response()
        }

        async fn register_handler(
            axum::extract::State(state): axum::extract::State<State>,
            axum::Json(body): axum::Json<Value>,
        ) -> impl IntoResponse {
            assert!(body["timestamp"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
            assert!(body["signature"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
            state.registrations.fetch_add(1, Ordering::SeqCst);
            axum::Json(json!({ "task_id": "task-quota-new" }))
        }

        let task_ids = Arc::new(Mutex::new(Vec::new()));
        let registrations = Arc::new(AtomicUsize::new(0));
        let state = State {
            task_ids: task_ids.clone(),
            registrations: registrations.clone(),
        };
        let app = axum::Router::new()
            .route("/backend-api/wham/usage", axum::routing::get(usage_handler))
            .route(
                "/v1/agent/runtime-quota/task/register",
                axum::routing::post(register_handler),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Agent Identity quota endpoint");
        let addr = listener
            .local_addr()
            .expect("Agent Identity quota endpoint address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("Agent Identity quota endpoint should run");
        });
        (
            format!("http://{addr}/backend-api/wham/usage"),
            format!("http://{addr}"),
            task_ids,
            registrations,
            task,
        )
    }

    fn build_record_with_quota_and_access_claim(
        quota_plan_type: &str,
        access_claim_plan_type: &str,
    ) -> CodexTokenRecord {
        CodexTokenRecord {
            credential: CodexCredential::Oauth {
                access_token: build_access_token_with_plan(access_claim_plan_type),
                refresh_token: "refresh-token".to_string(),
                client_id: Some(CodexRefreshTokenClient::Codex.client_id().to_string()),
                id_token: build_id_token("paid@example.com", "acct-paid"),
                auto_refresh_enabled: true,
                openai_device_id: None,
                expires_at: future_rfc3339(24),
                last_refresh: None,
            },
            status: CodexAccountStatus::Active,
            account_id: Some("acct-paid".to_string()),
            user_id: None,
            email: Some("paid@example.com".to_string()),
            quota: CodexQuotaCache {
                plan_type: Some(quota_plan_type.to_string()),
                quotas: Vec::new(),
                error: None,
                checked_at: Some(now_rfc3339()),
            },
        }
    }

    fn future_rfc3339(hours: i64) -> String {
        (OffsetDateTime::now_utc() + time::Duration::hours(hours))
            .format(&Rfc3339)
            .expect("format expires_at")
    }

    #[allow(clippy::too_many_arguments)]
    fn oauth_test_record(
        access_token: &str,
        refresh_token: &str,
        id_token: String,
        auto_refresh_enabled: bool,
        status: CodexAccountStatus,
        account_id: &str,
        email: &str,
        expires_at: String,
    ) -> CodexTokenRecord {
        CodexTokenRecord {
            credential: CodexCredential::Oauth {
                access_token: access_token.to_string(),
                refresh_token: refresh_token.to_string(),
                client_id: Some(CodexRefreshTokenClient::Codex.client_id().to_string()),
                id_token,
                auto_refresh_enabled,
                openai_device_id: None,
                expires_at,
                last_refresh: None,
            },
            status,
            account_id: Some(account_id.to_string()),
            user_id: None,
            email: Some(email.to_string()),
            quota: CodexQuotaCache::default(),
        }
    }

    fn test_agent_private_key() -> String {
        BASE64_STANDARD.encode(
            SigningKey::from_bytes(&[9_u8; 32])
                .to_pkcs8_der()
                .expect("encode Agent Identity test key")
                .as_bytes(),
        )
    }

    #[test]
    fn import_text_accepts_agent_identity_record_without_oauth_fields() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let imported = store
                .import_text(
                    &json!({
                        "auth_mode": "agentIdentity",
                        "agent_identity": {
                            "agent_runtime_id": "runtime-import",
                            "agent_private_key": test_agent_private_key(),
                            "task_id": "task-import",
                            "account_id": "account-import",
                            "chatgpt_user_id": "user-import",
                            "email": "agent@example.com",
                            "plan_type": "team",
                            "chatgpt_account_is_fedramp": false
                        }
                    })
                    .to_string(),
                )
                .await
                .expect("Agent Identity import should succeed");
            assert_eq!(imported.len(), 1);
            assert_eq!(
                imported[0].auth_method,
                super::super::types::CodexAuthMethod::AgentIdentity
            );
            assert!(imported[0].expires_at.is_none());
            assert!(imported[0].auto_refresh_enabled.is_none());

            let account_id = &imported[0].account_id;
            let authorization = store
                .authorization_header(account_id)
                .await
                .expect("build Agent Identity authorization");
            assert!(authorization.starts_with("AgentAssertion "));
            let persisted = store
                .load_account(account_id)
                .await
                .expect("load imported record");
            let serialized = serde_json::to_value(persisted).expect("serialize imported record");
            assert!(serialized.get("access_token").is_none());
            assert!(serialized.get("refresh_token").is_none());
            assert_eq!(serialized["credential"]["type"], "agent_identity");

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_text_treats_null_agent_identity_as_oauth_auth_json() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let imported = store
                .import_text(
                    &json!({
                        "auth_mode": "chatgpt",
                        "agent_identity": null,
                        "tokens": {
                            "access_token": "oauth-access",
                            "refresh_token": "oauth-refresh",
                            "id_token": build_id_token("oauth@example.com", "acct-oauth"),
                            "expires_at": future_rfc3339(24)
                        }
                    })
                    .to_string(),
                )
                .await
                .expect("OAuth auth.json with null Agent Identity should import");

            let _ = std::fs::remove_dir_all(data_dir);

            assert_eq!(imported.len(), 1);
            assert_eq!(
                imported[0].auth_method,
                super::super::types::CodexAuthMethod::Oauth
            );
        });
    }

    #[test]
    fn concurrent_agent_authorization_registers_and_persists_one_task() {
        run_async(async {
            async fn register_handler(
                axum::extract::State(count): axum::extract::State<Arc<AtomicUsize>>,
                axum::Json(body): axum::Json<Value>,
            ) -> impl IntoResponse {
                assert!(body["timestamp"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()));
                assert!(body["signature"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()));
                count.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({ "task_id": "task-registered" }))
            }

            let count = Arc::new(AtomicUsize::new(0));
            let app = axum::Router::new()
                .route(
                    "/v1/agent/runtime-concurrent/task/register",
                    axum::routing::post(register_handler),
                )
                .with_state(count.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind Agent Identity registration endpoint");
            let address = listener
                .local_addr()
                .expect("registration endpoint address");
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("registration endpoint");
            });

            let (store, data_dir) = create_test_store();
            let store = Arc::new(store);
            store
                .set_test_agent_identity_urls(&format!("http://{address}"), "http://unused")
                .await;
            let imported = store
                .import_text(
                    &json!({
                        "auth_mode": "agentIdentity",
                        "agent_identity": {
                            "agent_runtime_id": "runtime-concurrent",
                            "agent_private_key": test_agent_private_key(),
                            "account_id": "account-concurrent",
                            "chatgpt_user_id": "user-concurrent"
                        }
                    })
                    .to_string(),
                )
                .await
                .expect("import Agent Identity without task");
            let account_id = imported[0].account_id.clone();
            let mut tasks = Vec::new();
            for _ in 0..8 {
                let store = store.clone();
                let account_id = account_id.clone();
                tasks.push(tokio::spawn(async move {
                    store.authorization_header(&account_id).await
                }));
            }
            for task in tasks {
                let authorization = task
                    .await
                    .expect("authorization task")
                    .expect("build authorization");
                assert!(authorization.starts_with("AgentAssertion "));
            }
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert_eq!(
                store
                    .load_account(&account_id)
                    .await
                    .expect("load registered account")
                    .agent_identity()
                    .and_then(|identity| identity.task_id),
                Some("task-registered")
            );

            server.abort();
            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    fn past_rfc3339(hours: i64) -> String {
        (OffsetDateTime::now_utc() - time::Duration::hours(hours))
            .format(&Rfc3339)
            .expect("format expires_at")
    }

    #[test]
    fn refresh_is_needed_when_paid_quota_disagrees_with_free_access_token_claim() {
        let record = build_record_with_quota_and_access_claim("prolite", "free");

        assert!(record_needs_refresh(&record));
    }

    #[test]
    fn refresh_is_not_needed_when_free_quota_matches_free_access_token_claim() {
        let record = build_record_with_quota_and_access_claim("free", "free");

        assert!(!record_needs_refresh(&record));
    }

    #[test]
    fn refresh_is_not_needed_when_paid_quota_matches_paid_access_token_claim() {
        let record = build_record_with_quota_and_access_claim("prolite", "prolite");

        assert!(!record_needs_refresh(&record));
    }

    #[test]
    fn refresh_token_client_for_record_uses_persisted_client_id() {
        let mut record = build_record_with_quota_and_access_claim("free", "free");

        *record.oauth_mut().expect("OAuth fixture").client_id = None;
        assert_eq!(
            refresh_token_client_for_record(&record).expect("missing client_id defaults to codex"),
            CodexRefreshTokenClient::Codex
        );

        *record.oauth_mut().expect("OAuth fixture").client_id =
            Some(CodexRefreshTokenClient::Mobile.client_id().to_string());
        assert_eq!(
            refresh_token_client_for_record(&record).expect("mobile client_id should resolve"),
            CodexRefreshTokenClient::Mobile
        );

        *record.oauth_mut().expect("OAuth fixture").client_id = Some("unknown-client".to_string());
        let err = refresh_token_client_for_record(&record)
            .expect_err("unknown client_id should fail fast");
        assert!(err.contains("Unsupported Codex refresh token client_id"));
    }

    #[test]
    fn quota_refresh_waits_for_30_second_interval() {
        let within_window = (OffsetDateTime::now_utc() - time::Duration::seconds(29))
            .format(&Rfc3339)
            .expect("format checked_at");
        assert!(!quota_refresh_is_due(Some(within_window.as_str())));

        let outside_window = (OffsetDateTime::now_utc() - time::Duration::seconds(31))
            .format(&Rfc3339)
            .expect("format checked_at");
        assert!(quota_refresh_is_due(Some(outside_window.as_str())));
    }

    #[test]
    fn import_file_parses_token_proxy_codex_record() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let id_token = build_id_token("alice@example.com", "acct-token-proxy");
            let expires_at = future_rfc3339(6);
            let input_path = data_dir.join("token-proxy-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": id_token,
                    "expires_at": expires_at,
                    "last_refresh": "2026-03-27T01:02:03Z",
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("alice@example.com"));
            assert_eq!(imported[0].expires_at.as_deref(), Some(expires_at.as_str()));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-token-proxy"));
            assert_eq!(record.email.as_deref(), Some("alice@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_parses_cliproxy_codex_record_with_expired_alias() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expires_at = future_rfc3339(8);
            let input_path = data_dir.join("cliproxy-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": build_id_token("bob@example.com", "acct-cliproxy"),
                    "account_id": "acct-cliproxy",
                    "email": "bob@example.com",
                    "expired": expires_at,
                    "last_refresh": "2026-03-27T02:03:04Z",
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("bob@example.com"));
            assert_eq!(imported[0].expires_at.as_deref(), Some(expires_at.as_str()));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.expires_at_str(), Some(expires_at.as_str()));
            assert_eq!(record.account_id.as_deref(), Some("acct-cliproxy"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_parses_sub2api_oauth_token_response() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("sub2api-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": build_id_token("carol@example.com", "acct-sub2api"),
                    "token_type": "Bearer",
                    "expires_in": 7200,
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("carol@example.com"));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-sub2api"));
            assert_eq!(record.email.as_deref(), Some("carol@example.com"));
            assert!(record.expires_at().is_some());
            assert!(!record.is_expired());

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_text_parses_sub2api_oauth_token_response() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let imported = store
                .import_text(
                    serde_json::to_string_pretty(&json!({
                        "access_token": "access-token",
                        "refresh_token": "refresh-token",
                        "id_token": build_id_token("text@example.com", "acct-text"),
                        "token_type": "Bearer",
                        "expires_in": 7200,
                    }))
                    .expect("serialize test json")
                    .as_str(),
                )
                .await
                .expect("text import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("text@example.com"));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-text"));
            assert_eq!(record.email.as_deref(), Some("text@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_text_preserves_refresh_token_client_id() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let imported = store
                .import_text(
                    serde_json::to_string_pretty(&json!({
                        "tokens": {
                            "accessToken": "access-token",
                            "refreshToken": "refresh-token",
                            "idToken": build_id_token("mobile@example.com", "acct-mobile"),
                            "expires_in": 7200,
                            "client_id": CodexRefreshTokenClient::Mobile.client_id(),
                        },
                    }))
                    .expect("serialize test json")
                    .as_str(),
                )
                .await
                .expect("text import should succeed");

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(
                record.oauth().expect("OAuth record").client_id,
                Some(CodexRefreshTokenClient::Mobile.client_id())
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_text_preserves_openai_device_id() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let imported = store
                .import_text(
                    serde_json::to_string_pretty(&json!({
                        "tokens": {
                            "accessToken": "access-token",
                            "refreshToken": "refresh-token",
                            "idToken": build_id_token("device@example.com", "acct-device"),
                            "expires_in": 7200,
                            "openai_device_id": "device-import-123",
                        },
                    }))
                    .expect("serialize test json")
                    .as_str(),
                )
                .await
                .expect("text import should succeed");

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(
                record.oauth().expect("OAuth record").openai_device_id,
                Some("device-import-123")
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_text_parses_raw_access_token_lines() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expires_at = OffsetDateTime::now_utc() + time::Duration::hours(2);
            let access_token = {
                let payload = json!({
                    "exp": expires_at.unix_timestamp(),
                    "email": "raw@example.com",
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "acct-raw",
                        "chatgpt_user_id": "user-raw",
                    },
                });
                let encoded = URL_SAFE_NO_PAD
                    .encode(serde_json::to_vec(&payload).expect("serialize payload"));
                format!("header.{encoded}.signature")
            };

            let imported = store
                .import_text(access_token.as_str())
                .await
                .expect("raw access token import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("raw@example.com"));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-raw"));
            assert_eq!(record.oauth().expect("OAuth record").refresh_token, "");
            assert_eq!(record.auto_refresh_enabled(), Some(false));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_parses_new_api_generated_response_without_id_token() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expires_at = future_rfc3339(12);
            let input_path = data_dir.join("new-api-codex-response.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "success": true,
                    "message": "generated",
                    "data": {
                        "key": serde_json::to_string(&json!({
                            "type": "codex",
                            "access_token": "access-token",
                            "refresh_token": "refresh-token",
                            "account_id": "acct-new-api",
                            "email": "dave@example.com",
                            "expired": expires_at,
                            "last_refresh": "2026-03-30T01:02:03Z",
                        }))
                        .expect("serialize nested key"),
                        "account_id": "acct-new-api",
                        "email": "dave@example.com",
                        "expires_at": expires_at,
                        "last_refresh": "2026-03-30T01:02:03Z",
                    }
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("dave@example.com"));
            assert_eq!(imported[0].expires_at.as_deref(), Some(expires_at.as_str()));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-new-api"));
            assert_eq!(record.email.as_deref(), Some("dave@example.com"));
            assert_eq!(record.oauth().expect("OAuth record").id_token, "");

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_accepts_record_without_refresh_token() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expires_at = future_rfc3339(6);
            let input_path = data_dir.join("codex-without-refresh-token.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "",
                    "account_id": "acct-no-refresh",
                    "email": "norefresh@example.com",
                    "expired": expires_at,
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");
            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("norefresh@example.com"));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.oauth().expect("OAuth record").refresh_token, "");

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_recursively_imports_directory_json_records() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let import_dir = data_dir.join("codex-imports");
            let nested_dir = import_dir.join("nested");
            tokio::fs::create_dir_all(&nested_dir)
                .await
                .expect("create import dir");
            tokio::fs::write(
                import_dir.join("codex-root.json"),
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "root-access-token",
                    "refresh_token": "root-refresh-token",
                    "account_id": "acct-root-dir",
                    "email": "root@example.com",
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize root json"),
            )
            .await
            .expect("write root json");
            tokio::fs::write(
                nested_dir.join("codex-nested.json"),
                serde_json::to_string_pretty(&json!({
                    "access_token": "nested-access-token",
                    "refresh_token": "nested-refresh-token",
                    "id_token": build_id_token("nested@example.com", "acct-nested-dir"),
                    "expires_at": future_rfc3339(6),
                }))
                .expect("serialize nested json"),
            )
            .await
            .expect("write nested json");
            tokio::fs::write(import_dir.join("README.txt"), "ignore me")
                .await
                .expect("write non json file");

            let imported = store
                .import_file(import_dir)
                .await
                .expect("directory import should succeed");

            assert_eq!(imported.len(), 2);
            let emails = imported
                .iter()
                .filter_map(|item| item.email.clone())
                .collect::<Vec<_>>();
            assert!(emails.iter().any(|value| value == "root@example.com"));
            assert!(emails.iter().any(|value| value == "nested@example.com"));

            let accounts = store
                .list_accounts()
                .await
                .expect("imported accounts should be listed");
            assert_eq!(accounts.len(), 2);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_keeps_distinct_workspace_accounts_with_same_email() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-team-a.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "team-a-access-token",
                    "refresh_token": "team-a-refresh-token",
                    "expired": future_rfc3339(6),
                    "user": {
                        "email": "team@example.com"
                    },
                    "account": {
                        "id": "acct-team-a",
                        "structure": "team",
                        "planType": "team"
                    }
                }))
                .expect("serialize first team json"),
            )
            .await
            .expect("write first team json");

            let second_path = data_dir.join("codex-team-b.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "team-b-access-token",
                    "refresh_token": "team-b-refresh-token",
                    "expired": future_rfc3339(12),
                    "user": {
                        "email": "team@example.com"
                    },
                    "account": {
                        "id": "acct-team-b",
                        "structure": "team",
                        "planType": "team"
                    }
                }))
                .expect("serialize second team json"),
            )
            .await
            .expect("write second team json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first team import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second team import should succeed");

            assert_eq!(first_imported.len(), 1);
            assert_eq!(second_imported.len(), 1);
            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);

            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);

            let first_record = store
                .get_account_record(&first_imported[0].account_id)
                .await
                .expect("first team record should exist");
            let second_record = store
                .get_account_record(&second_imported[0].account_id)
                .await
                .expect("second team record should exist");
            assert_eq!(first_record.account_id.as_deref(), Some("acct-team-a"));
            assert_eq!(second_record.account_id.as_deref(), Some("acct-team-b"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_keeps_distinct_emails_when_user_id_missing_and_chatgpt_account_shared() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-missing-user-a.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "missing-user-a-access-token",
                    "refresh_token": "missing-user-a-refresh-token",
                    "account_id": "acct-shared-without-user",
                    "email": "missing-a@example.com",
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize first missing user json"),
            )
            .await
            .expect("write first missing user json");

            let second_path = data_dir.join("codex-missing-user-b.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "missing-user-b-access-token",
                    "refresh_token": "missing-user-b-refresh-token",
                    "account_id": "acct-shared-without-user",
                    "email": "missing-b@example.com",
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize second missing user json"),
            )
            .await
            .expect("write second missing user json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first missing user import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second missing user import should succeed");

            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);
            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_keeps_distinct_users_from_same_chatgpt_account() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-user-a.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "user-a-access-token",
                    "refresh_token": "user-a-refresh-token",
                    "id_token": build_id_token_with_user(
                        "user-a@example.com",
                        "acct-shared-team",
                        Some("user-a"),
                    ),
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize first user json"),
            )
            .await
            .expect("write first user json");

            let second_path = data_dir.join("codex-user-b.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "user-b-access-token",
                    "refresh_token": "user-b-refresh-token",
                    "id_token": build_id_token_with_user(
                        "user-b@example.com",
                        "acct-shared-team",
                        Some("user-b"),
                    ),
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize second user json"),
            )
            .await
            .expect("write second user json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first user import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second user import should succeed");

            assert_eq!(first_imported.len(), 1);
            assert_eq!(second_imported.len(), 1);
            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);

            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);
            assert!(accounts
                .iter()
                .any(|account| account.email.as_deref() == Some("user-a@example.com")));
            assert!(accounts
                .iter()
                .any(|account| account.email.as_deref() == Some("user-b@example.com")));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_keeps_distinct_chatgpt_accounts_for_same_user() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-account-a.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "account-a-access-token",
                    "refresh_token": "account-a-refresh-token",
                    "id_token": build_id_token_with_user(
                        "same-user@example.com",
                        "acct-a",
                        Some("same-user"),
                    ),
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize first account json"),
            )
            .await
            .expect("write first account json");

            let second_path = data_dir.join("codex-account-b.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "account-b-access-token",
                    "refresh_token": "account-b-refresh-token",
                    "id_token": build_id_token_with_user(
                        "same-user@example.com",
                        "acct-b",
                        Some("same-user"),
                    ),
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize second account json"),
            )
            .await
            .expect("write second account json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first account import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second account import should succeed");

            assert_eq!(first_imported.len(), 1);
            assert_eq!(second_imported.len(), 1);
            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);

            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_uses_auth_user_id_claim_alias() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-user-alias-a.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "user-alias-a-access-token",
                    "refresh_token": "user-alias-a-refresh-token",
                    "id_token": build_id_token_with_user_claim(
                        "alias-a@example.com",
                        "acct-shared-alias-team",
                        "user_id",
                        Some("alias-user-a"),
                    ),
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize first user json"),
            )
            .await
            .expect("write first user json");

            let second_path = data_dir.join("codex-user-alias-b.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "user-alias-b-access-token",
                    "refresh_token": "user-alias-b-refresh-token",
                    "id_token": build_id_token_with_user_claim(
                        "alias-b@example.com",
                        "acct-shared-alias-team",
                        "user_id",
                        Some("alias-user-b"),
                    ),
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize second user json"),
            )
            .await
            .expect("write second user json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first alias user import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second alias user import should succeed");

            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);
            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_keeps_case_distinct_chatgpt_user_ids_separate() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-case-user-a.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "case-user-a-access-token",
                    "refresh_token": "case-user-a-refresh-token",
                    "id_token": build_id_token_with_user(
                        "case-a@example.com",
                        "acct-case-shared-team",
                        Some("user-AbC"),
                    ),
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize first case user json"),
            )
            .await
            .expect("write first case user json");

            let second_path = data_dir.join("codex-case-user-b.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "case-user-b-access-token",
                    "refresh_token": "case-user-b-refresh-token",
                    "id_token": build_id_token_with_user(
                        "case-b@example.com",
                        "acct-case-shared-team",
                        Some("user-aBc"),
                    ),
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize second case user json"),
            )
            .await
            .expect("write second case user json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first case user import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second case user import should succeed");

            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);
            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_keeps_same_user_separate_when_only_one_account_id_is_known() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-user-no-account.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "same-user-no-account-access-token",
                    "refresh_token": "same-user-no-account-refresh-token",
                    "id_token": build_id_token_with_user_without_account(
                        "same-user-one-sided@example.com",
                        "same-user-one-sided",
                    ),
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize same user no account json"),
            )
            .await
            .expect("write same user no account json");

            let second_path = data_dir.join("codex-user-known-account.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "same-user-known-account-access-token",
                    "refresh_token": "same-user-known-account-refresh-token",
                    "id_token": build_id_token_with_user(
                        "same-user-one-sided@example.com",
                        "acct-known-for-same-user",
                        Some("same-user-one-sided"),
                    ),
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize same user known account json"),
            )
            .await
            .expect("write same user known account json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first same user import should succeed");
            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second same user import should succeed");

            assert_ne!(first_imported[0].account_id, second_imported[0].account_id);
            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 2);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_prefers_jwt_identity_over_outer_fields() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("codex-jwt-authoritative.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "jwt-authoritative-access-token",
                    "refresh_token": "jwt-authoritative-refresh-token",
                    "id_token": build_id_token_with_user(
                        "jwt-user@example.com",
                        "acct-from-jwt",
                        Some("user-from-jwt"),
                    ),
                    "account_id": "acct-from-wrapper",
                    "user_id": "user-from-wrapper",
                    "email": "wrapper@example.com",
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize jwt authoritative json"),
            )
            .await
            .expect("write jwt authoritative json");

            let imported = store
                .import_file(input_path)
                .await
                .expect("jwt authoritative import should succeed");
            assert_eq!(imported.len(), 1);

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-from-jwt"));
            assert_eq!(record.user_id.as_deref(), Some("user-from-jwt"));
            assert_eq!(record.email.as_deref(), Some("jwt-user@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_reads_profile_email_claim_from_jwt() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("codex-profile-email.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "profile-email-access-token",
                    "refresh_token": "profile-email-refresh-token",
                    "id_token": build_id_token_with_profile_email(
                        "profile@example.com",
                        "acct-profile-email",
                        "user-profile-email",
                    ),
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize profile email json"),
            )
            .await
            .expect("write profile email json");

            let imported = store
                .import_file(input_path)
                .await
                .expect("profile email import should succeed");
            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("profile@example.com"));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.email.as_deref(), Some("profile@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_reads_identity_from_access_token_when_id_token_missing() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("codex-access-identity.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": build_access_token_with_identity(
                        "access-identity@example.com",
                        "acct-access-identity",
                        "user-access-identity",
                    ),
                    "refresh_token": "access-identity-refresh-token",
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize access identity json"),
            )
            .await
            .expect("write access identity json");

            let imported = store
                .import_file(input_path)
                .await
                .expect("access identity import should succeed");
            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");

            assert_eq!(record.account_id.as_deref(), Some("acct-access-identity"));
            assert_eq!(record.user_id.as_deref(), Some("user-access-identity"));
            assert_eq!(record.email.as_deref(), Some("access-identity@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_overwrites_existing_record_for_same_real_account() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first_path = data_dir.join("codex-first.json");
            tokio::fs::write(
                &first_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "first-access-token",
                    "refresh_token": "first-refresh-token",
                    "client_id": CodexRefreshTokenClient::Mobile.client_id(),
                    "openai_device_id": "device-existing",
                    "account_id": "acct-overwrite",
                    "email": "overwrite@example.com",
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize first json"),
            )
            .await
            .expect("write first json");

            let first_imported = store
                .import_file(first_path)
                .await
                .expect("first import should succeed");
            let first_local_account_id = first_imported[0].account_id.clone();
            // Phase B: 账户无 proxy_url；overwrite 只刷新凭据并保留 client_id / device_id。

            let second_path = data_dir.join("codex-second.json");
            tokio::fs::write(
                &second_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "second-access-token",
                    "refresh_token": "second-refresh-token",
                    "account_id": "acct-overwrite",
                    "email": "overwrite@example.com",
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize second json"),
            )
            .await
            .expect("write second json");

            let second_imported = store
                .import_file(second_path)
                .await
                .expect("second import should succeed");

            assert_eq!(second_imported.len(), 1);
            assert_eq!(second_imported[0].account_id, first_local_account_id);

            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should succeed");
            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0].account_id, first_local_account_id);

            let record = store
                .get_account_record(&first_local_account_id)
                .await
                .expect("record should exist");
            assert_eq!(
                record.oauth().expect("OAuth record").access_token,
                "second-access-token"
            );
            assert_eq!(
                record.oauth().expect("OAuth record").refresh_token,
                "second-refresh-token"
            );
            assert_eq!(
                record.oauth().expect("OAuth record").client_id,
                Some(CodexRefreshTokenClient::Mobile.client_id())
            );
            assert_eq!(
                record.oauth().expect("OAuth record").openai_device_id,
                Some("device-existing")
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn refresh_account_without_refresh_token_requires_relogin() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("codex-refresh-missing.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "",
                    "account_id": "acct-refresh-missing",
                    "email": "expired@example.com",
                    "expired": "2020-01-01T00:00:00Z",
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");
            let err = store
                .refresh_account(&imported[0].account_id)
                .await
                .expect_err("refresh should fail without refresh token");
            assert_eq!(
                err,
                "Codex account has no refresh token. Please sign in again."
            );

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should still be readable");
            assert!(record.is_expired());
            assert_eq!(record.oauth().expect("OAuth record").refresh_token, "");

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn set_auto_refresh_updates_record_flag() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("codex-set-auto-refresh.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "account_id": "acct-toggle-auto-refresh",
                    "email": "toggle@example.com",
                    "expired": future_rfc3339(6),
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");
            assert_eq!(imported[0].auto_refresh_enabled, Some(false));

            let updated = store
                .set_auto_refresh(&imported[0].account_id, true)
                .await
                .expect("set auto refresh should succeed");
            assert_eq!(updated.auto_refresh_enabled, Some(true));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.auto_refresh_enabled(), Some(true));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn keepalive_refreshes_expired_auto_refresh_accounts_without_resolve() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expired_at = (OffsetDateTime::now_utc() - time::Duration::hours(1))
                .format(&Rfc3339)
                .expect("format expires_at");
            let account_id = "codex-keepalive.json".to_string();
            store
                .save_record(
                    account_id.clone(),
                    oauth_test_record(
                        "expired-access",
                        "refresh-token",
                        build_id_token("old@example.com", "acct-old"),
                        true,
                        CodexAccountStatus::Active,
                        "acct-old",
                        "old@example.com",
                        expired_at,
                    ),
                )
                .await
                .expect("seed expired codex account");
            let (token_url, task) = spawn_token_endpoint("refreshed-access").await;
            store.set_test_token_url(&token_url).await;

            let refreshed = store
                .refresh_due_accounts()
                .await
                .expect("refresh due accounts");
            let record = store
                .load_account(&account_id)
                .await
                .expect("load refreshed record");

            task.abort();
            let _ = std::fs::remove_dir_all(data_dir);

            assert_eq!(refreshed, vec![account_id]);
            assert_eq!(
                record.oauth().expect("OAuth record").access_token,
                "refreshed-access"
            );
            assert!(!record.is_expired());
        });
    }

    #[test]
    fn quota_refresh_recovers_stale_agent_identity_task_once() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let account_id = "codex-agent-quota.json".to_string();
            store
                .save_record(
                    account_id.clone(),
                    CodexTokenRecord {
                        credential: CodexCredential::AgentIdentity {
                            agent_runtime_id: "runtime-quota".to_string(),
                            agent_private_key: test_agent_private_key(),
                            task_id: Some("task-quota-old".to_string()),
                            plan_type: Some("team".to_string()),
                            chatgpt_account_is_fedramp: false,
                        },
                        status: CodexAccountStatus::Active,
                        account_id: Some("acct-agent-quota".to_string()),
                        user_id: Some("user-agent-quota".to_string()),
                        email: Some("agent-quota@example.com".to_string()),
                        quota: CodexQuotaCache::default(),
                    },
                )
                .await
                .expect("seed Agent Identity quota account");
            let (usage_url, auth_url, task_ids, registrations, endpoint_task) =
                spawn_agent_identity_quota_endpoint().await;
            store
                .set_test_agent_identity_urls(&auth_url, "http://unused")
                .await;

            let quota = crate::quota::refresh_quota_cache_with_usage_endpoint(
                &store,
                &account_id,
                &usage_url,
            )
            .await
            .expect("quota refresh should recover stale Agent Identity task");
            let record = store
                .load_account(&account_id)
                .await
                .expect("load recovered Agent Identity account");

            endpoint_task.abort();
            let _ = std::fs::remove_dir_all(data_dir);

            assert_eq!(quota.plan_type.as_deref(), Some("team"));
            assert!(quota.error.is_none());
            assert_eq!(
                *task_ids.lock().expect("quota Agent Identity task IDs lock"),
                vec!["task-quota-old".to_string(), "task-quota-new".to_string()]
            );
            assert_eq!(registrations.load(Ordering::SeqCst), 1);
            assert_eq!(
                record
                    .agent_identity()
                    .and_then(|identity| identity.task_id),
                Some("task-quota-new")
            );
        });
    }

    #[test]
    fn quota_refresh_retries_usage_after_relogin_error() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let account_id = "codex-quota-retry.json".to_string();
            store
                .save_record(
                    account_id.clone(),
                    oauth_test_record(
                        "access-old",
                        "refresh-token",
                        build_id_token("quota@example.com", "acct-quota"),
                        true,
                        CodexAccountStatus::Active,
                        "acct-quota",
                        "quota@example.com",
                        future_rfc3339(24),
                    ),
                )
                .await
                .expect("seed codex account");
            let (usage_url, request_headers, usage_task) =
                spawn_usage_relogin_then_ok_endpoint().await;
            let (token_url, token_task) = spawn_token_endpoint("access-new").await;
            store.set_test_token_url(&token_url).await;

            let quota = crate::quota::refresh_quota_cache_with_usage_endpoint(
                &store,
                &account_id,
                &usage_url,
            )
            .await
            .expect("quota refresh should retry after token refresh");
            let record = store
                .load_account(&account_id)
                .await
                .expect("load refreshed record");

            usage_task.abort();
            token_task.abort();
            let _ = std::fs::remove_dir_all(data_dir);

            assert_eq!(quota.plan_type.as_deref(), Some("pro"));
            assert!(quota.error.is_none());
            assert_eq!(
                record.oauth().expect("OAuth record").access_token,
                "access-new"
            );
            assert_eq!(
                *request_headers.lock().expect("usage request headers lock"),
                vec![
                    (
                        "Bearer access-old".to_string(),
                        crate::USER_AGENT.to_string()
                    ),
                    (
                        "Bearer access-new".to_string(),
                        crate::USER_AGENT.to_string()
                    )
                ]
            );
        });
    }

    #[test]
    fn refresh_account_persists_invalid_after_relogin_error() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let account_id = "codex-refresh-invalid.json".to_string();
            store
                .save_record(
                    account_id.clone(),
                    oauth_test_record(
                        "access-old",
                        "refresh-token",
                        build_id_token("refresh-invalid@example.com", "acct-invalid"),
                        true,
                        CodexAccountStatus::Active,
                        "acct-invalid",
                        "refresh-invalid@example.com",
                        future_rfc3339(24),
                    ),
                )
                .await
                .expect("seed codex account");
            let (token_url, token_task) = spawn_relogin_required_token_endpoint().await;
            store.set_test_token_url(&token_url).await;

            let err = store
                .refresh_account(&account_id)
                .await
                .expect_err("refresh should require re-login");
            let record = store
                .load_account(&account_id)
                .await
                .expect("load invalid record");

            token_task.abort();
            let _ = std::fs::remove_dir_all(data_dir);

            assert!(err.contains("Codex 登录已失效"));
            assert!(matches!(record.status, CodexAccountStatus::Invalid));
            assert!(matches!(
                record.effective_status(),
                CodexAccountStatus::Invalid
            ));
        });
    }

    #[test]
    fn quota_refresh_persists_token_refresh_failure_after_relogin_error() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let account_id = "codex-quota-refresh-fails.json".to_string();
            store
                .save_record(
                    account_id.clone(),
                    oauth_test_record(
                        "access-old",
                        "refresh-token",
                        build_id_token("quota-fails@example.com", "acct-quota-fails"),
                        true,
                        CodexAccountStatus::Active,
                        "acct-quota-fails",
                        "quota-fails@example.com",
                        future_rfc3339(24),
                    ),
                )
                .await
                .expect("seed codex account");
            let (usage_url, request_headers, usage_task) =
                spawn_usage_relogin_then_ok_endpoint().await;
            let (token_url, token_task) = spawn_relogin_required_token_endpoint().await;
            store.set_test_token_url(&token_url).await;

            let quota = crate::quota::refresh_quota_cache_with_usage_endpoint(
                &store,
                &account_id,
                &usage_url,
            )
            .await
            .expect("quota failure should be persisted");
            let record = store
                .load_account(&account_id)
                .await
                .expect("load persisted record");

            usage_task.abort();
            token_task.abort();
            let _ = std::fs::remove_dir_all(data_dir);

            let error = quota.error.as_deref().expect("quota error");
            assert!(error.contains("Codex usage request failed after token refresh failed"));
            assert!(error.contains("Codex 登录已失效"));
            assert_eq!(record.quota.error.as_deref(), Some(error));
            assert!(matches!(record.status, CodexAccountStatus::Invalid));
            assert!(matches!(
                record.effective_status(),
                CodexAccountStatus::Invalid
            ));
            assert_eq!(
                *request_headers.lock().expect("usage request headers lock"),
                vec![(
                    "Bearer access-old".to_string(),
                    crate::USER_AGENT.to_string()
                )]
            );
        });
    }

    #[test]
    fn keepalive_skips_accounts_that_cannot_auto_refresh() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expired_at = (OffsetDateTime::now_utc() - time::Duration::hours(1))
                .format(&Rfc3339)
                .expect("format expires_at");
            let records = [
                (
                    "codex-disabled.json",
                    oauth_test_record(
                        "disabled-access",
                        "refresh-token",
                        build_id_token("disabled@example.com", "acct-disabled"),
                        true,
                        CodexAccountStatus::Invalid,
                        "acct-disabled",
                        "disabled@example.com",
                        expired_at.clone(),
                    ),
                ),
                (
                    "codex-manual.json",
                    oauth_test_record(
                        "manual-access",
                        "refresh-token",
                        build_id_token("manual@example.com", "acct-manual"),
                        false,
                        CodexAccountStatus::Active,
                        "acct-manual",
                        "manual@example.com",
                        expired_at.clone(),
                    ),
                ),
                (
                    "codex-access-only.json",
                    oauth_test_record(
                        "access-only",
                        "",
                        build_id_token("access-only@example.com", "acct-access-only"),
                        true,
                        CodexAccountStatus::Active,
                        "acct-access-only",
                        "access-only@example.com",
                        expired_at.clone(),
                    ),
                ),
            ];
            for (account_id, record) in records {
                store
                    .save_record(account_id.to_string(), record)
                    .await
                    .expect("seed account");
            }
            let (token_url, task) = spawn_token_endpoint("should-not-be-used").await;
            store.set_test_token_url(&token_url).await;

            let refreshed = store
                .refresh_due_accounts()
                .await
                .expect("refresh due accounts");

            task.abort();
            let _ = std::fs::remove_dir_all(data_dir);

            assert!(refreshed.is_empty());
        });
    }

    #[test]
    fn list_accounts_orders_by_account_id_ascending() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            // Phase B: 列表稳定按 account_id 升序，不再按 priority。
            store
                .save_record(
                    "codex-z.json".to_string(),
                    oauth_test_record(
                        "access-z",
                        "refresh-z",
                        String::new(),
                        true,
                        CodexAccountStatus::Active,
                        "acct-z",
                        "z@example.com",
                        future_rfc3339(6),
                    ),
                )
                .await
                .expect("save z");
            store
                .save_record(
                    "codex-a.json".to_string(),
                    oauth_test_record(
                        "access-a",
                        "refresh-a",
                        String::new(),
                        true,
                        CodexAccountStatus::Active,
                        "acct-a",
                        "a@example.com",
                        future_rfc3339(6),
                    ),
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
                vec!["codex-a.json".to_string(), "codex-z.json".to_string()]
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn get_account_record_does_not_refresh_invalid_expired_account() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            // Invalid 健康态：读取路径不得触发 token refresh。
            store
                .save_record(
                    "codex-invalid.json".to_string(),
                    oauth_test_record(
                        "expired-access",
                        "refresh-token",
                        String::new(),
                        true,
                        CodexAccountStatus::Invalid,
                        "acct-invalid",
                        "invalid@example.com",
                        past_rfc3339(2),
                    ),
                )
                .await
                .expect("save invalid expired account");

            let record = store
                .get_account_record("codex-invalid.json")
                .await
                .expect("invalid account should still load");

            assert!(matches!(record.status, CodexAccountStatus::Invalid));
            assert!(!record.is_usable());
            assert_eq!(
                record.oauth().expect("OAuth record").access_token,
                "expired-access"
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn list_accounts_reads_from_sqlite_after_legacy_files_are_removed() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("sqlite-backed-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": build_id_token("db@example.com", "acct-sqlite"),
                    "expires_at": future_rfc3339(6),
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");
            let legacy_dir = data_dir.join("codex-auth");
            if legacy_dir.exists() {
                std::fs::remove_dir_all(&legacy_dir).expect("remove legacy auth dir");
            }

            let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
            let reloaded_store =
                CodexAccountStore::new(&paths, app_proxy::new_state()).expect("codex store");
            let accounts = reloaded_store
                .list_accounts()
                .await
                .expect("list accounts should read sqlite data");

            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0].account_id, imported[0].account_id);
            assert_eq!(accounts[0].email.as_deref(), Some("db@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn list_accounts_does_not_load_legacy_directory_records() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let legacy_dir = data_dir.join("codex-auth");
            tokio::fs::create_dir_all(&legacy_dir)
                .await
                .expect("create legacy codex dir");
            tokio::fs::write(
                legacy_dir.join("codex-legacy.json"),
                serde_json::to_string_pretty(&json!({
                    "access_token": "legacy-access-token",
                    "refresh_token": "legacy-refresh-token",
                    "id_token": build_id_token("legacy@example.com", "acct-legacy"),
                    "expires_at": future_rfc3339(6),
                }))
                .expect("serialize legacy codex json"),
            )
            .await
            .expect("write legacy codex json");

            let accounts = store
                .list_accounts()
                .await
                .expect("list accounts should only use sqlite");
            assert!(accounts.is_empty());

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }
}
