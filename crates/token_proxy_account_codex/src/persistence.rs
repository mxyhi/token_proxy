//! Codex mapping onto provider-independent account records.

use std::collections::HashMap;

use serde::Deserialize;
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_account_store::records::{self, AccountRecordMetadata, ProviderKind};

use crate::types::{CodexAccountStatus, CodexCredential, CodexQuotaCache, CodexTokenRecord};

pub async fn upsert_codex_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: &CodexTokenRecord,
) -> Result<(), String> {
    records::upsert_record(
        paths,
        ProviderKind::Codex,
        AccountRecordMetadata {
            account_id,
            email: record.email.as_deref(),
            expires_at: record.expires_at_str(),
            expires_at_ms: record.expires_at().map(records::unix_millis),
            auth_method: Some(record.auth_method().as_str()),
            provider_name: None,
        },
        record,
    )
    .await
}

pub async fn list_codex_records(
    paths: &TokenProxyPaths,
) -> Result<HashMap<String, CodexTokenRecord>, String> {
    let raw_records =
        records::list_records::<serde_json::Value>(paths, ProviderKind::Codex).await?;
    let mut migrated = HashMap::with_capacity(raw_records.len());
    for (account_id, value) in raw_records {
        let (record, was_legacy) = match serde_json::from_value::<CodexTokenRecord>(value.clone()) {
            Ok(record) => (record, false),
            Err(canonical_error) => {
                let legacy = serde_json::from_value::<LegacyCodexTokenRecord>(value).map_err(|legacy_error| {
                    format!(
                        "Failed to deserialize Codex account {account_id}: canonical={canonical_error}; legacy={legacy_error}"
                    )
                })?;
                (legacy.into_canonical(), true)
            }
        };
        if was_legacy {
            // Persist the canonical shape immediately so legacy handling remains a one-time read migration.
            upsert_codex_account(paths, &account_id, &record).await?;
            tracing::info!(
                account_id,
                "codex oauth account migrated to canonical credential format"
            );
        }
        migrated.insert(account_id, record);
    }
    Ok(migrated)
}

pub async fn delete_account(paths: &TokenProxyPaths, account_id: &str) -> Result<(), String> {
    records::delete_record(paths, ProviderKind::Codex, account_id).await
}

/// 单事务替换该 provider 全部记录（编排层 restore_all）。
pub async fn replace_all_codex_records(
    paths: &TokenProxyPaths,
    snapshot: &HashMap<String, CodexTokenRecord>,
) -> Result<(), String> {
    let mut rows = Vec::with_capacity(snapshot.len());
    for (account_id, record) in snapshot {
        let record_json = serde_json::to_string(record)
            .map_err(|error| format!("Failed to serialize codex account {account_id}: {error}"))?;
        rows.push(records::ProviderSnapshotRow {
            account_id: account_id.to_string(),
            email: record.email.clone(),
            expires_at: record.expires_at_str().map(str::to_string),
            expires_at_ms: record.expires_at().map(records::unix_millis),
            auth_method: Some(record.auth_method().as_str().to_string()),
            provider_name: None,
            record_json,
        });
    }
    records::replace_provider_snapshot(paths, ProviderKind::Codex, &rows).await
}

/// 单账户原子 restore：`None` 删除。
pub async fn replace_codex_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: Option<&CodexTokenRecord>,
) -> Result<(), String> {
    let metadata = record.map(|r| AccountRecordMetadata {
        account_id,
        email: r.email.as_deref(),
        expires_at: r.expires_at_str(),
        expires_at_ms: r.expires_at().map(records::unix_millis),
        auth_method: Some(r.auth_method().as_str()),
        provider_name: None,
    });
    records::replace_single_record(paths, ProviderKind::Codex, account_id, record, metadata).await
}

#[derive(Deserialize)]
struct LegacyCodexTokenRecord {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    id_token: String,
    #[serde(default = "legacy_auto_refresh_enabled")]
    auto_refresh_enabled: bool,
    #[serde(default = "legacy_account_status")]
    status: CodexAccountStatus,
    account_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    openai_device_id: Option<String>,
    email: Option<String>,
    expires_at: String,
    last_refresh: Option<String>,
    #[serde(default)]
    quota: CodexQuotaCache,
}

impl LegacyCodexTokenRecord {
    fn into_canonical(self) -> CodexTokenRecord {
        CodexTokenRecord {
            credential: CodexCredential::Oauth {
                access_token: self.access_token,
                refresh_token: self.refresh_token,
                client_id: self.client_id,
                id_token: self.id_token,
                auto_refresh_enabled: self.auto_refresh_enabled,
                openai_device_id: self.openai_device_id,
                expires_at: self.expires_at,
                last_refresh: self.last_refresh,
            },
            // 历史 manual disabled 不再参与调度，按健康态默认 Active 读入。
            status: match self.status {
                CodexAccountStatus::Invalid => CodexAccountStatus::Invalid,
                CodexAccountStatus::Expired => CodexAccountStatus::Expired,
                CodexAccountStatus::Active => CodexAccountStatus::Active,
            },
            account_id: self.account_id,
            user_id: self.user_id,
            email: self.email,
            quota: self.quota,
        }
    }
}

fn legacy_auto_refresh_enabled() -> bool {
    true
}

fn legacy_account_status() -> CodexAccountStatus {
    CodexAccountStatus::Active
}
