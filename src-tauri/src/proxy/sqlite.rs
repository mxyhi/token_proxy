use sqlx::Row;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::path::PathBuf;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::OnceCell;

use super::config;

const DB_FILE_NAME: &str = "data.db";

struct SqlitePools {
    read: SqlitePool,
    write: SqlitePool,
}

// 只初始化一次，避免每次刷新重复建池与 schema/index 检查。
static SQLITE_POOLS: OnceCell<SqlitePools> = OnceCell::const_new();

pub(crate) async fn open_read_pool(app: &AppHandle) -> Result<SqlitePool, String> {
    let pools = open_pools(app).await?;
    Ok(pools.read.clone())
}

pub(crate) async fn open_write_pool(app: &AppHandle) -> Result<SqlitePool, String> {
    let pools = open_pools(app).await?;
    Ok(pools.write.clone())
}

async fn open_pools(app: &AppHandle) -> Result<&'static SqlitePools, String> {
    let app = app.clone();
    SQLITE_POOLS
        .get_or_try_init(|| async move {
            let path = usage_db_path(&app)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|err| format!("Failed to create db directory: {err}"))?;
            }
            let read = connect_pool(&path).await?;
            init_schema(&read).await?;
            let write = connect_pool(&path).await?;
            init_schema(&write).await?;
            Ok(SqlitePools { read, write })
        })
        .await
}

fn usage_db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config::config_dir_path(app)?.join(DB_FILE_NAME))
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

pub(crate) async fn init_schema(pool: &SqlitePool) -> Result<(), String> {
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

    Ok(())
}
