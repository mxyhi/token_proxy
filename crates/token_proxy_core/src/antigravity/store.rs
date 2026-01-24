use std::collections::HashMap;
use std::path::PathBuf;

use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::oauth_util::{expires_at_from_seconds, sanitize_id_part};
use crate::paths::TokenProxyPaths;

use super::oauth::AntigravityOAuthClient;
use super::types::{
    AntigravityAccountSummary, AntigravityAccountStatus, AntigravityTokenRecord,
};

const ANTIGRAVITY_AUTH_DIR_NAME: &str = "antigravity-auth";

pub struct AntigravityAccountStore {
    dir: PathBuf,
    cache: RwLock<HashMap<String, AntigravityTokenRecord>>,
    app_proxy: AppProxyState,
}

impl AntigravityAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        let dir = paths.data_dir().join(ANTIGRAVITY_AUTH_DIR_NAME);
        Ok(Self {
            dir,
            cache: RwLock::new(HashMap::new()),
            app_proxy,
        })
    }

    pub async fn list_accounts(&self) -> Result<Vec<AntigravityAccountSummary>, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let mut items: Vec<AntigravityAccountSummary> = cache
            .iter()
            .map(|(account_id, record)| AntigravityAccountSummary {
                account_id: account_id.clone(),
                email: record.email.clone(),
                expires_at: record.expires_at().map(|value| {
                    value
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| record.expired.clone().unwrap_or_default())
                }),
                status: record.status(),
                source: record.source.clone(),
            })
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub(crate) async fn get_account_record(
        &self,
        account_id: &str,
    ) -> Result<AntigravityTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub(crate) async fn save_new_account(
        &self,
        record: AntigravityTokenRecord,
    ) -> Result<AntigravityAccountSummary, String> {
        let id_part_source = record
            .email
            .as_deref()
            .or(record.source.as_deref())
            .unwrap_or_default();
        let mut id_part = sanitize_id_part(id_part_source);
        if id_part.is_empty() {
            id_part = format!("{}", OffsetDateTime::now_utc().unix_timestamp());
        }
        let account_id = self.unique_account_id(&id_part).await?;
        self.save_record(account_id, record).await
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: AntigravityTokenRecord,
    ) -> Result<AntigravityAccountSummary, String> {
        self.ensure_dir().await?;
        let path = self.account_path(&account_id);
        let payload = serde_json::to_string_pretty(&record)
            .map_err(|err| format!("Failed to serialize token record: {err}"))?;
        tokio::fs::write(&path, payload)
            .await
            .map_err(|err| format!("Failed to write token record: {err}"))?;
        let mut cache = self.cache.write().await;
        cache.insert(account_id.clone(), record.clone());
        Ok(AntigravityAccountSummary {
            account_id,
            email: record.email.clone(),
            expires_at: record.expires_at().map(|value| {
                value
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| record.expired.clone().unwrap_or_default())
            }),
            status: record.status(),
            source: record.source.clone(),
        })
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

    pub(crate) async fn update_project_id(
        &self,
        account_id: &str,
        project_id: String,
    ) -> Result<(), String> {
        let mut record = self.get_account_record(account_id).await?;
        record.project_id = Some(project_id);
        let _ = self.save_record(account_id.to_string(), record).await?;
        Ok(())
    }

    async fn refresh_if_needed(
        &self,
        account_id: &str,
        record: AntigravityTokenRecord,
    ) -> Result<AntigravityTokenRecord, String> {
        if !record.is_expired() {
            return Ok(record);
        }
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: AntigravityTokenRecord,
    ) -> Result<AntigravityTokenRecord, String> {
        let refresh_token = record
            .refresh_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Antigravity refresh token is missing.".to_string())?;
        let proxy_url = self.app_proxy_url().await;
        let client = AntigravityOAuthClient::new(proxy_url);
        let response = client.refresh_token(refresh_token).await?;
        let refreshed = AntigravityTokenRecord {
            access_token: response.access_token,
            refresh_token: response
                .refresh_token
                .filter(|value| !value.trim().is_empty())
                .or(record.refresh_token.clone()),
            expired: Some(expires_at_from_seconds(response.expires_in)),
            expires_in: Some(response.expires_in),
            timestamp: Some(OffsetDateTime::now_utc().unix_timestamp() * 1000),
            email: record.email.clone(),
            token_type: response.token_type.or(record.token_type.clone()),
            project_id: record.project_id.clone(),
            source: record.source.clone().or_else(|| Some("oauth".to_string())),
        };
        let summary = self
            .save_record(account_id.to_string(), refreshed.clone())
            .await?;
        if matches!(summary.status, AntigravityAccountStatus::Expired) {
            return Err("Antigravity token refresh failed.".to_string());
        }
        Ok(refreshed)
    }

    async fn load_account(&self, account_id: &str) -> Result<AntigravityTokenRecord, String> {
        if let Some(record) = self.cache.read().await.get(account_id).cloned() {
            return Ok(record);
        }
        self.refresh_cache().await?;
        self.cache
            .read()
            .await
            .get(account_id)
            .cloned()
            .ok_or_else(|| format!("Antigravity account not found: {account_id}"))
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
            Err(err) => return Err(format!("Failed to read Antigravity auth directory: {err}")),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read Antigravity auth entry: {err}"))?
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
            let record: AntigravityTokenRecord = match serde_json::from_str(&contents) {
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
            .map_err(|err| format!("Failed to create Antigravity auth dir: {err}"))
    }

    async fn unique_account_id(&self, id_part: &str) -> Result<String, String> {
        self.ensure_dir().await?;
        let mut suffix = 0u32;
        loop {
            let candidate = if suffix == 0 {
                format!("antigravity-{id_part}.json")
            } else {
                format!("antigravity-{id_part}-{suffix}.json")
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
