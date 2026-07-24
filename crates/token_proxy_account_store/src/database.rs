//! SQLite infrastructure owned by provider accounts.

use sqlx::Row;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool, Transaction,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};

use crate::paths::TokenProxyPaths;

static ACCOUNT_POOLS: OnceCell<Mutex<HashMap<PathBuf, SqlitePool>>> = OnceCell::const_new();

/// Opens a cached write pool and initializes the account-owned schema.
pub async fn open_write_pool(paths: &TokenProxyPaths) -> Result<SqlitePool, String> {
    let pools = ACCOUNT_POOLS
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;
    let db_path = paths.sqlite_db_path();
    let mut guard = pools.lock().await;
    if let Some(pool) = guard.get(&db_path) {
        return Ok(pool.clone());
    }
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Failed to create db directory: {error}"))?;
    }
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| format!("Failed to connect sqlite: {error}"))?;
    init_schema(&pool).await?;
    guard.insert(db_path, pool.clone());
    Ok(pool)
}

/// Account reads share the same WAL pool; serialization keeps migrations and
/// mutations ordered without creating another per-path pool cache.
pub async fn open_read_pool(paths: &TokenProxyPaths) -> Result<SqlitePool, String> {
    open_write_pool(paths).await
}

/// Initializes and migrates the provider-account projection.
///
/// `priority` is not part of the account projection (routing lives on Upstream).
/// Existing DBs that still have a `priority` column are rebuilt in the same
/// transaction as `record_json` cleanup so a failed row never leaves a half-migrated schema.
pub async fn init_schema(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS provider_accounts (
  provider_kind TEXT NOT NULL,
  account_id TEXT PRIMARY KEY,
  email TEXT,
  expires_at TEXT,
  expires_at_ms INTEGER,
  auth_method TEXT,
  provider_name TEXT,
  record_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|error| format!("Failed to create provider_accounts table: {error}"))?;

    // rebuild（若有 priority）+ record_json cleanup 同事务一次 commit；任一行失败整笔 rollback。
    migrate_provider_accounts(pool).await?;
    // 索引仅在迁移成功后确保；失败路径不得触碰旧索引/表。
    ensure_provider_account_indexes(pool).await?;
    sqlx::query("DROP TABLE IF EXISTS account_state_logs;")
        .execute(pool)
        .await
        .map_err(|error| format!("Failed to drop legacy account_state_logs table: {error}"))?;
    Ok(())
}

/// Single transaction: optional priority-column rebuild + all record_json cleanup.
async fn migrate_provider_accounts(pool: &SqlitePool) -> Result<(), String> {
    let columns = table_column_names(pool).await?;
    let needs_rebuild = columns.contains("priority");

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Failed to begin provider_accounts migration: {error}"))?;

    if needs_rebuild {
        tracing::info!(
            "provider_accounts migration: rebuilding table to drop legacy priority column"
        );
        rebuild_without_priority(&mut transaction).await?;
    } else {
        tracing::debug!("provider_accounts has no priority column; skip rebuild");
    }

    // 新 DB / 无 priority 表也走同一事务做 JSON 清理（空表 no-op）。
    migrate_record_json(&mut transaction).await?;

    transaction
        .commit()
        .await
        .map_err(|error| format!("Failed to commit provider_accounts migration: {error}"))?;

    if needs_rebuild {
        tracing::info!("provider_accounts migration committed; priority column removed");
    }
    Ok(())
}

/// Rename-copy rebuild that drops the legacy `priority` column (no DROP COLUMN).
async fn rebuild_without_priority(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TABLE provider_accounts_new (
  provider_kind TEXT NOT NULL,
  account_id TEXT PRIMARY KEY,
  email TEXT,
  expires_at TEXT,
  expires_at_ms INTEGER,
  auth_method TEXT,
  provider_name TEXT,
  record_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
"#,
    )
    .execute(&mut **transaction)
    .await
    .map_err(|error| format!("Failed to create provider_accounts_new: {error}"))?;

    sqlx::query(
        r#"
INSERT INTO provider_accounts_new (
  provider_kind, account_id, email, expires_at, expires_at_ms,
  auth_method, provider_name, record_json, updated_at_ms
)
SELECT
  provider_kind, account_id, email, expires_at, expires_at_ms,
  auth_method, provider_name, record_json, updated_at_ms
FROM provider_accounts;
"#,
    )
    .execute(&mut **transaction)
    .await
    .map_err(|error| format!("Failed to copy provider_accounts rows: {error}"))?;

    sqlx::query("DROP TABLE provider_accounts;")
        .execute(&mut **transaction)
        .await
        .map_err(|error| format!("Failed to drop legacy provider_accounts: {error}"))?;

    sqlx::query("ALTER TABLE provider_accounts_new RENAME TO provider_accounts;")
        .execute(&mut **transaction)
        .await
        .map_err(|error| format!("Failed to rename provider_accounts_new: {error}"))?;

    tracing::debug!("provider_accounts rebuild stage done (pending commit with json cleanup)");
    Ok(())
}

async fn ensure_provider_account_indexes(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_provider_accounts_kind_account_id ON provider_accounts(provider_kind, account_id);",
    )
    .execute(pool)
    .await
    .map_err(|error| format!("Failed to create provider account kind index: {error}"))?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_provider_accounts_email ON provider_accounts(email);",
    )
    .execute(pool)
    .await
    .map_err(|error| format!("Failed to create provider account email index: {error}"))?;
    Ok(())
}

async fn table_column_names(
    pool: &SqlitePool,
) -> Result<std::collections::HashSet<String>, String> {
    Ok(sqlx::query("PRAGMA table_info(provider_accounts);")
        .fetch_all(pool)
        .await
        .map_err(|error| format!("Failed to read provider_accounts schema: {error}"))?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect())
}

/// JSON cleanup inside the open migration transaction.
///
/// - `active` / `invalid` / `expired` kept as-is
/// - `disabled` or missing `status` → `active`
/// - non-string / unknown status → hard error with `account_id` only (no JSON / credentials)
/// - non-object `record_json` → hard fail
/// - strip `enabled` / `priority` / `proxy_url` (routing lives on Upstream)
async fn migrate_record_json(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), String> {
    let rows = sqlx::query("SELECT account_id, record_json FROM provider_accounts;")
        .fetch_all(&mut **transaction)
        .await
        .map_err(|error| {
            format!("Failed to read provider_accounts for status migration: {error}")
        })?;
    if rows.is_empty() {
        tracing::debug!("provider_accounts record_json cleanup: no rows");
        return Ok(());
    }

    let mut migrated = 0usize;
    for row in rows {
        let account_id = row
            .try_get::<String, _>("account_id")
            .map_err(|error| format!("Failed to decode provider account id: {error}"))?;
        let record_json = row
            .try_get::<String, _>("record_json")
            .map_err(|error| format!("Failed to decode provider account record: {error}"))?;
        let mut value =
            serde_json::from_str::<serde_json::Value>(&record_json).map_err(|error| {
                // 解析失败只带 account_id；不回显 record_json。
                format!("Failed to parse provider account record_json for {account_id}: {error}")
            })?;
        let Some(object) = value.as_object_mut() else {
            return Err(format!(
                "provider account {account_id} record_json is not a JSON object"
            ));
        };

        let previous = object.clone();
        let next_status = resolve_migration_status(object.get("status"), &account_id)?;
        object.insert(
            "status".to_string(),
            serde_json::Value::String(next_status.to_string()),
        );
        // 旧调度字段一律剥离；路由只看 Upstream。
        object.remove("enabled");
        object.remove("priority");
        object.remove("proxy_url");

        if *object == previous {
            continue;
        }

        let next_record_json = serde_json::to_string(&value).map_err(|error| {
            format!("Failed to serialize provider account {account_id}: {error}")
        })?;
        sqlx::query("UPDATE provider_accounts SET record_json = ? WHERE account_id = ?;")
            .bind(next_record_json)
            .bind(&account_id)
            .execute(&mut **transaction)
            .await
            .map_err(|error| format!("Failed to migrate provider account row: {error}"))?;
        migrated += 1;
        tracing::debug!(
            account_id,
            status = next_status,
            "provider account record_json migrated"
        );
    }
    if migrated > 0 {
        tracing::info!(
            migrated,
            "provider account record_json migration complete (pending commit)"
        );
    } else {
        tracing::debug!("provider account record_json cleanup: no rows needed update");
    }
    Ok(())
}

/// Map legacy status field to the allowed set; never log or return the raw value body.
fn resolve_migration_status(
    status: Option<&serde_json::Value>,
    account_id: &str,
) -> Result<&'static str, String> {
    match status {
        None => Ok("active"),
        Some(serde_json::Value::String(raw)) => match raw.trim() {
            "active" => Ok("active"),
            "invalid" => Ok("invalid"),
            "expired" => Ok("expired"),
            "disabled" => Ok("active"),
            _ => Err(format!(
                "provider account {account_id} has unknown status; expected active|invalid|expired|disabled"
            )),
        },
        Some(_) => Err(format!(
            "provider account {account_id} has non-string status; expected a string status"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{sqlite::SqlitePoolOptions, Row};

    async fn memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite")
    }

    async fn column_names(pool: &SqlitePool) -> Vec<String> {
        let mut names = table_column_names(pool)
            .await
            .expect("read columns")
            .into_iter()
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[tokio::test]
    async fn init_schema_creates_provider_accounts_without_priority() {
        let pool = memory_pool().await;

        init_schema(&pool).await.expect("init schema");

        let table = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'provider_accounts';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query sqlite schema");
        assert!(table.is_some());
        let columns = column_names(&pool).await;
        assert!(!columns.iter().any(|name| name == "priority"));
        assert!(columns.iter().any(|name| name == "record_json"));
        assert!(columns.iter().any(|name| name == "account_id"));
    }

    #[tokio::test]
    async fn init_schema_rebuilds_legacy_priority_column_and_cleans_json() {
        let pool = memory_pool().await;
        // 旧表：含 priority 列，无部分新元数据列也要能 copy 成功（全量列齐全）。
        sqlx::query(
            r#"
CREATE TABLE provider_accounts (
  provider_kind TEXT NOT NULL,
  account_id TEXT PRIMARY KEY,
  email TEXT,
  expires_at TEXT,
  expires_at_ms INTEGER,
  auth_method TEXT,
  provider_name TEXT,
  record_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE account_state_logs (id INTEGER PRIMARY KEY, ts_ms INTEGER NOT NULL);
"#,
        )
        .execute(&pool)
        .await
        .expect("create legacy schema");
        sqlx::query(
            r#"
INSERT INTO provider_accounts (
  provider_kind, account_id, record_json, updated_at_ms, priority
) VALUES
  ('codex', 'legacy-disabled.json',
   '{"enabled":false,"priority":7,"proxy_url":"http://127.0.0.1:7890","status":"disabled","access_token":"a"}', 0, 7),
  ('codex', 'legacy-invalid.json',
   '{"enabled":true,"priority":1,"status":"invalid","access_token":"b"}', 0, 1),
  ('codex', 'legacy-no-status.json',
   '{"enabled":false,"priority":3,"proxy_url":"socks5://proxy","access_token":"c"}', 0, 3);
"#,
        )
        .execute(&pool)
        .await
        .expect("insert legacy accounts");

        init_schema(&pool).await.expect("migrate account schema");

        let columns = column_names(&pool).await;
        assert!(
            !columns.iter().any(|name| name == "priority"),
            "priority column must be gone after rebuild: {columns:?}"
        );

        let rows = sqlx::query(
            "SELECT account_id, record_json FROM provider_accounts ORDER BY account_id ASC;",
        )
        .fetch_all(&pool)
        .await
        .expect("read migrated accounts");
        assert_eq!(rows.len(), 3);

        for row in rows {
            let account_id = row
                .try_get::<String, _>("account_id")
                .expect("decode account_id");
            let record_json = row
                .try_get::<String, _>("record_json")
                .expect("decode record");
            let record: serde_json::Value =
                serde_json::from_str(&record_json).expect("parse migrated record");
            assert!(
                record.get("enabled").is_none(),
                "{account_id} still has enabled"
            );
            assert!(
                record.get("priority").is_none(),
                "{account_id} still has priority"
            );
            assert!(
                record.get("proxy_url").is_none(),
                "{account_id} still has proxy_url"
            );
            let status = record
                .get("status")
                .and_then(serde_json::Value::as_str)
                .expect("status present");
            match account_id.as_str() {
                "legacy-invalid.json" => assert_eq!(status, "invalid"),
                "legacy-disabled.json" | "legacy-no-status.json" => {
                    assert_eq!(status, "active");
                }
                other => panic!("unexpected account_id {other}"),
            }
            // 迁移后 JSON 必须仍可当作对象反序列化（不引入 Disabled 等未知枚举值）。
            assert!(record.is_object());
        }

        let legacy_table = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'account_state_logs';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query legacy table");
        assert!(legacy_table.is_none());

        // 索引在 rebuild 后应存在。
        let index = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_provider_accounts_kind_account_id';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query index");
        assert!(index.is_some());
    }

    #[tokio::test]
    async fn init_schema_is_idempotent_after_priority_drop() {
        let pool = memory_pool().await;
        init_schema(&pool).await.expect("first init");
        init_schema(&pool).await.expect("second init");
        let columns = column_names(&pool).await;
        assert!(!columns.iter().any(|name| name == "priority"));
    }

    /// 半迁移防护：JSON status 非法时整笔 rollback，旧 priority 列与 record_json 字节保持原样。
    #[tokio::test]
    async fn init_schema_rolls_back_when_status_is_unknown_or_non_string() {
        let pool = memory_pool().await;
        sqlx::query(
            r#"
CREATE TABLE provider_accounts (
  provider_kind TEXT NOT NULL,
  account_id TEXT PRIMARY KEY,
  email TEXT,
  expires_at TEXT,
  expires_at_ms INTEGER,
  auth_method TEXT,
  provider_name TEXT,
  record_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0
);
"#,
        )
        .execute(&pool)
        .await
        .expect("create legacy schema");

        // 一行未知 status 字符串 + 一行非字符串 status；任一行失败都应 rollback。
        let unknown_json =
            r#"{"enabled":true,"priority":1,"status":"paused","access_token":"secret-a"}"#;
        let non_string_json =
            r#"{"enabled":false,"priority":2,"status":1,"access_token":"secret-b"}"#;
        sqlx::query(
            r#"
INSERT INTO provider_accounts (
  provider_kind, account_id, record_json, updated_at_ms, priority
) VALUES
  ('codex', 'bad-unknown.json', ?, 0, 1),
  ('codex', 'bad-non-string.json', ?, 0, 2);
"#,
        )
        .bind(unknown_json)
        .bind(non_string_json)
        .execute(&pool)
        .await
        .expect("insert bad rows");

        let err = init_schema(&pool)
            .await
            .expect_err("migration must fail on unknown/non-string status");
        // 安全错误：只含 account_id，不泄漏 JSON / 凭据。
        assert!(
            err.contains("bad-unknown.json") || err.contains("bad-non-string.json"),
            "error should name account_id: {err}"
        );
        assert!(
            !err.contains("secret-a")
                && !err.contains("secret-b")
                && !err.contains("access_token")
                && !err.contains(unknown_json)
                && !err.contains(non_string_json),
            "error must not leak credentials/json: {err}"
        );

        // 旧 schema 完整保留：priority 列仍在。
        let columns = column_names(&pool).await;
        assert!(
            columns.iter().any(|name| name == "priority"),
            "priority column must remain after rollback: {columns:?}"
        );

        // 行仍在且 record_json 字节与插入时完全一致。
        let rows = sqlx::query(
            "SELECT account_id, record_json, priority FROM provider_accounts ORDER BY account_id ASC;",
        )
        .fetch_all(&pool)
        .await
        .expect("read rows after failed migration");
        assert_eq!(rows.len(), 2);
        let by_id: std::collections::HashMap<String, (String, i64)> = rows
            .into_iter()
            .map(|row| {
                let id = row.try_get::<String, _>("account_id").expect("id");
                let json = row.try_get::<String, _>("record_json").expect("json");
                let priority = row.try_get::<i64, _>("priority").expect("priority");
                (id, (json, priority))
            })
            .collect();
        assert_eq!(
            by_id.get("bad-unknown.json").map(|(j, _)| j.as_str()),
            Some(unknown_json)
        );
        assert_eq!(
            by_id.get("bad-non-string.json").map(|(j, _)| j.as_str()),
            Some(non_string_json)
        );
        assert_eq!(by_id.get("bad-unknown.json").map(|(_, p)| *p), Some(1));
        assert_eq!(by_id.get("bad-non-string.json").map(|(_, p)| *p), Some(2));

        // 失败不得留下 rebuild 中间表。
        let leftover = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'provider_accounts_new';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query leftover table");
        assert!(
            leftover.is_none(),
            "rebuild temp table must not survive rollback"
        );
    }
}
