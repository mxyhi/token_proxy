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
  latency_ms INTEGER NOT NULL
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create request_logs table: {err}"))?;

    ensure_request_logs_columns(pool).await?;

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

    Ok(())
}
