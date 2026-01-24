use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::paths::TokenProxyPaths;

use super::oauth;
use super::sso_oidc;
use super::types::{KiroAccountStatus, KiroAccountSummary, KiroTokenRecord};
use super::util::{expires_at_from_seconds, extract_email_from_jwt, now_rfc3339, sanitize_id_part};

const KIRO_AUTH_DIR_NAME: &str = "kiro-auth";

pub struct KiroAccountStore {
    dir: PathBuf,
    cache: RwLock<HashMap<String, KiroTokenRecord>>,
    app_proxy: AppProxyState,
}

impl KiroAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        let dir = paths.data_dir().join(KIRO_AUTH_DIR_NAME);
        Ok(Self {
            dir,
            cache: RwLock::new(HashMap::new()),
            app_proxy,
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
                expires_at: record.expires_at().map(|value| value.format(&time::format_description::well_known::Rfc3339).unwrap_or_else(|_| record.expires_at.clone())),
                status: record.status(),
            })
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub(crate) async fn get_access_token(&self, account_id: &str) -> Result<String, String> {
        let record = self.load_account(account_id).await?;
        let refreshed = self.refresh_if_needed(account_id, record).await?;
        Ok(refreshed.access_token)
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
        let summary = self
            .save_record(account_id.to_string(), refreshed)
            .await?;
        if matches!(summary.status, KiroAccountStatus::Expired) {
            return Err("Kiro token refresh failed.".to_string());
        }
        Ok(())
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: KiroTokenRecord,
    ) -> Result<KiroAccountSummary, String> {
        self.ensure_dir().await?;
        let path = self.account_path(&account_id);
        let payload = serde_json::to_string_pretty(&record)
            .map_err(|err| format!("Failed to serialize token record: {err}"))?;
        tokio::fs::write(&path, payload)
            .await
            .map_err(|err| format!("Failed to write token record: {err}"))?;
        let mut cache = self.cache.write().await;
        cache.insert(account_id.clone(), record.clone());
        Ok(KiroAccountSummary {
            account_id,
            provider: record.provider.clone(),
            auth_method: record.auth_method.clone(),
            email: record.email.clone(),
            expires_at: record.expires_at().map(|value| value.format(&time::format_description::well_known::Rfc3339).unwrap_or_else(|_| record.expires_at.clone())),
            status: record.status(),
        })
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
        let path = self.account_path(account_id);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|err| format!("Failed to delete token record: {err}"))?;
        }
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
        let proxy_url = self.app_proxy_url().await;
        let refreshed = match record.auth_method.as_str() {
            "builder-id" => sso_oidc::refresh_builder_token(&record, proxy_url.as_deref()).await?,
            "idc" => sso_oidc::refresh_idc_token(&record, proxy_url.as_deref()).await?,
            "social" => oauth::refresh_social_token(&record, proxy_url.as_deref()).await?,
            _ => return Err("Unsupported Kiro auth method.".to_string()),
        };
        let summary = self.save_record(account_id.to_string(), refreshed.clone()).await?;
        if matches!(summary.status, KiroAccountStatus::Expired) {
            return Err("Kiro token refresh failed.".to_string());
        }
        Ok(refreshed)
    }

    async fn load_account(&self, account_id: &str) -> Result<KiroTokenRecord, String> {
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

    async fn refresh_cache(&self) -> Result<(), String> {
        let mut cache = HashMap::new();
        let dir = self.dir.clone();
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let mut guard = self.cache.write().await;
                guard.clear();
                return Ok(());
            }
            Err(err) => return Err(format!("Failed to read Kiro auth directory: {err}")),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read Kiro auth entry: {err}"))?
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let file_name = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            let contents = match tokio::fs::read_to_string(&path).await {
                Ok(contents) => contents,
                Err(_) => continue,
            };
            let record: KiroTokenRecord = match serde_json::from_str(&contents) {
                Ok(record) => record,
                Err(_) => continue,
            };
            cache.insert(file_name, record);
        }

        let mut guard = self.cache.write().await;
        *guard = cache;
        Ok(())
    }

    async fn ensure_dir(&self) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|err| format!("Failed to create Kiro auth dir: {err}"))
    }

    async fn unique_account_id(&self, provider: &str, id_part: &str) -> Result<String, String> {
        self.ensure_dir().await?;
        let mut suffix = 0u32;
        loop {
            let candidate = if suffix == 0 {
                format!("kiro-{provider}-{id_part}.json")
            } else {
                format!("kiro-{provider}-{id_part}-{suffix}.json")
            };
            if !tokio::fs::try_exists(self.account_path(&candidate))
                .await
                .unwrap_or(false)
            {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }

    fn account_path(&self, account_id: &str) -> PathBuf {
        self.dir.join(account_id)
    }
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
    let auth_method = normalize_auth_method(
        credentials.auth_method.as_deref(),
        Some(provider.as_str()),
    );
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
        })
    }
}
