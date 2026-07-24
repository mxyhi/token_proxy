//! Generic durable records shared by account providers.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;

use crate::database;
use crate::paths::TokenProxyPaths;

/// Stable provider discriminator stored beside each opaque provider record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Kiro,
    Codex,
    Xai,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kiro => "kiro",
            Self::Codex => "codex",
            Self::Xai => "xai",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "kiro" => Ok(Self::Kiro),
            "codex" => Ok(Self::Codex),
            "xai" => Ok(Self::Xai),
            other => Err(format!("Unsupported provider filter: {other}")),
        }
    }
}

/// Searchable metadata stored next to provider-owned JSON.
///
/// Routing fields (`priority` / enabled / proxy) live on Upstream, not here.
pub struct AccountRecordMetadata<'a> {
    pub account_id: &'a str,
    pub email: Option<&'a str>,
    pub expires_at: Option<&'a str>,
    pub expires_at_ms: Option<i64>,
    pub auth_method: Option<&'a str>,
    pub provider_name: Option<&'a str>,
}

/// Raw account projection consumed by the cross-provider facade.
#[derive(Clone, Debug)]
pub struct StoredAccountRow {
    pub provider_kind: ProviderKind,
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub auth_method: Option<String>,
    pub provider_name: Option<String>,
    pub record_json: String,
}

/// Persists opaque provider JSON with shared searchable metadata.
pub async fn upsert_record<T>(
    paths: &TokenProxyPaths,
    provider_kind: ProviderKind,
    metadata: AccountRecordMetadata<'_>,
    record: &T,
) -> Result<(), String>
where
    T: Serialize,
{
    let record_json = serde_json::to_string(record).map_err(|error| {
        format!(
            "Failed to serialize {} account {}: {error}",
            provider_kind.as_str(),
            metadata.account_id
        )
    })?;
    let pool = database::open_write_pool(paths).await?;
    sqlx::query(
        r#"
INSERT INTO provider_accounts (
  provider_kind, account_id, email, expires_at, expires_at_ms,
  auth_method, provider_name, record_json, updated_at_ms
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
    .bind(provider_kind.as_str())
    .bind(metadata.account_id)
    .bind(normalize_optional_string(metadata.email))
    .bind(normalize_optional_string(metadata.expires_at))
    .bind(metadata.expires_at_ms)
    .bind(normalize_optional_string(metadata.auth_method))
    .bind(normalize_optional_string(metadata.provider_name))
    .bind(record_json)
    .bind(unix_millis(OffsetDateTime::now_utc()))
    .execute(&pool)
    .await
    .map_err(|error| format!("Failed to upsert provider account row: {error}"))?;
    tracing::debug!(
        provider = provider_kind.as_str(),
        account_id = metadata.account_id,
        "provider account persisted"
    );
    Ok(())
}

/// Loads and deserializes all records owned by one provider.
pub async fn list_records<T>(
    paths: &TokenProxyPaths,
    provider_kind: ProviderKind,
) -> Result<HashMap<String, T>, String>
where
    T: DeserializeOwned,
{
    let pool = database::open_read_pool(paths).await?;
    let rows = sqlx::query(
        "SELECT account_id, record_json FROM provider_accounts WHERE provider_kind = ? ORDER BY account_id ASC;",
    )
    .bind(provider_kind.as_str())
    .fetch_all(&pool)
    .await
    .map_err(|error| format!("Failed to read provider account records: {error}"))?;

    let mut snapshot = HashMap::with_capacity(rows.len());
    for row in rows {
        let account_id = row
            .try_get::<String, _>("account_id")
            .map_err(|error| format!("Failed to decode provider account_id: {error}"))?;
        let record_json = row
            .try_get::<String, _>("record_json")
            .map_err(|error| format!("Failed to decode provider record_json: {error}"))?;
        let record = serde_json::from_str::<T>(&record_json).map_err(|error| {
            format!(
                "Failed to deserialize {} account {}: {error}",
                provider_kind.as_str(),
                account_id
            )
        })?;
        snapshot.insert(account_id, record);
    }
    Ok(snapshot)
}

/// Reads raw rows for cross-provider administration.
pub async fn list_rows(
    paths: &TokenProxyPaths,
    provider_filter: Option<ProviderKind>,
    search: &str,
) -> Result<Vec<StoredAccountRow>, String> {
    let provider_filter = provider_filter.map(ProviderKind::as_str).unwrap_or("");
    let search = search.trim().to_ascii_lowercase();
    let search_pattern = if search.is_empty() {
        String::new()
    } else {
        format!("%{search}%")
    };
    let pool = database::open_read_pool(paths).await?;
    let rows = sqlx::query(
        r#"
SELECT provider_kind, account_id, email, expires_at, auth_method, provider_name, record_json
FROM provider_accounts
WHERE (?1 = '' OR provider_kind = ?1)
  AND (?2 = '' OR lower(account_id) LIKE ?3 OR lower(COALESCE(email, '')) LIKE ?3)
ORDER BY account_id ASC
"#,
    )
    .bind(provider_filter)
    .bind(search.as_str())
    .bind(search_pattern.as_str())
    .fetch_all(&pool)
    .await
    .map_err(|error| format!("Failed to read provider account rows: {error}"))?;

    rows.into_iter()
        .map(|row| {
            Ok(StoredAccountRow {
                provider_kind: ProviderKind::parse(
                    row.try_get::<String, _>("provider_kind")
                        .map_err(|error| format!("Failed to decode provider_kind: {error}"))?
                        .as_str(),
                )?,
                account_id: row
                    .try_get("account_id")
                    .map_err(|error| format!("Failed to decode account_id: {error}"))?,
                email: row
                    .try_get("email")
                    .map_err(|error| format!("Failed to decode email: {error}"))?,
                expires_at: row
                    .try_get("expires_at")
                    .map_err(|error| format!("Failed to decode expires_at: {error}"))?,
                auth_method: row
                    .try_get("auth_method")
                    .map_err(|error| format!("Failed to decode auth_method: {error}"))?,
                provider_name: row
                    .try_get("provider_name")
                    .map_err(|error| format!("Failed to decode provider_name: {error}"))?,
                record_json: row
                    .try_get("record_json")
                    .map_err(|error| format!("Failed to decode record_json: {error}"))?,
            })
        })
        .collect()
}

/// Deletes one record only when both provider and account ID match.
pub async fn delete_record(
    paths: &TokenProxyPaths,
    provider_kind: ProviderKind,
    account_id: &str,
) -> Result<(), String> {
    let pool = database::open_write_pool(paths).await?;
    let result =
        sqlx::query("DELETE FROM provider_accounts WHERE provider_kind = ? AND account_id = ?;")
            .bind(provider_kind.as_str())
            .bind(account_id)
            .execute(&pool)
            .await
            .map_err(|error| format!("Failed to delete provider account row: {error}"))?;
    if result.rows_affected() == 0 {
        return Err(format!(
            "{} account not found: {account_id}",
            provider_kind.as_str()
        ));
    }
    tracing::debug!(
        provider = provider_kind.as_str(),
        account_id,
        "provider account deleted"
    );
    Ok(())
}

/// Deletes multiple cross-provider rows in one transaction.
pub async fn delete_rows(paths: &TokenProxyPaths, account_ids: &[String]) -> Result<(), String> {
    if account_ids.is_empty() {
        return Ok(());
    }
    let pool = database::open_write_pool(paths).await?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Failed to begin delete transaction: {error}"))?;
    for account_id in account_ids {
        sqlx::query("DELETE FROM provider_accounts WHERE account_id = ?;")
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                format!("Failed to delete provider account row {account_id}: {error}")
            })?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| format!("Failed to commit delete transaction: {error}"))?;
    tracing::debug!(count = account_ids.len(), "provider accounts deleted");
    Ok(())
}

/// 单 provider 全量快照替换（单 SQLite 事务）：删掉该 provider 全部行再写入 snapshot。
/// 提交失败时旧数据保持；调用方应在事务提交成功后再替换内存 cache。
/// 禁止复制 sqlite 文件；不跨 provider 写入。
pub async fn replace_provider_snapshot(
    paths: &TokenProxyPaths,
    provider_kind: ProviderKind,
    rows: &[ProviderSnapshotRow],
) -> Result<(), String> {
    let pool = database::open_write_pool(paths).await?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Failed to begin replace snapshot transaction: {error}"))?;

    sqlx::query("DELETE FROM provider_accounts WHERE provider_kind = ?;")
        .bind(provider_kind.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            format!(
                "Failed to clear {} accounts for snapshot replace: {error}",
                provider_kind.as_str()
            )
        })?;

    let updated_at_ms = unix_millis(OffsetDateTime::now_utc());
    for row in rows {
        sqlx::query(
            r#"
INSERT INTO provider_accounts (
  provider_kind, account_id, email, expires_at, expires_at_ms,
  auth_method, provider_name, record_json, updated_at_ms
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
"#,
        )
        .bind(provider_kind.as_str())
        .bind(row.account_id.as_str())
        .bind(normalize_optional_string(row.email.as_deref()))
        .bind(normalize_optional_string(row.expires_at.as_deref()))
        .bind(row.expires_at_ms)
        .bind(normalize_optional_string(row.auth_method.as_deref()))
        .bind(normalize_optional_string(row.provider_name.as_deref()))
        .bind(row.record_json.as_str())
        .bind(updated_at_ms)
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            format!(
                "Failed to insert {} account {} during snapshot replace: {error}",
                provider_kind.as_str(),
                row.account_id
            )
        })?;
    }

    transaction
        .commit()
        .await
        .map_err(|error| format!("Failed to commit replace snapshot transaction: {error}"))?;
    tracing::debug!(
        provider = provider_kind.as_str(),
        count = rows.len(),
        "provider account snapshot replaced in single transaction"
    );
    Ok(())
}

/// 单账户在单事务内替换：`None` 删除，`Some` upsert。保证 DB 原子。
pub async fn replace_single_record<T>(
    paths: &TokenProxyPaths,
    provider_kind: ProviderKind,
    account_id: &str,
    record: Option<&T>,
    metadata: Option<AccountRecordMetadata<'_>>,
) -> Result<(), String>
where
    T: Serialize,
{
    let pool = database::open_write_pool(paths).await?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Failed to begin single-record replace transaction: {error}"))?;

    match (record, metadata) {
        (None, _) => {
            sqlx::query(
                "DELETE FROM provider_accounts WHERE provider_kind = ? AND account_id = ?;",
            )
            .bind(provider_kind.as_str())
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                format!(
                    "Failed to delete {} account {account_id} during restore: {error}",
                    provider_kind.as_str()
                )
            })?;
            // 删除幂等：0 行也视为成功（新登录补偿时可能已不存在）。
        }
        (Some(record), Some(metadata)) => {
            let record_json = serde_json::to_string(record).map_err(|error| {
                format!(
                    "Failed to serialize {} account {}: {error}",
                    provider_kind.as_str(),
                    metadata.account_id
                )
            })?;
            sqlx::query(
                r#"
INSERT INTO provider_accounts (
  provider_kind, account_id, email, expires_at, expires_at_ms,
  auth_method, provider_name, record_json, updated_at_ms
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
            .bind(provider_kind.as_str())
            .bind(metadata.account_id)
            .bind(normalize_optional_string(metadata.email))
            .bind(normalize_optional_string(metadata.expires_at))
            .bind(metadata.expires_at_ms)
            .bind(normalize_optional_string(metadata.auth_method))
            .bind(normalize_optional_string(metadata.provider_name))
            .bind(record_json)
            .bind(unix_millis(OffsetDateTime::now_utc()))
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                format!(
                    "Failed to upsert {} account {account_id} during restore: {error}",
                    provider_kind.as_str()
                )
            })?;
        }
        (Some(_), None) => {
            return Err(format!(
                "Missing metadata for {} account {account_id} restore",
                provider_kind.as_str()
            ));
        }
    }

    transaction
        .commit()
        .await
        .map_err(|error| format!("Failed to commit single-record replace: {error}"))?;
    tracing::debug!(
        provider = provider_kind.as_str(),
        account_id,
        restored = record.is_some(),
        "single account record replaced atomically"
    );
    Ok(())
}

/// 全量快照替换的一行（序列化后的 record_json + 检索元数据）。
#[derive(Clone, Debug)]
pub struct ProviderSnapshotRow {
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub auth_method: Option<String>,
    pub provider_name: Option<String>,
    pub record_json: String,
}

pub fn unix_millis(value: OffsetDateTime) -> i64 {
    let millis = value.unix_timestamp_nanos() / 1_000_000;
    i64::try_from(millis).unwrap_or_else(|_| {
        if millis.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use sqlx::Row;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct SampleRecord {
        status: String,
        access_token: String,
    }

    #[tokio::test]
    async fn upsert_list_delete_without_priority_column() {
        let data_dir = std::env::temp_dir().join(format!(
            "token-proxy-account-store-records-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("paths");
        let record = SampleRecord {
            status: "active".to_string(),
            access_token: "token-1".to_string(),
        };

        upsert_record(
            &paths,
            ProviderKind::Codex,
            AccountRecordMetadata {
                account_id: "acct-1.json",
                email: Some("user@example.com"),
                expires_at: Some("2099-01-01T00:00:00Z"),
                expires_at_ms: Some(4_102_444_800_000),
                auth_method: Some("oauth"),
                provider_name: None,
            },
            &record,
        )
        .await
        .expect("upsert");

        let pool = database::open_read_pool(&paths).await.expect("pool");
        let columns = sqlx::query("PRAGMA table_info(provider_accounts);")
            .fetch_all(&pool)
            .await
            .expect("pragma")
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("name").ok())
            .collect::<Vec<_>>();
        assert!(
            !columns.iter().any(|name| name == "priority"),
            "upsert path must not require priority column: {columns:?}"
        );

        let listed = list_records::<SampleRecord>(&paths, ProviderKind::Codex)
            .await
            .expect("list");
        assert_eq!(listed.get("acct-1.json"), Some(&record));

        let rows = list_rows(&paths, Some(ProviderKind::Codex), "user@")
            .await
            .expect("list rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account_id, "acct-1.json");

        // 全量快照替换：单事务后只剩 snapshot 中的账户。
        let record_b = SampleRecord {
            status: "active".to_string(),
            access_token: "token-2".to_string(),
        };
        let row_b = ProviderSnapshotRow {
            account_id: "acct-2.json".to_string(),
            email: Some("b@example.com".to_string()),
            expires_at: Some("2099-01-01T00:00:00Z".to_string()),
            expires_at_ms: Some(4_102_444_800_000),
            auth_method: Some("oauth".to_string()),
            provider_name: None,
            record_json: serde_json::to_string(&record_b).expect("ser"),
        };
        replace_provider_snapshot(&paths, ProviderKind::Codex, &[row_b])
            .await
            .expect("replace snapshot");
        let replaced = list_records::<SampleRecord>(&paths, ProviderKind::Codex)
            .await
            .expect("list after replace");
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced.get("acct-2.json"), Some(&record_b));
        assert!(!replaced.contains_key("acct-1.json"));

        // 单账户原子删除。
        replace_single_record::<SampleRecord>(
            &paths,
            ProviderKind::Codex,
            "acct-2.json",
            None,
            None,
        )
        .await
        .expect("delete single");
        let empty = list_records::<SampleRecord>(&paths, ProviderKind::Codex)
            .await
            .expect("list empty");
        assert!(empty.is_empty());

        let _ = std::fs::remove_dir_all(data_dir);
    }
}
