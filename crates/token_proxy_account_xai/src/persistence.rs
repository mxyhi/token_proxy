//! xAI mapping onto provider-independent account records.

use std::collections::HashMap;

use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_account_store::records::{self, AccountRecordMetadata, ProviderKind};

use crate::types::XaiTokenRecord;

pub async fn upsert_xai_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: &XaiTokenRecord,
) -> Result<(), String> {
    records::upsert_record(
        paths,
        ProviderKind::Xai,
        AccountRecordMetadata {
            account_id,
            email: record.email.as_deref(),
            expires_at: Some(record.expires_at.as_str()),
            expires_at_ms: record.expires_at().map(records::unix_millis),
            auth_method: Some("oauth"),
            provider_name: Some("xai"),
        },
        record,
    )
    .await
}

pub async fn list_xai_records(
    paths: &TokenProxyPaths,
) -> Result<HashMap<String, XaiTokenRecord>, String> {
    records::list_records(paths, ProviderKind::Xai).await
}

pub async fn delete_account(paths: &TokenProxyPaths, account_id: &str) -> Result<(), String> {
    records::delete_record(paths, ProviderKind::Xai, account_id).await
}

/// 单事务替换该 provider 全部记录（编排层 restore_all）。
pub async fn replace_all_xai_records(
    paths: &TokenProxyPaths,
    snapshot: &HashMap<String, XaiTokenRecord>,
) -> Result<(), String> {
    let mut rows = Vec::with_capacity(snapshot.len());
    for (account_id, record) in snapshot {
        let record_json = serde_json::to_string(record)
            .map_err(|error| format!("Failed to serialize xai account {account_id}: {error}"))?;
        rows.push(records::ProviderSnapshotRow {
            account_id: account_id.to_string(),
            email: record.email.clone(),
            expires_at: Some(record.expires_at.clone()),
            expires_at_ms: record.expires_at().map(records::unix_millis),
            auth_method: Some("oauth".to_string()),
            provider_name: Some("xai".to_string()),
            record_json,
        });
    }
    records::replace_provider_snapshot(paths, ProviderKind::Xai, &rows).await
}

/// 单账户原子 restore：`None` 删除。
pub async fn replace_xai_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: Option<&XaiTokenRecord>,
) -> Result<(), String> {
    let metadata = record.map(|r| AccountRecordMetadata {
        account_id,
        email: r.email.as_deref(),
        expires_at: Some(r.expires_at.as_str()),
        expires_at_ms: r.expires_at().map(records::unix_millis),
        auth_method: Some("oauth"),
        provider_name: Some("xai"),
    });
    records::replace_single_record(paths, ProviderKind::Xai, account_id, record, metadata).await
}
