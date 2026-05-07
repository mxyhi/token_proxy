use sqlx::Row;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};

use crate::paths::TokenProxyPaths;

struct SqlitePools {
    read: SqlitePool,
    write: SqlitePool,
}

// 进程内复用连接池，避免频繁建池与 schema/index 检查。
static SQLITE_POOLS: OnceCell<Mutex<HashMap<PathBuf, SqlitePools>>> = OnceCell::const_new();

pub async fn open_read_pool(paths: &TokenProxyPaths) -> Result<SqlitePool, String> {
    let pools = open_pools(paths).await?;
    Ok(pools.read)
}

pub async fn open_write_pool(paths: &TokenProxyPaths) -> Result<SqlitePool, String> {
    let pools = open_pools(paths).await?;
    Ok(pools.write)
}

async fn open_pools(paths: &TokenProxyPaths) -> Result<SqlitePools, String> {
    let pools_map = SQLITE_POOLS
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;

    let db_path = paths.sqlite_db_path();
    let mut guard = pools_map.lock().await;
    if let Some(pools) = guard.get(&db_path) {
        return Ok(SqlitePools {
            read: pools.read.clone(),
            write: pools.write.clone(),
        });
    }

    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| format!("Failed to create db directory: {err}"))?;
    }
    let read = connect_pool(&db_path).await?;
    init_schema(&read).await?;
    let write = connect_pool(&db_path).await?;
    init_schema(&write).await?;
    guard.insert(
        db_path,
        SqlitePools {
            read: read.clone(),
            write: write.clone(),
        },
    );
    Ok(SqlitePools { read, write })
}

async fn connect_pool(path: &PathBuf) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));

    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|err| format!("Failed to connect sqlite: {err}"))
}

pub async fn init_schema(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS request_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts_ms INTEGER NOT NULL,
  path TEXT NOT NULL,
  provider TEXT NOT NULL,
  upstream_id TEXT NOT NULL,
  account_id TEXT,
  model TEXT,
  mapped_model TEXT,
  stream INTEGER NOT NULL,
  status INTEGER NOT NULL,
  input_tokens INTEGER,
  output_tokens INTEGER,
  total_tokens INTEGER,
  cached_tokens INTEGER,
  usage_json TEXT,
  upstream_request_id TEXT,
  request_headers TEXT,
  request_body TEXT,
  response_error TEXT,
  latency_ms INTEGER NOT NULL,
  upstream_first_byte_ms INTEGER,
  upstream_response_headers_ms INTEGER,
  upstream_first_body_chunk_ms INTEGER,
  first_client_flush_ms INTEGER,
  first_output_ms INTEGER,
  cost_nano_usd INTEGER,
  pricing_version TEXT,
  pricing_model TEXT,
  pricing_context_tier TEXT
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create request_logs table: {err}"))?;

    ensure_request_logs_columns(pool).await?;
    super::pricing::init_model_pricing_table(pool).await?;
    super::pricing::backfill_request_log_costs(pool).await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_request_logs_ts_ms ON request_logs(ts_ms);")
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to create idx_request_logs_ts_ms: {err}"))?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_request_logs_provider_ts_ms ON request_logs(provider, ts_ms);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_request_logs_provider_ts_ms: {err}"))?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_request_logs_upstream_ts_ms ON request_logs(upstream_id, ts_ms);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_request_logs_upstream_ts_ms: {err}"))?;

    // 复合索引：优化中位数延迟查询（按时间范围过滤后按延迟排序）
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_request_logs_ts_latency ON request_logs(ts_ms, latency_ms);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_request_logs_ts_latency: {err}"))?;

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
  updated_at_ms INTEGER NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create provider_accounts table: {err}"))?;

    ensure_provider_accounts_columns(pool).await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_provider_accounts_kind_account_id ON provider_accounts(provider_kind, account_id);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_provider_accounts_kind_account_id: {err}"))?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_provider_accounts_email ON provider_accounts(email);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_provider_accounts_email: {err}"))?;

    sqlx::query("DROP TABLE IF EXISTS account_state_logs;")
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to drop legacy account_state_logs table: {err}"))?;

    migrate_provider_account_status(pool).await?;

    Ok(())
}

async fn ensure_request_logs_columns(pool: &SqlitePool) -> Result<(), String> {
    let columns = sqlx::query("PRAGMA table_info(request_logs);")
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to read request_logs schema: {err}"))?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<std::collections::HashSet<_>>();

    if !columns.contains("cached_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN cached_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add cached_tokens column: {err}"))?;
    }

    if !columns.contains("mapped_model") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN mapped_model TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add mapped_model column: {err}"))?;
    }

    if !columns.contains("usage_json") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN usage_json TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add usage_json column: {err}"))?;
    }

    if !columns.contains("request_headers") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN request_headers TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add request_headers column: {err}"))?;
    }

    if !columns.contains("request_body") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN request_body TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add request_body column: {err}"))?;
    }

    if !columns.contains("response_error") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN response_error TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add response_error column: {err}"))?;
    }

    if !columns.contains("account_id") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN account_id TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add account_id column: {err}"))?;
    }

    if !columns.contains("upstream_first_byte_ms") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN upstream_first_byte_ms INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add upstream_first_byte_ms column: {err}"))?;
    }

    if !columns.contains("upstream_response_headers_ms") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN upstream_response_headers_ms INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add upstream_response_headers_ms column: {err}"))?;
    }

    if !columns.contains("upstream_first_body_chunk_ms") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN upstream_first_body_chunk_ms INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add upstream_first_body_chunk_ms column: {err}"))?;
    }

    if !columns.contains("first_client_flush_ms") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN first_client_flush_ms INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add first_client_flush_ms column: {err}"))?;
    }

    if !columns.contains("first_output_ms") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN first_output_ms INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add first_output_ms column: {err}"))?;
    }

    if !columns.contains("cost_nano_usd") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN cost_nano_usd INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add cost_nano_usd column: {err}"))?;
    }

    if !columns.contains("pricing_version") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN pricing_version TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add pricing_version column: {err}"))?;
    }

    if !columns.contains("pricing_model") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN pricing_model TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add pricing_model column: {err}"))?;
    }

    if !columns.contains("pricing_context_tier") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN pricing_context_tier TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add pricing_context_tier column: {err}"))?;
    }

    Ok(())
}

async fn ensure_provider_accounts_columns(pool: &SqlitePool) -> Result<(), String> {
    let columns = sqlx::query("PRAGMA table_info(provider_accounts);")
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to read provider_accounts schema: {err}"))?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<std::collections::HashSet<_>>();

    if !columns.contains("priority") {
        sqlx::query(
            "ALTER TABLE provider_accounts ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;",
        )
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to add priority column: {err}"))?;
    }

    Ok(())
}

async fn migrate_provider_account_status(pool: &SqlitePool) -> Result<(), String> {
    let rows = sqlx::query("SELECT account_id, record_json FROM provider_accounts;")
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to read provider_accounts for status migration: {err}"))?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|err| format!("Failed to begin provider_accounts migration transaction: {err}"))?;

    for row in rows {
        let account_id = row
            .try_get::<String, _>("account_id")
            .map_err(|err| format!("Failed to decode provider_accounts.account_id: {err}"))?;
        let record_json = row
            .try_get::<String, _>("record_json")
            .map_err(|err| format!("Failed to decode provider_accounts.record_json: {err}"))?;
        let mut value = serde_json::from_str::<serde_json::Value>(&record_json).map_err(|err| {
            format!("Failed to parse provider_accounts.record_json for {account_id}: {err}")
        })?;
        let Some(object) = value.as_object_mut() else {
            continue;
        };

        let mut changed = false;
        if !object.contains_key("status") {
            let next_status = if object
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .is_some_and(|enabled| !enabled)
            {
                "disabled"
            } else {
                "active"
            };
            object.insert(
                "status".to_string(),
                serde_json::Value::String(next_status.to_string()),
            );
            changed = true;
        }
        if object.remove("enabled").is_some() {
            changed = true;
        }
        let priority = object
            .get("priority")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0);
        if object
            .insert(
                "priority".to_string(),
                serde_json::Value::Number(serde_json::Number::from(priority)),
            )
            .is_none()
        {
            changed = true;
        }
        let next_record_json = serde_json::to_string(&value).map_err(|err| {
            format!("Failed to serialize migrated provider_accounts.record_json for {account_id}: {err}")
        })?;
        if !changed {
            sqlx::query("UPDATE provider_accounts SET priority = ? WHERE account_id = ?;")
                .bind(priority)
                .bind(account_id)
                .execute(&mut *tx)
                .await
                .map_err(|err| format!("Failed to update provider_accounts priority row: {err}"))?;
            continue;
        }
        sqlx::query(
            "UPDATE provider_accounts SET record_json = ?, priority = ? WHERE account_id = ?;",
        )
        .bind(next_record_json)
        .bind(priority)
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| format!("Failed to update migrated provider_accounts row: {err}"))?;
    }

    tx.commit()
        .await
        .map_err(|err| format!("Failed to commit provider_accounts status migration: {err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_logs_schema_includes_timing_and_cost_columns() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        init_schema(&pool).await.expect("init schema");

        let columns = sqlx::query("PRAGMA table_info(request_logs);")
            .fetch_all(&pool)
            .await
            .expect("read schema")
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("name").ok())
            .collect::<std::collections::HashSet<_>>();

        assert!(columns.contains("upstream_first_byte_ms"));
        assert!(columns.contains("upstream_response_headers_ms"));
        assert!(columns.contains("upstream_first_body_chunk_ms"));
        assert!(columns.contains("first_client_flush_ms"));
        assert!(columns.contains("first_output_ms"));
        assert!(columns.contains("cost_nano_usd"));
        assert!(columns.contains("pricing_version"));
        assert!(columns.contains("pricing_model"));
        assert!(columns.contains("pricing_context_tier"));

        let pricing_table = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'model_pricing_settings';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query sqlite_master");
        assert!(pricing_table.is_some());
    }

    #[tokio::test]
    async fn init_schema_backfills_request_log_costs_when_pricing_version_changes() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        init_schema(&pool).await.expect("init schema");
        sqlx::query(
            r#"
INSERT INTO request_logs (
  ts_ms,
  path,
  provider,
  upstream_id,
  model,
  mapped_model,
  stream,
  status,
  input_tokens,
  output_tokens,
  total_tokens,
  cached_tokens,
  latency_ms,
  pricing_version
) VALUES (
  1,
  '/v1/responses',
  'openai-response',
  'airouter',
  'alias',
  'gpt-5.4',
  0,
  200,
  1000000,
  10000,
  1010000,
  200000,
  30,
  'old'
);
"#,
        )
        .execute(&pool)
        .await
        .expect("insert old request log");

        init_schema(&pool).await.expect("backfill schema");

        let row = sqlx::query(
            r#"
SELECT cost_nano_usd, pricing_version, pricing_model, pricing_context_tier
FROM request_logs
LIMIT 1;
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("select backfilled cost");

        assert_eq!(
            row.try_get::<i64, _>("cost_nano_usd").ok(),
            Some(4_325_000_000)
        );
        assert_eq!(
            row.try_get::<String, _>("pricing_version").ok().as_deref(),
            Some(crate::proxy::pricing::DEFAULT_PRICING_VERSION)
        );
        assert_eq!(
            row.try_get::<String, _>("pricing_model").ok().as_deref(),
            Some("gpt-5.4")
        );
        assert_eq!(
            row.try_get::<String, _>("pricing_context_tier")
                .ok()
                .as_deref(),
            Some("long")
        );
    }

    #[tokio::test]
    async fn init_schema_creates_provider_accounts_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        init_schema(&pool).await.expect("init schema");

        let row = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'provider_accounts';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query sqlite_master");

        assert!(row.is_some(), "provider_accounts table should exist");
    }

    #[tokio::test]
    async fn init_schema_migrates_provider_account_enabled_to_status() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

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
  updated_at_ms INTEGER NOT NULL
);
"#,
        )
        .execute(&pool)
        .await
        .expect("create legacy provider_accounts");

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
) VALUES (
  'codex',
  'codex-legacy.json',
  'legacy@example.com',
  '2026-04-01T00:00:00Z',
  0,
  NULL,
  NULL,
  '{"access_token":"a","refresh_token":"r","id_token":"i","auto_refresh_enabled":true,"enabled":false,"email":"legacy@example.com","expires_at":"2026-04-01T00:00:00Z","last_refresh":null,"proxy_url":null,"quota":{"plan_type":null,"quotas":[],"error":null,"checked_at":null}}',
  0
);
"#,
        )
        .execute(&pool)
        .await
        .expect("insert legacy provider account");

        init_schema(&pool).await.expect("migrate schema");

        let row = sqlx::query("SELECT record_json FROM provider_accounts WHERE account_id = ?;")
            .bind("codex-legacy.json")
            .fetch_one(&pool)
            .await
            .expect("select migrated record_json");
        let record_json = row
            .try_get::<String, _>("record_json")
            .expect("decode record_json");
        let value: serde_json::Value =
            serde_json::from_str(&record_json).expect("parse record_json");

        assert_eq!(
            value.get("status").and_then(serde_json::Value::as_str),
            Some("disabled")
        );
        assert!(
            value.get("enabled").is_none(),
            "legacy enabled field should be removed after migration"
        );
    }

    #[tokio::test]
    async fn init_schema_drops_legacy_account_state_logs_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        sqlx::query(
            r#"
CREATE TABLE account_state_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts_ms INTEGER NOT NULL
);
"#,
        )
        .execute(&pool)
        .await
        .expect("create legacy account_state_logs");

        init_schema(&pool).await.expect("init schema");

        let row = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'account_state_logs';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query sqlite_master");

        assert!(
            row.is_none(),
            "legacy account_state_logs table should be dropped"
        );
    }

    #[tokio::test]
    async fn init_schema_adds_provider_account_priority_column_with_default_zero() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

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
  updated_at_ms INTEGER NOT NULL
);
"#,
        )
        .execute(&pool)
        .await
        .expect("create legacy provider_accounts");

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
) VALUES (
  'kiro',
  'kiro-priority.json',
  'priority@example.com',
  '2026-04-01T00:00:00Z',
  0,
  'google',
  'kiro',
  '{"access_token":"a","refresh_token":"r","profile_arn":"arn","expires_at":"2026-04-01T00:00:00Z","auth_method":"google","provider":"kiro","client_id":null,"client_secret":null,"email":"priority@example.com","last_refresh":null,"start_url":null,"region":null,"status":"active","proxy_url":null,"quota":{"plan_type":null,"quotas":[],"error":null,"checked_at":null}}',
  0
);
"#,
        )
        .execute(&pool)
        .await
        .expect("insert legacy provider account");

        init_schema(&pool).await.expect("migrate schema");

        let columns = sqlx::query("PRAGMA table_info(provider_accounts);")
            .fetch_all(&pool)
            .await
            .expect("read provider_accounts schema");
        let priority_column = columns
            .into_iter()
            .find(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("priority"))
            .expect("priority column should exist");
        assert_eq!(
            priority_column
                .try_get::<Option<String>, _>("dflt_value")
                .expect("decode dflt_value")
                .as_deref(),
            Some("0")
        );

        let row = sqlx::query("SELECT priority FROM provider_accounts WHERE account_id = ?;")
            .bind("kiro-priority.json")
            .fetch_one(&pool)
            .await
            .expect("select migrated priority");
        assert_eq!(
            row.try_get::<i64, _>("priority").expect("decode priority"),
            0
        );
    }
}
