use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::{AppHandle, Manager};
use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::proxy::config::config_dir_path;

use super::oauth;
use super::sso_oidc;
use super::types::{KiroAccountStatus, KiroAccountSummary, KiroTokenRecord};
use super::util::{expires_at_from_seconds, extract_email_from_jwt, now_rfc3339, sanitize_id_part};

const KIRO_AUTH_DIR_NAME: &str = "kiro-auth";

pub(crate) struct KiroAccountStore {
    dir: PathBuf,
    cache: RwLock<HashMap<String, KiroTokenRecord>>,
    app_proxy: AppProxyState,
}

impl KiroAccountStore {
    pub(crate) fn new(app: &AppHandle, app_proxy: AppProxyState) -> Result<Self, String> {
        let dir = config_dir_path(app)?.join(KIRO_AUTH_DIR_NAME);
        Ok(Self {
            dir,
            cache: RwLock::new(HashMap::new()),
            app_proxy,
        })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) async fn import_ide_tokens(
        &self,
        app: &AppHandle,
    ) -> Result<Vec<KiroAccountSummary>, String> {
        let home_dir = resolve_home_dir(app)?;
        let cache_dir = home_dir.join(".aws").join("sso").join("cache");
        // Mirror CLIProxyAPIPlus: only import ~/.aws/sso/cache/kiro-auth-token.json.
        let mut entries = match tokio::fs::read_dir(&cache_dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err("No Kiro IDE token files found.".to_string());
            }
            Err(err) => {
                return Err(format!("Failed to read Kiro IDE token directory: {err}"));
            }
        };
        let mut imported = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read Kiro IDE token entry: {err}"))?
        {
            let path = entry.path();
            let file_name = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name,
                None => continue,
            };
            if file_name != "kiro-auth-token.json" {
                continue;
            }
            let contents = match tokio::fs::read_to_string(&path).await {
                Ok(contents) => contents,
                Err(_) => continue,
            };
            let token: KiroIdeTokenFile = match serde_json::from_str(&contents) {
                Ok(token) => token,
                Err(_) => continue,
            };
            let record = match token.into_record() {
                Ok(record) => record,
                Err(_) => continue,
            };
            if let Ok(summary) = self.save_new_account(record).await {
                imported.push(summary);
            }
        }
        if imported.is_empty() {
            return Err("No valid Kiro IDE token file found.".to_string());
        }
        Ok(imported)
    }

    pub(crate) async fn list_accounts(&self) -> Result<Vec<KiroAccountSummary>, String> {
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

fn resolve_home_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(dir) = app.path().home_dir() {
        return Ok(dir);
    }
    if let Some(dir) = std::env::var_os("HOME").map(PathBuf::from) {
        return Ok(dir);
    }
    Err("Failed to resolve home directory.".to_string())
}
