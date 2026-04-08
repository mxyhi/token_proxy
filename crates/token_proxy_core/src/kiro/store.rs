use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, RwLock};

use crate::app_proxy::AppProxyState;
use crate::oauth_util::normalize_proxy_url;
use crate::paths::TokenProxyPaths;
use crate::provider_accounts;

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
        })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub async fn import_ide_tokens(
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
            if let Ok(summary) = self.save_new_account(record).await {
                imported.push(summary);
            }
        }
        if imported.is_empty() {
            return Err("No valid Kiro token JSON files found.".to_string());
        }
        Ok(imported)
    }

    pub async fn import_kam_export(
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
            if let Ok(summary) = self.save_new_account(record).await {
                imported.push(summary);
            }
        }
        if imported.is_empty() {
            return Err("No valid Kiro accounts found in JSON file.".to_string());
        }
        Ok(imported)
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
                proxy_url: record.proxy_url.clone(),
            })
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub(crate) async fn get_account_record(
        &self,
        account_id: &str,
    ) -> Result<KiroTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub(crate) async fn refresh_account(&self, account_id: &str) -> Result<(), String> {
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

    pub async fn set_proxy_url(
        &self,
        account_id: &str,
        proxy_url: Option<&str>,
    ) -> Result<KiroAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        record.proxy_url = normalize_proxy_url(proxy_url)?;
        self.save_record(account_id.to_string(), record).await
    }

    pub async fn set_status(
        &self,
        account_id: &str,
        status: KiroAccountStatus,
    ) -> Result<KiroAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        record.status = status;
        self.save_record(account_id.to_string(), record).await
    }

    pub(crate) async fn save_record(
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
            proxy_url: record.proxy_url.clone(),
        })
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

    pub(crate) async fn save_new_account(
        &self,
        mut record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        if record.email.is_none() {
            record.email = extract_email_from_jwt(&record.access_token);
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
        let account_id = self.unique_account_id(&provider, &id_part).await?;
        self.save_record(account_id, record).await
    }

    pub(crate) async fn delete_account(&self, account_id: &str) -> Result<(), String> {
        provider_accounts::delete_account(&self.paths, account_id).await?;
        let mut cache = self.cache.write().await;
        cache.remove(account_id);
        Ok(())
    }

    async fn refresh_if_needed(
        &self,
        account_id: &str,
        record: KiroTokenRecord,
    ) -> Result<KiroTokenRecord, String> {
        if !record.is_expired() {
            return Ok(record);
        }
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: KiroTokenRecord,
    ) -> Result<KiroTokenRecord, String> {
        let proxy_url = self.effective_proxy_url(record.proxy_url.as_deref()).await;
        let refreshed = match record.auth_method.as_str() {
            "builder-id" => sso_oidc::refresh_builder_token(&record, proxy_url.as_deref()).await?,
            "idc" => sso_oidc::refresh_idc_token(&record, proxy_url.as_deref()).await?,
            "social" => oauth::refresh_social_token(&record, proxy_url.as_deref()).await?,
            _ => return Err("Unsupported Kiro auth method.".to_string()),
        };
        let refreshed = KiroTokenRecord {
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

    pub(crate) async fn app_proxy_url(&self) -> Option<String> {
        self.app_proxy.read().await.clone()
    }

    pub(crate) async fn refresh_quota_if_stale(&self, account_id: &str) -> Result<bool, String> {
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
    pub(crate) async fn resolve_account_record(
        &self,
        account_id: Option<&str>,
    ) -> Result<(String, KiroTokenRecord), String> {
        self.resolve_account_record_with_order(account_id, None)
            .await
    }

    pub(crate) async fn resolve_account_record_with_order(
        &self,
        account_id: Option<&str>,
        ordered_account_ids: Option<&[String]>,
    ) -> Result<(String, KiroTokenRecord), String> {
        if let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) {
            let record = self.get_account_record(account_id).await?;
            if matches!(record.effective_status(), KiroAccountStatus::Disabled) {
                return Err(format!("Kiro account is disabled: {account_id}"));
            }
            if matches!(record.effective_status(), KiroAccountStatus::Expired) {
                return Err(format!("Kiro account is expired: {account_id}"));
            }
            return Ok((account_id.to_string(), record));
        }

        self.refresh_cache().await?;
        let account_ids = if let Some(ordered_account_ids) = ordered_account_ids {
            ordered_account_ids.to_vec()
        } else {
            let cache = self.cache.read().await;
            let mut account_ids = cache.keys().cloned().collect::<Vec<_>>();
            account_ids.sort();
            account_ids
        };

        let mut last_error = None;
        for account_id in account_ids {
            match self.get_account_record(&account_id).await {
                Ok(record) if record.is_schedulable() => {
                    return Ok((account_id, record));
                }
                Ok(record) if matches!(record.effective_status(), KiroAccountStatus::Disabled) => {
                    last_error = Some(format!("Kiro account is disabled: {account_id}"));
                }
                Ok(_) => {
                    last_error = Some(format!("Kiro account is expired: {account_id}"));
                }
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "Kiro account is not configured.".to_string()))
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

    async fn refresh_quota_if_stale_inner(&self, account_id: &str) -> Result<bool, String> {
        let record = self.load_account(account_id).await?;
        if !quota_refresh_is_due(record.quota.checked_at.as_deref()) {
            return Ok(false);
        }
        super::quota::refresh_quota_cache_if_stale(self, account_id).await?;
        Ok(true)
    }

    async fn refresh_cache(&self) -> Result<(), String> {
        let cache = provider_accounts::list_kiro_records(&self.paths).await?;
        let mut guard = self.cache.write().await;
        *guard = cache;
        Ok(())
    }

    async fn unique_account_id(&self, provider: &str, id_part: &str) -> Result<String, String> {
        self.refresh_cache().await?;
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
        proxy_url: None,
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
            proxy_url: None,
            quota: super::types::KiroQuotaCache::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_proxy;
    use crate::paths::TokenProxyPaths;
    use rand::random;
    use serde_json::json;
    use std::future::Future;
    use time::Duration;

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
                    proxy_url: None,
                    quota: crate::kiro::KiroQuotaCache::default(),
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
    fn set_proxy_url_updates_record_value() {
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
                    email: Some("proxy-kiro@example.com".to_string()),
                    last_refresh: None,
                    start_url: None,
                    region: None,
                    status: KiroAccountStatus::Active,
                    proxy_url: None,
                    quota: crate::kiro::KiroQuotaCache::default(),
                })
                .await
                .expect("save kiro account");

            let updated = store
                .set_proxy_url(&saved.account_id, Some("http://127.0.0.1:7890"))
                .await
                .expect("set proxy url should succeed");
            assert_eq!(updated.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));

            let record = store
                .get_account_record(&saved.account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));

            let cleared = store
                .set_proxy_url(&saved.account_id, None::<&str>)
                .await
                .expect("clear proxy url should succeed");
            assert_eq!(cleared.proxy_url, None);

            let cleared_record = store
                .get_account_record(&saved.account_id)
                .await
                .expect("record should exist");
            assert_eq!(cleared_record.proxy_url, None);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn set_enabled_updates_record_flag() {
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
                    email: Some("enabled-kiro@example.com".to_string()),
                    last_refresh: None,
                    start_url: None,
                    region: None,
                    status: KiroAccountStatus::Active,
                    proxy_url: None,
                    quota: crate::kiro::KiroQuotaCache::default(),
                })
                .await
                .expect("save kiro account");

            let updated = store
                .set_status(&saved.account_id, KiroAccountStatus::Disabled)
                .await
                .expect("set status should succeed");
            assert!(matches!(updated.status, KiroAccountStatus::Disabled));

            let record = store
                .get_account_record(&saved.account_id)
                .await
                .expect("record should exist");
            assert!(matches!(record.status, KiroAccountStatus::Disabled));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn resolve_account_record_skips_disabled_accounts() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            store
                .save_record(
                    "kiro-google-a.json".to_string(),
                    KiroTokenRecord {
                        access_token: "access-1".to_string(),
                        refresh_token: "refresh-1".to_string(),
                        profile_arn: Some("arn:aws:iam::123456789012:user/a".to_string()),
                        expires_at: future_rfc3339(6),
                        auth_method: "google".to_string(),
                        provider: "kiro".to_string(),
                        client_id: None,
                        client_secret: None,
                        email: Some("aaa@example.com".to_string()),
                        last_refresh: None,
                        start_url: None,
                        region: None,
                        status: KiroAccountStatus::Disabled,
                        proxy_url: None,
                        quota: crate::kiro::KiroQuotaCache::default(),
                    },
                )
                .await
                .expect("save first account");
            store
                .save_record(
                    "kiro-google-b.json".to_string(),
                    KiroTokenRecord {
                        access_token: "access-2".to_string(),
                        refresh_token: "refresh-2".to_string(),
                        profile_arn: Some("arn:aws:iam::123456789012:user/b".to_string()),
                        expires_at: future_rfc3339(6),
                        auth_method: "google".to_string(),
                        provider: "kiro".to_string(),
                        client_id: None,
                        client_secret: None,
                        email: Some("zzz@example.com".to_string()),
                        last_refresh: None,
                        start_url: None,
                        region: None,
                        status: KiroAccountStatus::Active,
                        proxy_url: None,
                        quota: crate::kiro::KiroQuotaCache::default(),
                    },
                )
                .await
                .expect("save second account");

            let (account_id, record) = store
                .resolve_account_record(None)
                .await
                .expect("should resolve enabled account");

            assert_eq!(account_id, "kiro-google-b.json");
            assert!(record.is_schedulable());

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
}
