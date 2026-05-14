use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, RwLock};

use crate::app_proxy::AppProxyState;
use crate::oauth_util::{
    decode_jwt_payload, expires_at_from_seconds, extract_chatgpt_account_id_from_jwt,
    extract_chatgpt_user_id_from_jwt, extract_email_from_jwt, normalize_proxy_url, now_rfc3339,
    sanitize_id_part,
};
use crate::paths::TokenProxyPaths;
use crate::provider_accounts;

use super::oauth::CodexOAuthClient;
use super::types::{CodexAccountStatus, CodexAccountSummary, CodexTokenRecord};

pub struct CodexAccountStore {
    paths: TokenProxyPaths,
    cache: RwLock<HashMap<String, CodexTokenRecord>>,
    app_proxy: AppProxyState,
    quota_refreshing: Mutex<HashSet<String>>,
}

impl CodexAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        Ok(Self {
            paths: paths.clone(),
            cache: RwLock::new(HashMap::new()),
            app_proxy,
            quota_refreshing: Mutex::new(HashSet::new()),
        })
    }

    pub async fn list_accounts(&self) -> Result<Vec<CodexAccountSummary>, String> {
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
                status: record.effective_status(),
                auto_refresh_enabled: record.auto_refresh_enabled,
                proxy_url: record.proxy_url.clone(),
                priority: record.priority,
            })
            .collect();
        items.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.account_id.cmp(&right.account_id))
        });
        Ok(items)
    }

    pub async fn import_file(&self, path: PathBuf) -> Result<Vec<CodexAccountSummary>, String> {
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
            let records = match parse_import_records(&contents) {
                Ok(records) => records,
                Err(err) if metadata.is_dir() => {
                    let _ = err;
                    continue;
                }
                Err(err) => return Err(err),
            };
            for record in records {
                if let Ok(summary) = self.save_new_account(record).await {
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

    pub(crate) async fn get_account_record(
        &self,
        account_id: &str,
    ) -> Result<CodexTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub async fn refresh_account(&self, account_id: &str) -> Result<(), String> {
        let record = self.load_account(account_id).await?;
        if record.refresh_token.trim().is_empty() {
            return Err("Codex account has no refresh token. Please sign in again.".to_string());
        }
        let refreshed = self.refresh_record(account_id, record).await?;
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
        record.auto_refresh_enabled = enabled;
        self.save_record(account_id.to_string(), record).await
    }

    pub async fn set_status(
        &self,
        account_id: &str,
        status: CodexAccountStatus,
    ) -> Result<CodexAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        record.status = status;
        self.save_record(account_id.to_string(), record).await
    }

    pub async fn set_proxy_url(
        &self,
        account_id: &str,
        proxy_url: Option<&str>,
    ) -> Result<CodexAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        record.proxy_url = normalize_proxy_url(proxy_url)?;
        self.save_record(account_id.to_string(), record).await
    }

    pub async fn set_priority(
        &self,
        account_id: &str,
        priority: i32,
    ) -> Result<CodexAccountSummary, String> {
        let mut record = self.load_account(account_id).await?;
        record.priority = priority;
        self.save_record(account_id.to_string(), record).await
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        provider_accounts::upsert_codex_account(&self.paths, &account_id, &record).await?;
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
            status: record.effective_status(),
            auto_refresh_enabled: record.auto_refresh_enabled,
            proxy_url: record.proxy_url.clone(),
            priority: record.priority,
        })
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

    pub(crate) async fn save_new_account(
        &self,
        mut record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        fill_record_from_jwt(&mut record);
        if let Some((existing_local_account_id, existing_record)) =
            self.find_existing_import_target(&record).await?
        {
            // Re-importing the same real Codex account should refresh credentials in place
            // instead of creating duplicate local entries. Keep app-local settings.
            record.auto_refresh_enabled = existing_record.auto_refresh_enabled;
            record.status = existing_record.status;
            if record.proxy_url.is_none() {
                record.proxy_url = existing_record.proxy_url.clone();
            }
            record.priority = existing_record.priority;
            return self.save_record(existing_local_account_id, record).await;
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
        let account_id = self.unique_account_id(&id_part).await?;
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
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        if !record_needs_refresh(&record) {
            return Ok(record);
        }
        if !record.auto_refresh_enabled {
            return Ok(record);
        }
        // Allow imported access-token-only records to stay usable until expiry.
        // When refresh_token is missing we should not fail reads/listing by forcing refresh.
        if record.refresh_token.trim().is_empty() {
            return Ok(record);
        }
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        let proxy_url = self.effective_proxy_url(record.proxy_url.as_deref()).await;
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
            auto_refresh_enabled: record.auto_refresh_enabled,
            status: record.status,
            account_id: record.account_id.clone(),
            user_id: record.user_id.clone(),
            email: record.email.clone(),
            expires_at: expires_at_from_seconds(response.expires_in),
            last_refresh: Some(now_rfc3339()),
            proxy_url: record.proxy_url.clone(),
            priority: record.priority,
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
    ) -> Result<(String, CodexTokenRecord), String> {
        self.resolve_account_record_with_order(account_id, None)
            .await
    }

    pub(crate) async fn resolve_account_record_with_order(
        &self,
        account_id: Option<&str>,
        ordered_account_ids: Option<&[String]>,
    ) -> Result<(String, CodexTokenRecord), String> {
        if let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) {
            let record = self.get_account_record(account_id).await?;
            if matches!(record.effective_status(), CodexAccountStatus::Disabled) {
                return Err(format!("Codex account is disabled: {account_id}"));
            }
            if matches!(record.effective_status(), CodexAccountStatus::Expired) {
                return Err(format!("Codex account is expired: {account_id}"));
            }
            return Ok((account_id.to_string(), record));
        }

        self.refresh_cache().await?;
        let account_ids = if let Some(ordered_account_ids) = ordered_account_ids {
            ordered_account_ids.to_vec()
        } else {
            let cache = self.cache.read().await;
            sorted_account_ids(&cache)
        };

        let mut last_error = None;
        for account_id in account_ids {
            match self.get_account_record(&account_id).await {
                Ok(record) if record.is_schedulable() => {
                    return Ok((account_id, record));
                }
                Ok(record) if matches!(record.effective_status(), CodexAccountStatus::Disabled) => {
                    last_error = Some(format!("Codex account is disabled: {account_id}"));
                }
                Ok(_) => {
                    last_error = Some(format!("Codex account is expired: {account_id}"));
                }
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "Codex account is not configured.".to_string()))
    }

    pub(crate) async fn resolve_next_account_record_with_order(
        &self,
        excluded_account_ids: &[String],
        ordered_account_ids: Option<&[String]>,
    ) -> Result<Option<(String, CodexTokenRecord)>, String> {
        self.refresh_cache().await?;
        let account_ids = if let Some(ordered_account_ids) = ordered_account_ids {
            ordered_account_ids.to_vec()
        } else {
            let cache = self.cache.read().await;
            sorted_account_ids(&cache)
        };

        for account_id in account_ids {
            if excluded_account_ids
                .iter()
                .any(|value| value == &account_id)
            {
                continue;
            }
            match self.get_account_record(&account_id).await {
                Ok(record) if record.is_schedulable() => {
                    return Ok(Some((account_id, record)));
                }
                Ok(_) | Err(_) => continue,
            }
        }

        Ok(None)
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
        let cache = provider_accounts::list_codex_records(&self.paths).await?;
        let mut guard = self.cache.write().await;
        *guard = cache;
        Ok(())
    }

    async fn unique_account_id(&self, id_part: &str) -> Result<String, String> {
        self.refresh_cache().await?;
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

    async fn find_existing_import_target(
        &self,
        imported: &CodexTokenRecord,
    ) -> Result<Option<(String, CodexTokenRecord)>, String> {
        self.refresh_cache().await?;
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
    let mut entries = cache.iter().collect::<Vec<_>>();
    entries.sort_by(|(left_id, left_record), (right_id, right_record)| {
        right_record
            .priority
            .cmp(&left_record.priority)
            .then_with(|| left_id.cmp(right_id))
    });
    entries
        .into_iter()
        .map(|(account_id, _)| account_id.clone())
        .collect()
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
    // JWT claims are the authority; wrapper fields from exported files are fallbacks.
    if let Some(account_id) = extract_chatgpt_account_id_from_jwt(&record.id_token)
        .or_else(|| extract_chatgpt_account_id_from_jwt(&record.access_token))
    {
        record.account_id = Some(account_id);
    }
    if let Some(user_id) = extract_chatgpt_user_id_from_jwt(&record.id_token)
        .or_else(|| extract_chatgpt_user_id_from_jwt(&record.access_token))
    {
        record.user_id = Some(user_id);
    }
    if let Some(email) = extract_email_from_jwt(&record.id_token)
        .or_else(|| extract_email_from_jwt(&record.access_token))
    {
        record.email = Some(email);
    }
}

fn record_needs_refresh(record: &CodexTokenRecord) -> bool {
    record.is_expired() || paid_quota_disagrees_with_free_access_token_claim(record)
}

fn paid_quota_disagrees_with_free_access_token_claim(record: &CodexTokenRecord) -> bool {
    if !is_paid_plan(record.quota.plan_type.as_deref()) {
        return false;
    }
    matches!(
        extract_chatgpt_plan_type_from_jwt(&record.access_token).as_deref(),
        Some("free")
    )
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

fn parse_import_records(contents: &str) -> Result<Vec<CodexTokenRecord>, String> {
    let value: Value = serde_json::from_str(contents)
        .map_err(|err| format!("Invalid Codex account JSON file: {err}"))?;
    let mut records = Vec::new();
    collect_import_records(&value, &mut records);
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
            &["token", "access_token"],
            &["token_data", "access_token"],
        ],
    )?;
    let refresh_token = find_string(
        value,
        &[
            &["refresh_token"],
            &["token", "refresh_token"],
            &["token_data", "refresh_token"],
        ],
    )
    .unwrap_or_default();
    let id_token = find_string(
        value,
        &[
            &["id_token"],
            &["token", "id_token"],
            &["token_data", "id_token"],
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
            &["token", "expires_at"],
            &["token", "expired"],
            &["token_data", "expires_at"],
            &["token_data", "expired"],
        ],
    )
    .or_else(|| {
        find_i64(
            value,
            &[
                &["expires_in"],
                &["token", "expires_in"],
                &["token_data", "expires_in"],
            ],
        )
        .map(expires_at_from_seconds)
    })?;

    let account_id = find_string(
        value,
        &[
            &["account_id"],
            &["chatgpt_account_id"],
            &["account", "uuid"],
            &["account", "id"],
            &["token_data", "account_id"],
            &["data", "account_id"],
        ],
    );
    let user_id = find_string(
        value,
        &[
            &["user_id"],
            &["chatgpt_user_id"],
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

    Some(CodexTokenRecord {
        access_token,
        refresh_token,
        id_token,
        auto_refresh_enabled,
        status: CodexAccountStatus::Active,
        account_id,
        user_id,
        email,
        expires_at,
        last_refresh,
        proxy_url: None,
        priority: find_i64(
            value,
            &[
                &["priority"],
                &["token_data", "priority"],
                &["data", "priority"],
            ],
        )
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_proxy;
    use crate::codex::CodexQuotaCache;
    use crate::paths::TokenProxyPaths;
    use crate::proxy::sqlite;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use rand::random;
    use serde_json::json;
    use sqlx::Row;
    use std::future::Future;
    use std::path::PathBuf;
    use time::format_description::well_known::Rfc3339;

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

    fn build_record_with_quota_and_access_claim(
        quota_plan_type: &str,
        access_claim_plan_type: &str,
    ) -> CodexTokenRecord {
        CodexTokenRecord {
            access_token: build_access_token_with_plan(access_claim_plan_type),
            refresh_token: "refresh-token".to_string(),
            id_token: build_id_token("paid@example.com", "acct-paid"),
            auto_refresh_enabled: true,
            status: CodexAccountStatus::Active,
            account_id: Some("acct-paid".to_string()),
            user_id: None,
            email: Some("paid@example.com".to_string()),
            expires_at: future_rfc3339(24),
            last_refresh: None,
            proxy_url: None,
            priority: 0,
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
            assert_eq!(record.expires_at, expires_at);
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
            assert_eq!(record.id_token, "");

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
            assert_eq!(record.refresh_token, "");

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
            store
                .set_proxy_url(&first_local_account_id, Some("http://127.0.0.1:7890"))
                .await
                .expect("set proxy url should succeed");

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
            assert_eq!(
                accounts[0].proxy_url.as_deref(),
                Some("http://127.0.0.1:7890")
            );

            let record = store
                .get_account_record(&first_local_account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.access_token, "second-access-token");
            assert_eq!(record.refresh_token, "second-refresh-token");
            assert_eq!(record.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn list_accounts_orders_by_priority_descending() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
            let pool = sqlite::open_write_pool(&paths)
                .await
                .expect("open sqlite pool");
            let columns = sqlx::query("PRAGMA table_info(provider_accounts);")
                .fetch_all(&pool)
                .await
                .expect("read provider_accounts schema");
            let has_priority = columns
                .into_iter()
                .any(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("priority"));
            if !has_priority {
                sqlx::query(
                    "ALTER TABLE provider_accounts ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;",
                )
                .execute(&pool)
                .await
                .expect("add priority column");
            }

            let high_expires_at = future_rfc3339(6);
            let low_expires_at = future_rfc3339(6);
            sqlx::query(
                r#"
INSERT INTO provider_accounts (
  provider_kind,
  account_id,
  email,
  expires_at,
  expires_at_ms,
  auth_method,
  provider_name,
  record_json,
  updated_at_ms,
  priority
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
"#,
            )
            .bind("codex")
            .bind("codex-a-low.json")
            .bind("low@example.com")
            .bind(low_expires_at.as_str())
            .bind(0_i64)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(
                json!({
                    "access_token": "access-low",
                    "refresh_token": "refresh-low",
                    "id_token": build_id_token("low@example.com", "acct-low"),
                    "auto_refresh_enabled": true,
                    "status": "active",
                    "account_id": "acct-low",
                    "email": "low@example.com",
                    "expires_at": low_expires_at,
                    "last_refresh": null,
                    "proxy_url": null,
                    "priority": 1,
                    "quota": {"plan_type": null, "quotas": [], "error": null, "checked_at": null}
                })
                .to_string(),
            )
            .bind(0_i64)
            .bind(1_i64)
            .execute(&pool)
            .await
            .expect("insert low priority account");

            sqlx::query(
                r#"
INSERT INTO provider_accounts (
  provider_kind,
  account_id,
  email,
  expires_at,
  expires_at_ms,
  auth_method,
  provider_name,
  record_json,
  updated_at_ms,
  priority
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
"#,
            )
            .bind("codex")
            .bind("codex-z-high.json")
            .bind("high@example.com")
            .bind(high_expires_at.as_str())
            .bind(0_i64)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(
                json!({
                    "access_token": "access-high",
                    "refresh_token": "refresh-high",
                    "id_token": build_id_token("high@example.com", "acct-high"),
                    "auto_refresh_enabled": true,
                    "status": "active",
                    "account_id": "acct-high",
                    "email": "high@example.com",
                    "expires_at": high_expires_at,
                    "last_refresh": null,
                    "proxy_url": null,
                    "priority": 9,
                    "quota": {"plan_type": null, "quotas": [], "error": null, "checked_at": null}
                })
                .to_string(),
            )
            .bind(0_i64)
            .bind(9_i64)
            .execute(&pool)
            .await
            .expect("insert high priority account");

            let accounts = store.list_accounts().await.expect("list accounts");
            let ordered_ids = accounts
                .into_iter()
                .map(|item| item.account_id)
                .collect::<Vec<_>>();
            assert_eq!(
                ordered_ids,
                vec![
                    "codex-z-high.json".to_string(),
                    "codex-a-low.json".to_string()
                ]
            );

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_overwrite_preserves_existing_priority_in_record_json() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
            let pool = sqlite::open_write_pool(&paths)
                .await
                .expect("open sqlite pool");
            let columns = sqlx::query("PRAGMA table_info(provider_accounts);")
                .fetch_all(&pool)
                .await
                .expect("read provider_accounts schema");
            let has_priority = columns
                .into_iter()
                .any(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("priority"));
            if !has_priority {
                sqlx::query(
                    "ALTER TABLE provider_accounts ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;",
                )
                .execute(&pool)
                .await
                .expect("add priority column");
            }

            let existing_expires_at = future_rfc3339(6);
            sqlx::query(
                r#"
INSERT INTO provider_accounts (
  provider_kind,
  account_id,
  email,
  expires_at,
  expires_at_ms,
  auth_method,
  provider_name,
  record_json,
  updated_at_ms,
  priority
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
"#,
            )
            .bind("codex")
            .bind("codex-priority.json")
            .bind("overwrite@example.com")
            .bind(existing_expires_at.as_str())
            .bind(0_i64)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(
                json!({
                    "access_token": "existing-access",
                    "refresh_token": "existing-refresh",
                    "id_token": build_id_token("overwrite@example.com", "acct-overwrite"),
                    "auto_refresh_enabled": true,
                    "status": "active",
                    "account_id": "acct-overwrite",
                    "email": "overwrite@example.com",
                    "expires_at": existing_expires_at,
                    "last_refresh": null,
                    "proxy_url": null,
                    "priority": 7,
                    "quota": {"plan_type": null, "quotas": [], "error": null, "checked_at": null}
                })
                .to_string(),
            )
            .bind(0_i64)
            .bind(7_i64)
            .execute(&pool)
            .await
            .expect("insert existing account");

            let input_path = data_dir.join("codex-overwrite.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "new-access-token",
                    "refresh_token": "new-refresh-token",
                    "account_id": "acct-overwrite",
                    "email": "overwrite@example.com",
                    "expired": future_rfc3339(12),
                }))
                .expect("serialize overwrite json"),
            )
            .await
            .expect("write overwrite json");

            store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            let row = sqlx::query(
                "SELECT record_json, priority FROM provider_accounts WHERE account_id = ?;",
            )
            .bind("codex-priority.json")
            .fetch_one(&pool)
            .await
            .expect("select overwritten record");
            let record_json = row
                .try_get::<String, _>("record_json")
                .expect("decode record_json");
            let value: serde_json::Value =
                serde_json::from_str(&record_json).expect("parse record json");
            assert_eq!(
                value.get("priority").and_then(serde_json::Value::as_i64),
                Some(7)
            );
            assert_eq!(
                row.try_get::<i64, _>("priority").expect("decode priority"),
                7
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
            assert_eq!(record.refresh_token, "");

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
            assert!(!imported[0].auto_refresh_enabled);

            let updated = store
                .set_auto_refresh(&imported[0].account_id, true)
                .await
                .expect("set auto refresh should succeed");
            assert!(updated.auto_refresh_enabled);

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert!(record.auto_refresh_enabled);

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn set_enabled_updates_record_flag() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expires_at = future_rfc3339(6);
            let input_path = data_dir.join("codex-enabled.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "account_id": "acct-enabled",
                    "email": "enabled@example.com",
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
            assert!(matches!(imported[0].status, CodexAccountStatus::Active));

            let updated = store
                .set_status(&imported[0].account_id, CodexAccountStatus::Disabled)
                .await
                .expect("set status should succeed");
            assert!(matches!(updated.status, CodexAccountStatus::Disabled));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert!(matches!(record.status, CodexAccountStatus::Disabled));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn resolve_account_record_skips_disabled_accounts() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let first = CodexTokenRecord {
                access_token: "access-1".to_string(),
                refresh_token: "refresh-1".to_string(),
                id_token: "".to_string(),
                auto_refresh_enabled: true,
                status: CodexAccountStatus::Disabled,
                account_id: Some("acct-disabled".to_string()),
                user_id: None,
                email: Some("aaa@example.com".to_string()),
                expires_at: future_rfc3339(6),
                last_refresh: None,
                proxy_url: None,
                priority: 0,
                quota: crate::codex::CodexQuotaCache::default(),
            };
            let second = CodexTokenRecord {
                access_token: "access-2".to_string(),
                refresh_token: "refresh-2".to_string(),
                id_token: "".to_string(),
                auto_refresh_enabled: true,
                status: CodexAccountStatus::Active,
                account_id: Some("acct-enabled".to_string()),
                user_id: None,
                email: Some("zzz@example.com".to_string()),
                expires_at: future_rfc3339(6),
                last_refresh: None,
                proxy_url: None,
                priority: 0,
                quota: crate::codex::CodexQuotaCache::default(),
            };

            store
                .save_record("codex-a.json".to_string(), first)
                .await
                .expect("save first account");
            store
                .save_record("codex-b.json".to_string(), second)
                .await
                .expect("save second account");

            let (account_id, record) = store
                .resolve_account_record(None)
                .await
                .expect("should resolve enabled account");

            assert_eq!(account_id, "codex-b.json");
            assert!(record.is_schedulable());

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn set_proxy_url_updates_record_value() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("codex-set-proxy-url.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "account_id": "acct-set-proxy-url",
                    "email": "proxy@example.com",
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

            let updated = store
                .set_proxy_url(&imported[0].account_id, Some("socks5://127.0.0.1:1080"))
                .await
                .expect("set proxy url should succeed");
            assert_eq!(
                updated.proxy_url.as_deref(),
                Some("socks5://127.0.0.1:1080")
            );

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.proxy_url.as_deref(), Some("socks5://127.0.0.1:1080"));

            let cleared = store
                .set_proxy_url(&imported[0].account_id, None::<&str>)
                .await
                .expect("clear proxy url should succeed");
            assert_eq!(cleared.proxy_url, None);

            let cleared_record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should still exist");
            assert_eq!(cleared_record.proxy_url, None);

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
