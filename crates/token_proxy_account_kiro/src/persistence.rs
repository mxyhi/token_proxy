//! Kiro mapping onto provider-independent account records.

use std::collections::HashMap;

use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_account_store::records::{
    self, AccountRecordMetadata, ProviderKind, ProviderSnapshotRow,
};

use crate::types::KiroTokenRecord;

pub async fn upsert_kiro_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: &KiroTokenRecord,
) -> Result<(), String> {
    records::upsert_record(
        paths,
        ProviderKind::Kiro,
        kiro_metadata(account_id, record),
        record,
    )
    .await
}

pub async fn list_kiro_records(
    paths: &TokenProxyPaths,
) -> Result<HashMap<String, KiroTokenRecord>, String> {
    records::list_records(paths, ProviderKind::Kiro).await
}

pub async fn delete_account(paths: &TokenProxyPaths, account_id: &str) -> Result<(), String> {
    records::delete_record(paths, ProviderKind::Kiro, account_id).await
}

/// 单事务替换该 provider 全部记录（编排层 restore_all）。
pub async fn replace_all_kiro_records(
    paths: &TokenProxyPaths,
    snapshot: &HashMap<String, KiroTokenRecord>,
) -> Result<(), String> {
    let mut rows = Vec::with_capacity(snapshot.len());
    for (account_id, record) in snapshot {
        rows.push(snapshot_row(account_id, record)?);
    }
    records::replace_provider_snapshot(paths, ProviderKind::Kiro, &rows).await
}

/// 单账户原子 restore：`None` 删除。
pub async fn replace_kiro_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: Option<&KiroTokenRecord>,
) -> Result<(), String> {
    let metadata = record.map(|r| kiro_metadata(account_id, r));
    records::replace_single_record(paths, ProviderKind::Kiro, account_id, record, metadata).await
}

fn kiro_metadata<'a>(
    account_id: &'a str,
    record: &'a KiroTokenRecord,
) -> AccountRecordMetadata<'a> {
    AccountRecordMetadata {
        account_id,
        email: record.email.as_deref(),
        expires_at: Some(record.expires_at.as_str()),
        expires_at_ms: record.expires_at().map(records::unix_millis),
        auth_method: Some(record.auth_method.as_str()),
        provider_name: Some(record.provider.as_str()),
    }
}

fn snapshot_row(account_id: &str, record: &KiroTokenRecord) -> Result<ProviderSnapshotRow, String> {
    let record_json = serde_json::to_string(record)
        .map_err(|error| format!("Failed to serialize kiro account {account_id}: {error}"))?;
    Ok(ProviderSnapshotRow {
        account_id: account_id.to_string(),
        email: record.email.clone(),
        expires_at: Some(record.expires_at.clone()),
        expires_at_ms: record.expires_at().map(records::unix_millis),
        auth_method: Some(record.auth_method.clone()),
        provider_name: Some(record.provider.clone()),
        record_json,
    })
}
