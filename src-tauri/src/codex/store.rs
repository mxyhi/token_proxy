use std::collections::HashMap;
use std::path::PathBuf;

use tauri::AppHandle;
use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::oauth_util::{
    expires_at_from_seconds,
    extract_chatgpt_account_id_from_jwt,
    extract_email_from_jwt,
    now_rfc3339,
    sanitize_id_part,
};
use crate::proxy::config::config_dir_path;

use super::oauth::CodexOAuthClient;
use super::types::{CodexAccountStatus, CodexAccountSummary, CodexTokenRecord};

const CODEX_AUTH_DIR_NAME: &str = "codex-auth";

pub(crate) struct CodexAccountStore {
    dir: PathBuf,
    cache: RwLock<HashMap<String, CodexTokenRecord>>,
    app_proxy: AppProxyState,
}

impl CodexAccountStore {
    pub(crate) fn new(app: &AppHandle, app_proxy: AppProxyState) -> Result<Self, String> {
        let dir = config_dir_path(app)?.join(CODEX_AUTH_DIR_NAME);
        Ok(Self {
            dir,
            cache: RwLock::new(HashMap::new()),
            app_proxy,
        })
    }

    pub(crate) async fn list_accounts(&self) -> Result<Vec<CodexAccountSummary>, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let mut items: Vec<CodexAccountSummary> = cache
            .iter()
            .map(|(account_id, record)| CodexAccountSummary {
                account_id: account_id.clone(),
                email: record.email.clone(),
                expires_at: record.expires_at().map(|value| {
                    value
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| record.expires_at.clone())
                }),
                status: record.status(),
            })
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub(crate) async fn get_account_record(
        &self,
        account_id: &str,
    ) -> Result<CodexTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        self.ensure_dir().await?;
        let path = self.account_path(&account_id);
        let payload = serde_json::to_string_pretty(&record)
            .map_err(|err| format!("Failed to serialize token record: {err}"))?;
        tokio::fs::write(&path, payload)
            .await
            .map_err(|err| format!("Failed to write token record: {err}"))?;
        let mut cache = self.cache.write().await;
        cache.insert(account_id.clone(), record.clone());
        Ok(CodexAccountSummary {
            account_id,
            email: record.email.clone(),
            expires_at: record.expires_at().map(|value| {
                value
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| record.expires_at.clone())
            }),
            status: record.status(),
        })
    }

    pub(crate) async fn save_new_account(
        &self,
        mut record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        fill_record_from_jwt(&mut record);
        let id_part_source = record
            .email
            .as_deref()
            .or(record.account_id.as_deref())
            .unwrap_or_default();
        let mut id_part = sanitize_id_part(id_part_source);
        if id_part.is_empty() {
            id_part = format!("{}", OffsetDateTime::now_utc().unix_timestamp());
        }
        let account_id = self.unique_account_id(&id_part).await?;
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
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        if !record.is_expired() {
            return Ok(record);
        }
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        let proxy_url = self.app_proxy_url().await;
        let client = CodexOAuthClient::new(proxy_url.as_deref())?;
        let response = client.refresh_token(&record.refresh_token).await?;
        let mut refreshed = CodexTokenRecord {
            access_token: response.access_token,
            refresh_token: if response.refresh_token.trim().is_empty() {
                record.refresh_token.clone()
            } else {
                response.refresh_token
            },
            id_token: if response.id_token.trim().is_empty() {
                record.id_token.clone()
            } else {
                response.id_token
            },
            account_id: record.account_id.clone(),
            email: record.email.clone(),
            expires_at: expires_at_from_seconds(response.expires_in),
            last_refresh: Some(now_rfc3339()),
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

    async fn load_account(&self, account_id: &str) -> Result<CodexTokenRecord, String> {
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
            Err(err) => return Err(format!("Failed to read Codex auth directory: {err}")),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read Codex auth entry: {err}"))?
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
            let record: CodexTokenRecord = match serde_json::from_str(&contents) {
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
            .map_err(|err| format!("Failed to create Codex auth dir: {err}"))
    }

    async fn unique_account_id(&self, id_part: &str) -> Result<String, String> {
        self.ensure_dir().await?;
        let mut suffix = 0u32;
        loop {
            let candidate = if suffix == 0 {
                format!("codex-{id_part}.json")
            } else {
                format!("codex-{id_part}-{suffix}.json")
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

fn fill_record_from_jwt(record: &mut CodexTokenRecord) {
    if record.account_id.is_none() {
        record.account_id = extract_chatgpt_account_id_from_jwt(&record.id_token);
    }
    if record.email.is_none() {
        record.email = extract_email_from_jwt(&record.id_token);
    }
}
