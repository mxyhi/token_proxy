use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;

use crate::codex::CodexTokenRecord;
use crate::kiro::KiroTokenRecord;
use crate::paths::TokenProxyPaths;
use crate::proxy::sqlite;

const PROVIDER_KIND_KIRO: &str = "kiro";
const PROVIDER_KIND_CODEX: &str = "codex";
const STATUS_ACTIVE: &str = "active";
const STATUS_EXPIRED: &str = "expired";
const MAX_PAGE_SIZE: u32 = 100;

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
    Expired,
}

impl ProviderAccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => STATUS_ACTIVE,
            Self::Expired => STATUS_EXPIRED,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            STATUS_ACTIVE => Ok(Self::Active),
            STATUS_EXPIRED => Ok(Self::Expired),
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
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountsPage {
    pub items: Vec<ProviderAccountListItem>,
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Clone, Debug)]
pub struct ProviderAccountsPageParams {
    pub page: u32,
    pub page_size: u32,
    pub provider_kind: Option<ProviderAccountKind>,
    pub status: Option<ProviderAccountStatus>,
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

pub async fn list_accounts_page(
    paths: &TokenProxyPaths,
    params: ProviderAccountsPageParams,
) -> Result<ProviderAccountsPage, String> {
    let page = params.page.max(1);
    let page_size = params.page_size.clamp(1, MAX_PAGE_SIZE);
    let provider_filter = params
        .provider_kind
        .map(ProviderAccountKind::as_str)
        .unwrap_or("");
    let status_filter = params
        .status
        .map(ProviderAccountStatus::as_str)
        .unwrap_or("");
    let search = params.search.trim().to_ascii_lowercase();
    let search_pattern = if search.is_empty() {
        String::new()
    } else {
        format!("%{search}%")
    };
    let now_ms = now_unix_ms();
    let offset = i64::from((page - 1) * page_size);
    let pool = sqlite::open_read_pool(paths).await?;

    let total_row = sqlx::query(
        r#"
SELECT COUNT(*) AS total
FROM provider_accounts
WHERE (?1 = '' OR provider_kind = ?1)
  AND (?2 = '' OR (
        CASE WHEN expires_at_ms IS NULL OR expires_at_ms <= ?3 THEN 'expired' ELSE 'active' END
      ) = ?2)
  AND (?4 = '' OR lower(account_id) LIKE ?5 OR lower(COALESCE(email, '')) LIKE ?5);
"#,
    )
    .bind(provider_filter)
    .bind(status_filter)
    .bind(now_ms)
    .bind(search.as_str())
    .bind(search_pattern.as_str())
    .fetch_one(&pool)
    .await
    .map_err(|err| format!("Failed to count provider account rows: {err}"))?;
    let total_i64 = total_row
        .try_get::<i64, _>("total")
        .map_err(|err| format!("Failed to decode provider account count: {err}"))?;
    let total = u32::try_from(total_i64).unwrap_or(u32::MAX);

    let rows = sqlx::query(
        r#"
SELECT
  provider_kind,
  account_id,
  email,
  expires_at,
  auth_method,
  record_json,
  provider_name,
  CASE WHEN expires_at_ms IS NULL OR expires_at_ms <= ?1 THEN 'expired' ELSE 'active' END AS status
FROM provider_accounts
WHERE (?2 = '' OR provider_kind = ?2)
  AND (?3 = '' OR (
        CASE WHEN expires_at_ms IS NULL OR expires_at_ms <= ?1 THEN 'expired' ELSE 'active' END
      ) = ?3)
  AND (?4 = '' OR lower(account_id) LIKE ?5 OR lower(COALESCE(email, '')) LIKE ?5)
ORDER BY provider_kind ASC, account_id ASC
LIMIT ?6 OFFSET ?7;
"#,
    )
    .bind(now_ms)
    .bind(provider_filter)
    .bind(status_filter)
    .bind(search.as_str())
    .bind(search_pattern.as_str())
    .bind(i64::from(page_size))
    .bind(offset)
    .fetch_all(&pool)
    .await
    .map_err(|err| format!("Failed to read provider account rows: {err}"))?;

    let items = rows
        .into_iter()
        .map(|row| {
            let provider_kind = ProviderAccountKind::parse(
                row.try_get::<String, _>("provider_kind")
                    .map_err(|err| format!("Failed to decode provider_kind: {err}"))?
                    .as_str(),
            )?;
            let status = ProviderAccountStatus::parse(
                row.try_get::<String, _>("status")
                    .map_err(|err| format!("Failed to decode provider status: {err}"))?
                    .as_str(),
            )?;
            Ok(ProviderAccountListItem {
                provider_kind,
                account_id: row
                    .try_get("account_id")
                    .map_err(|err| format!("Failed to decode account_id: {err}"))?,
                email: row
                    .try_get("email")
                    .map_err(|err| format!("Failed to decode email: {err}"))?,
                expires_at: row
                    .try_get("expires_at")
                    .map_err(|err| format!("Failed to decode expires_at: {err}"))?,
                status,
                auth_method: row
                    .try_get("auth_method")
                    .map_err(|err| format!("Failed to decode auth_method: {err}"))?,
                auto_refresh_enabled: decode_auto_refresh_enabled(
                    provider_kind,
                    row.try_get::<String, _>("record_json")
                        .map_err(|err| format!("Failed to decode record_json: {err}"))?
                        .as_str(),
                )?,
                proxy_url: decode_proxy_url(
                    row.try_get::<String, _>("record_json")
                        .map_err(|err| format!("Failed to decode record_json: {err}"))?
                        .as_str(),
                )?,
                provider_name: row
                    .try_get("provider_name")
                    .map_err(|err| format!("Failed to decode provider_name: {err}"))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(ProviderAccountsPage {
        items,
        total,
        page,
        page_size,
    })
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

fn decode_auto_refresh_enabled(
    provider_kind: ProviderAccountKind,
    record_json: &str,
) -> Result<Option<bool>, String> {
    if provider_kind != ProviderAccountKind::Codex {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_str(record_json)
        .map_err(|err| format!("Failed to parse codex record_json: {err}"))?;
    Ok(Some(
        value
            .get("auto_refresh_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    ))
}

fn decode_proxy_url(record_json: &str) -> Result<Option<String>, String> {
    let value: serde_json::Value = serde_json::from_str(record_json)
        .map_err(|err| format!("Failed to parse provider record_json: {err}"))?;
    Ok(value
        .get("proxy_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string))
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
