use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;

use crate::codex::{CodexQuotaCache, CodexQuotaItem, CodexTokenRecord};
use crate::kiro::{KiroQuotaCache, KiroQuotaItem, KiroTokenRecord};
use crate::paths::TokenProxyPaths;
use crate::proxy::sqlite;

const PROVIDER_KIND_KIRO: &str = "kiro";
const PROVIDER_KIND_CODEX: &str = "codex";
const STATUS_ACTIVE: &str = "active";
const STATUS_DISABLED: &str = "disabled";
const STATUS_EXPIRED: &str = "expired";
const STATUS_COOLING_DOWN: &str = "cooling_down";

pub const MAX_PAGE_SIZE: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccountKind {
    Kiro,
    Codex,
}

impl ProviderAccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kiro => PROVIDER_KIND_KIRO,
            Self::Codex => PROVIDER_KIND_CODEX,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            PROVIDER_KIND_KIRO => Ok(Self::Kiro),
            PROVIDER_KIND_CODEX => Ok(Self::Codex),
            other => Err(format!("Unsupported provider filter: {other}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccountStatus {
    Active,
    Disabled,
    Expired,
    CoolingDown,
}

impl ProviderAccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => STATUS_ACTIVE,
            Self::Disabled => STATUS_DISABLED,
            Self::Expired => STATUS_EXPIRED,
            Self::CoolingDown => STATUS_COOLING_DOWN,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            STATUS_ACTIVE => Ok(Self::Active),
            STATUS_DISABLED => Ok(Self::Disabled),
            STATUS_EXPIRED => Ok(Self::Expired),
            STATUS_COOLING_DOWN => Ok(Self::CoolingDown),
            other => Err(format!("Unsupported status filter: {other}")),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountListItem {
    pub provider_kind: ProviderAccountKind,
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: ProviderAccountStatus,
    pub auth_method: Option<String>,
    pub provider_name: Option<String>,
    pub auto_refresh_enabled: Option<bool>,
    pub proxy_url: Option<String>,
    pub quota: ProviderAccountQuotaSnapshot,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProviderAccountQuotaSnapshot {
    pub plan_type: Option<String>,
    pub error: Option<String>,
    pub checked_at: Option<String>,
    pub items: Vec<ProviderAccountQuotaItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
    pub is_trial: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountsPage {
    pub items: Vec<ProviderAccountListItem>,
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Clone, Debug)]
pub struct ProviderAccountsQueryParams {
    pub provider_kind: Option<ProviderAccountKind>,
    pub search: String,
}

#[derive(Clone)]
struct ProviderAccountDbRecord {
    provider_kind: ProviderAccountKind,
    account_id: String,
    email: Option<String>,
    expires_at: Option<String>,
    expires_at_ms: Option<i64>,
    auth_method: Option<String>,
    provider_name: Option<String>,
    record_json: String,
    updated_at_ms: i64,
}

pub async fn upsert_kiro_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: &KiroTokenRecord,
) -> Result<(), String> {
    let db_record = build_kiro_db_record(account_id, record)?;
    upsert_account(paths, &db_record).await
}

pub async fn upsert_codex_account(
    paths: &TokenProxyPaths,
    account_id: &str,
    record: &CodexTokenRecord,
) -> Result<(), String> {
    let db_record = build_codex_db_record(account_id, record)?;
    upsert_account(paths, &db_record).await
}

pub async fn delete_account(paths: &TokenProxyPaths, account_id: &str) -> Result<(), String> {
    let pool = sqlite::open_write_pool(paths).await?;
    sqlx::query("DELETE FROM provider_accounts WHERE account_id = ?;")
        .bind(account_id)
        .execute(&pool)
        .await
        .map_err(|err| format!("Failed to delete provider account row: {err}"))?;
    Ok(())
}

pub async fn delete_accounts(paths: &TokenProxyPaths, account_ids: &[String]) -> Result<(), String> {
    if account_ids.is_empty() {
        return Ok(());
    }
    let pool = sqlite::open_write_pool(paths).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| format!("Failed to begin delete transaction: {err}"))?;
    for account_id in account_ids {
        sqlx::query("DELETE FROM provider_accounts WHERE account_id = ?;")
            .bind(account_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|err| format!("Failed to delete provider account row {}: {err}", account_id))?;
    }
    tx.commit()
        .await
        .map_err(|err| format!("Failed to commit delete transaction: {err}"))?;
    Ok(())
}

pub async fn list_accounts_snapshot(
    paths: &TokenProxyPaths,
    params: ProviderAccountsQueryParams,
) -> Result<Vec<ProviderAccountListItem>, String> {
    let provider_filter = params
        .provider_kind
        .map(ProviderAccountKind::as_str)
        .unwrap_or("");
    let search = params.search.trim().to_ascii_lowercase();
    let search_pattern = if search.is_empty() {
        String::new()
    } else {
        format!("%{search}%")
    };
    let pool = sqlite::open_read_pool(paths).await?;

    let rows = sqlx::query(
        r#"
SELECT
  provider_kind,
  account_id,
  email,
  expires_at,
  expires_at_ms,
  auth_method,
  record_json,
  provider_name
FROM provider_accounts
WHERE (?1 = '' OR provider_kind = ?1)
  AND (?2 = '' OR lower(account_id) LIKE ?3 OR lower(COALESCE(email, '')) LIKE ?3)
ORDER BY provider_kind ASC, account_id ASC
"#,
    )
    .bind(provider_filter)
    .bind(search.as_str())
    .bind(search_pattern.as_str())
    .fetch_all(&pool)
    .await
    .map_err(|err| format!("Failed to read provider account rows: {err}"))?;

    rows
        .into_iter()
        .map(|row| {
            let provider_kind = ProviderAccountKind::parse(
                row.try_get::<String, _>("provider_kind")
                    .map_err(|err| format!("Failed to decode provider_kind: {err}"))?
                    .as_str(),
            )?;
            let record_json = row
                .try_get::<String, _>("record_json")
                .map_err(|err| format!("Failed to decode record_json: {err}"))?;
            let account_id = row
                .try_get("account_id")
                .map_err(|err| format!("Failed to decode account_id: {err}"))?;
            match provider_kind {
                ProviderAccountKind::Kiro => build_kiro_list_item(
                    account_id,
                    row.try_get("email")
                        .map_err(|err| format!("Failed to decode email: {err}"))?,
                    row.try_get("expires_at")
                        .map_err(|err| format!("Failed to decode expires_at: {err}"))?,
                    row.try_get("auth_method")
                        .map_err(|err| format!("Failed to decode auth_method: {err}"))?,
                    row.try_get("provider_name")
                        .map_err(|err| format!("Failed to decode provider_name: {err}"))?,
                    &record_json,
                ),
                ProviderAccountKind::Codex => build_codex_list_item(
                    account_id,
                    row.try_get("email")
                        .map_err(|err| format!("Failed to decode email: {err}"))?,
                    row.try_get("expires_at")
                        .map_err(|err| format!("Failed to decode expires_at: {err}"))?,
                    row.try_get("provider_name")
                        .map_err(|err| format!("Failed to decode provider_name: {err}"))?,
                    &record_json,
                ),
            }
        })
        .collect::<Result<Vec<_>, String>>()
}

pub async fn list_kiro_records(
    paths: &TokenProxyPaths,
) -> Result<HashMap<String, KiroTokenRecord>, String> {
    list_records_by_kind(paths, ProviderAccountKind::Kiro).await
}

pub async fn list_codex_records(
    paths: &TokenProxyPaths,
) -> Result<HashMap<String, CodexTokenRecord>, String> {
    list_records_by_kind(paths, ProviderAccountKind::Codex).await
}

fn build_kiro_db_record(
    account_id: &str,
    record: &KiroTokenRecord,
) -> Result<ProviderAccountDbRecord, String> {
    Ok(ProviderAccountDbRecord {
        provider_kind: ProviderAccountKind::Kiro,
        account_id: account_id.to_string(),
        email: normalize_optional_string(record.email.as_deref()),
        expires_at: normalize_optional_string(Some(record.expires_at.as_str())),
        expires_at_ms: record.expires_at().map(offset_datetime_to_unix_ms),
        auth_method: normalize_optional_string(Some(record.auth_method.as_str())),
        provider_name: normalize_optional_string(Some(record.provider.as_str())),
        record_json: serde_json::to_string(record)
            .map_err(|err| format!("Failed to serialize Kiro token record for sqlite: {err}"))?,
        updated_at_ms: now_unix_ms(),
    })
}

fn build_codex_db_record(
    account_id: &str,
    record: &CodexTokenRecord,
) -> Result<ProviderAccountDbRecord, String> {
    Ok(ProviderAccountDbRecord {
        provider_kind: ProviderAccountKind::Codex,
        account_id: account_id.to_string(),
        email: normalize_optional_string(record.email.as_deref()),
        expires_at: normalize_optional_string(Some(record.expires_at.as_str())),
        expires_at_ms: record
            .expires_at()
            .map(offset_datetime_to_unix_ms),
        auth_method: None,
        provider_name: None,
        record_json: serde_json::to_string(record)
            .map_err(|err| format!("Failed to serialize Codex token record for sqlite: {err}"))?,
        updated_at_ms: now_unix_ms(),
    })
}

async fn upsert_account(paths: &TokenProxyPaths, record: &ProviderAccountDbRecord) -> Result<(), String> {
    let pool = sqlite::open_write_pool(paths).await?;
    execute_upsert_pool(&pool, record).await
}

async fn execute_upsert_pool(
    pool: &sqlx::SqlitePool,
    record: &ProviderAccountDbRecord,
) -> Result<(), String> {
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
  updated_at_ms
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(account_id) DO UPDATE SET
  provider_kind = excluded.provider_kind,
  email = excluded.email,
  expires_at = excluded.expires_at,
  expires_at_ms = excluded.expires_at_ms,
  auth_method = excluded.auth_method,
  provider_name = excluded.provider_name,
  record_json = excluded.record_json,
  updated_at_ms = excluded.updated_at_ms;
"#,
    )
    .bind(record.provider_kind.as_str())
    .bind(record.account_id.as_str())
    .bind(record.email.as_deref())
    .bind(record.expires_at.as_deref())
    .bind(record.expires_at_ms)
    .bind(record.auth_method.as_deref())
    .bind(record.provider_name.as_deref())
    .bind(record.record_json.as_str())
    .bind(record.updated_at_ms)
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to upsert provider account row: {err}"))?;
    Ok(())
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn build_kiro_list_item(
    account_id: String,
    email: Option<String>,
    expires_at: Option<String>,
    auth_method: Option<String>,
    provider_name: Option<String>,
    record_json: &str,
) -> Result<ProviderAccountListItem, String> {
    let record = serde_json::from_str::<KiroTokenRecord>(record_json)
        .map_err(|err| format!("Failed to parse kiro record_json: {err}"))?;
    Ok(ProviderAccountListItem {
        provider_kind: ProviderAccountKind::Kiro,
        account_id,
        email,
        expires_at,
        status: provider_status_from_kiro(&record),
        auth_method,
        provider_name,
        auto_refresh_enabled: None,
        proxy_url: normalize_optional_string(record.proxy_url.as_deref()),
        quota: provider_quota_snapshot_from_kiro(&record.quota),
    })
}

fn build_codex_list_item(
    account_id: String,
    email: Option<String>,
    expires_at: Option<String>,
    provider_name: Option<String>,
    record_json: &str,
) -> Result<ProviderAccountListItem, String> {
    let record = serde_json::from_str::<CodexTokenRecord>(record_json)
        .map_err(|err| format!("Failed to parse codex record_json: {err}"))?;
    Ok(ProviderAccountListItem {
        provider_kind: ProviderAccountKind::Codex,
        account_id,
        email,
        expires_at,
        status: provider_status_from_codex(&record),
        auth_method: None,
        provider_name,
        auto_refresh_enabled: Some(record.auto_refresh_enabled),
        proxy_url: normalize_optional_string(record.proxy_url.as_deref()),
        quota: provider_quota_snapshot_from_codex(&record.quota),
    })
}

fn provider_status_from_kiro(record: &KiroTokenRecord) -> ProviderAccountStatus {
    match record.effective_status() {
        crate::kiro::KiroAccountStatus::Active => ProviderAccountStatus::Active,
        crate::kiro::KiroAccountStatus::Disabled => ProviderAccountStatus::Disabled,
        crate::kiro::KiroAccountStatus::Expired => ProviderAccountStatus::Expired,
    }
}

fn provider_status_from_codex(record: &CodexTokenRecord) -> ProviderAccountStatus {
    match record.effective_status() {
        crate::codex::CodexAccountStatus::Active => ProviderAccountStatus::Active,
        crate::codex::CodexAccountStatus::Disabled => ProviderAccountStatus::Disabled,
        crate::codex::CodexAccountStatus::Expired => ProviderAccountStatus::Expired,
    }
}

fn provider_quota_snapshot_from_kiro(quota: &KiroQuotaCache) -> ProviderAccountQuotaSnapshot {
    ProviderAccountQuotaSnapshot {
        plan_type: quota.plan_type.clone(),
        error: quota.error.clone(),
        checked_at: quota.checked_at.clone(),
        items: quota
            .quotas
            .iter()
            .map(provider_quota_item_from_kiro)
            .collect(),
    }
}

fn provider_quota_snapshot_from_codex(quota: &CodexQuotaCache) -> ProviderAccountQuotaSnapshot {
    ProviderAccountQuotaSnapshot {
        plan_type: quota.plan_type.clone(),
        error: quota.error.clone(),
        checked_at: quota.checked_at.clone(),
        items: quota
            .quotas
            .iter()
            .map(provider_quota_item_from_codex)
            .collect(),
    }
}

fn provider_quota_item_from_kiro(item: &KiroQuotaItem) -> ProviderAccountQuotaItem {
    ProviderAccountQuotaItem {
        name: item.name.clone(),
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        reset_at: item.reset_at.clone(),
        is_trial: item.is_trial,
    }
}

fn provider_quota_item_from_codex(item: &CodexQuotaItem) -> ProviderAccountQuotaItem {
    ProviderAccountQuotaItem {
        name: item.name.clone(),
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        reset_at: item.reset_at.clone(),
        is_trial: false,
    }
}

fn now_unix_ms() -> i64 {
    offset_datetime_to_unix_ms(OffsetDateTime::now_utc())
}

fn offset_datetime_to_unix_ms(value: OffsetDateTime) -> i64 {
    let nanos = value.unix_timestamp_nanos();
    let millis = nanos / 1_000_000;
    i64::try_from(millis).unwrap_or_else(|_| {
        if millis.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

async fn list_records_by_kind<T>(
    paths: &TokenProxyPaths,
    provider_kind: ProviderAccountKind,
) -> Result<HashMap<String, T>, String>
where
    T: DeserializeOwned,
{
    let pool = sqlite::open_read_pool(paths).await?;
    let rows = sqlx::query(
        r#"
SELECT account_id, record_json
FROM provider_accounts
WHERE provider_kind = ?
ORDER BY account_id ASC;
"#,
    )
    .bind(provider_kind.as_str())
    .fetch_all(&pool)
    .await
    .map_err(|err| format!("Failed to read provider account records: {err}"))?;

    let mut snapshot = HashMap::with_capacity(rows.len());
    for row in rows {
        let account_id = row
            .try_get::<String, _>("account_id")
            .map_err(|err| format!("Failed to decode provider account_id: {err}"))?;
        let record_json = row
            .try_get::<String, _>("record_json")
            .map_err(|err| format!("Failed to decode provider record_json: {err}"))?;
        let record = serde_json::from_str::<T>(&record_json).map_err(|err| {
            format!(
                "Failed to deserialize provider record_json for {} account {}: {err}",
                provider_kind.as_str(),
                account_id
            )
        })?;
        snapshot.insert(account_id, record);
    }
    Ok(snapshot)
}
