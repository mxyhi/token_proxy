use sqlx::Row;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, OnceCell};

mod usage_backfill;

struct SqlitePools {
    read: SqlitePool,
    write: SqlitePool,
}

// 进程内复用连接池，避免频繁建池与 schema/index 检查。
static SQLITE_POOLS: OnceCell<Mutex<HashMap<PathBuf, SqlitePools>>> = OnceCell::const_new();

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
const REQUEST_DETAIL_RETENTION_DAYS: i64 = 7;
const ERROR_REQUEST_RETENTION_DAYS: i64 = 7;
const RETENTION_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Default, Eq, PartialEq)]
struct RequestLogRetentionStats {
    deleted_error_requests: u64,
    cleared_request_details: u64,
}

pub async fn open_read_pool(db_path: &Path) -> Result<SqlitePool, String> {
    let pools = open_pools(db_path).await?;
    Ok(pools.read)
}

pub async fn open_write_pool(db_path: &Path) -> Result<SqlitePool, String> {
    let pools = open_pools(db_path).await?;
    Ok(pools.write)
}

async fn open_pools(db_path: &Path) -> Result<SqlitePools, String> {
    let pools_map = SQLITE_POOLS
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;

    let db_path = db_path.to_path_buf();
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
        db_path.clone(),
        SqlitePools {
            read: read.clone(),
            write: write.clone(),
        },
    );
    spawn_request_log_retention(write.clone(), db_path);
    Ok(SqlitePools { read, write })
}

fn spawn_request_log_retention(pool: SqlitePool, db_path: PathBuf) {
    tokio::spawn(async move {
        loop {
            let started_at = Instant::now();
            match retain_request_logs(&pool, current_time_ms()).await {
                Ok(stats) => tracing::info!(
                    database = %db_path.display(),
                    deleted_error_requests = stats.deleted_error_requests,
                    cleared_request_details = stats.cleared_request_details,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "request log retention completed"
                ),
                Err(error) => tracing::error!(
                    database = %db_path.display(),
                    error = %error,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "request log retention failed"
                ),
            }
            tokio::time::sleep(RETENTION_INTERVAL).await;
        }
    });
}

async fn retain_request_logs(
    pool: &SqlitePool,
    now_ms: i64,
) -> Result<RequestLogRetentionStats, String> {
    let detail_cutoff_ms = now_ms.saturating_sub(REQUEST_DETAIL_RETENTION_DAYS * DAY_MS);
    let error_cutoff_ms = now_ms.saturating_sub(ERROR_REQUEST_RETENTION_DAYS * DAY_MS);
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Failed to begin request log retention: {error}"))?;

    // 错误请求只保留七天，超过保留期后整行删除，不参与长期统计。
    let deleted_error_requests =
        sqlx::query("DELETE FROM request_logs WHERE status >= 400 AND ts_ms < ?;")
            .bind(error_cutoff_ms)
            .execute(&mut *transaction)
            .await
            .map_err(|error| format!("Failed to delete expired error requests: {error}"))?
            .rows_affected();

    // 成功请求永久保留统计字段（含 usage_json）；七天后只清临时排障字段。
    // client_ip 属隐私/排障信息，不参与长期用量与成本统计，一并清空。
    let cleared_request_details = sqlx::query(
        r#"
UPDATE request_logs
SET request_headers = NULL,
    request_body = NULL,
    response_body = NULL,
    client_ip = NULL
WHERE status < 400
  AND ts_ms < ?
  AND (
    request_headers IS NOT NULL
    OR request_body IS NOT NULL
    OR response_body IS NOT NULL
    OR client_ip IS NOT NULL
  );
"#,
    )
    .bind(detail_cutoff_ms)
    .execute(&mut *transaction)
    .await
    .map_err(|error| format!("Failed to clear expired request details: {error}"))?
    .rows_affected();

    transaction
        .commit()
        .await
        .map_err(|error| format!("Failed to commit request log retention: {error}"))?;

    Ok(RequestLogRetentionStats {
        deleted_error_requests,
        cleared_request_details,
    })
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
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
  client_ip TEXT,
  path TEXT NOT NULL,
  provider TEXT NOT NULL,
  upstream_id TEXT NOT NULL,
  account_id TEXT,
  model TEXT,
  mapped_model TEXT,
  upstream_response_model TEXT,
  stream INTEGER NOT NULL,
  status INTEGER NOT NULL,
  input_tokens INTEGER,
  output_tokens INTEGER,
  total_tokens INTEGER,
  uncached_input_tokens INTEGER,
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  cache_write_5m_tokens INTEGER,
  cache_write_1h_tokens INTEGER,
  image_input_tokens INTEGER,
  image_output_tokens INTEGER,
  service_tier TEXT,
  usage_json TEXT,
  upstream_request_id TEXT,
  request_headers TEXT,
  request_body TEXT,
  response_body TEXT,
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
  pricing_context_tier TEXT,
  client_request_id TEXT,
  attempt_index INTEGER,
  is_billable INTEGER NOT NULL DEFAULT 1
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create request_logs table: {err}"))?;

    ensure_request_logs_columns(pool).await?;
    // SELECT * 视图在创建时冻结列。加列后必须重建，否则最近请求读不到 upstream_response_model。
    sqlx::query("DROP VIEW IF EXISTS billable_request_logs;")
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to drop billable_request_logs view: {err}"))?;
    sqlx::query(
        "CREATE VIEW billable_request_logs AS SELECT * FROM request_logs WHERE is_billable = 1;",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create billable_request_logs view: {err}"))?;
    usage_backfill::backfill_request_log_usage(pool).await?;
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

    // 归因更新只扫描同一客户端请求的 attempts，避免日志表增长后逐次全表扫描。
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_request_logs_client_attempt ON request_logs(client_request_id, attempt_index DESC, id DESC);",
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create idx_request_logs_client_attempt: {err}"))?;

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

    if !columns.contains("cache_read_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN cache_read_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add cache_read_tokens column: {err}"))?;
    }

    if !columns.contains("uncached_input_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN uncached_input_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add uncached_input_tokens column: {err}"))?;
    }

    if !columns.contains("cache_write_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN cache_write_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add cache_write_tokens column: {err}"))?;
    }

    if !columns.contains("cache_write_5m_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN cache_write_5m_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add cache_write_5m_tokens column: {err}"))?;
    }

    if !columns.contains("cache_write_1h_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN cache_write_1h_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add cache_write_1h_tokens column: {err}"))?;
    }

    if !columns.contains("image_input_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN image_input_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add image_input_tokens column: {err}"))?;
    }

    if !columns.contains("image_output_tokens") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN image_output_tokens INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add image_output_tokens column: {err}"))?;
    }

    if !columns.contains("service_tier") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN service_tier TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add service_tier column: {err}"))?;
    }

    if !columns.contains("client_ip") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN client_ip TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add client_ip column: {err}"))?;
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

    if !columns.contains("response_body") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN response_body TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add response_body column: {err}"))?;
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

    if !columns.contains("client_request_id") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN client_request_id TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add client_request_id column: {err}"))?;
    }

    if !columns.contains("attempt_index") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN attempt_index INTEGER;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add attempt_index column: {err}"))?;
    }

    if !columns.contains("is_billable") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN is_billable INTEGER NOT NULL DEFAULT 1;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add is_billable column: {err}"))?;
    }

    if !columns.contains("upstream_response_model") {
        sqlx::query("ALTER TABLE request_logs ADD COLUMN upstream_response_model TEXT;")
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to add upstream_response_model column: {err}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn insert_retention_test_log(
        pool: &SqlitePool,
        ts_ms: i64,
        status: i64,
        detail: &str,
        client_ip: Option<&str>,
        usage_json: Option<&str>,
    ) -> i64 {
        sqlx::query(
            r#"
INSERT INTO request_logs (
  ts_ms, path, provider, upstream_id, stream, status, client_ip,
  request_headers, request_body, response_body, response_error, usage_json, latency_ms
) VALUES (?, '/v1/responses', 'openai-response', 'test', 0, ?, ?, ?, ?, ?, ?, ?, 10);
"#,
        )
        .bind(ts_ms)
        .bind(status)
        .bind(client_ip)
        .bind(detail)
        .bind(detail)
        .bind(detail)
        .bind(detail)
        .bind(usage_json)
        .execute(pool)
        .await
        .expect("insert retention test log")
        .last_insert_rowid()
    }

    #[tokio::test]
    async fn request_log_retention_deletes_errors_and_expires_success_details() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        init_schema(&pool).await.expect("init schema");
        let now_ms = 100 * DAY_MS;

        let old_error_id = insert_retention_test_log(
            &pool,
            now_ms - 8 * DAY_MS,
            400,
            "old error",
            Some("1.1.1.1"),
            Some(r#"{"input":1}"#),
        )
        .await;
        let recent_error_id = insert_retention_test_log(
            &pool,
            now_ms - 6 * DAY_MS,
            500,
            "recent error",
            Some("2.2.2.2"),
            Some(r#"{"input":2}"#),
        )
        .await;
        let old_success_id = insert_retention_test_log(
            &pool,
            now_ms - 8 * DAY_MS,
            200,
            "old success",
            Some("3.3.3.3"),
            Some(r#"{"input":3}"#),
        )
        .await;
        let old_redirect_id = insert_retention_test_log(
            &pool,
            now_ms - 8 * DAY_MS,
            399,
            "old redirect",
            Some("4.4.4.4"),
            Some(r#"{"input":4}"#),
        )
        .await;
        // 成功请求永久保留：超过原 90 天窗口也不删行，只清临时排障字段。
        let ancient_success_id = insert_retention_test_log(
            &pool,
            now_ms - 91 * DAY_MS,
            200,
            "ancient success",
            Some("5.5.5.5"),
            Some(r#"{"input":5}"#),
        )
        .await;

        let stats = retain_request_logs(&pool, now_ms)
            .await
            .expect("retain request logs");

        assert_eq!(
            stats,
            RequestLogRetentionStats {
                deleted_error_requests: 1,
                // old_success + old_redirect + ancient_success
                cleared_request_details: 3,
            }
        );
        let deleted_error_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM request_logs WHERE id = ?;")
                .bind(old_error_id)
                .fetch_one(&pool)
                .await
                .expect("count deleted error request log");
        assert_eq!(deleted_error_count, 0);

        let recent_error = sqlx::query(
            "SELECT response_error, client_ip, usage_json FROM request_logs WHERE id = ?;",
        )
        .bind(recent_error_id)
        .fetch_one(&pool)
        .await
        .expect("read recent error detail");
        assert_eq!(
            recent_error
                .try_get::<Option<String>, _>("response_error")
                .ok()
                .flatten()
                .as_deref(),
            Some("recent error")
        );
        assert_eq!(
            recent_error
                .try_get::<Option<String>, _>("client_ip")
                .ok()
                .flatten()
                .as_deref(),
            Some("2.2.2.2")
        );
        assert_eq!(
            recent_error
                .try_get::<Option<String>, _>("usage_json")
                .ok()
                .flatten()
                .as_deref(),
            Some(r#"{"input":2}"#)
        );

        for (retained_id, expected_error, expected_usage) in [
            (old_success_id, "old success", r#"{"input":3}"#),
            (old_redirect_id, "old redirect", r#"{"input":4}"#),
            (ancient_success_id, "ancient success", r#"{"input":5}"#),
        ] {
            let details = sqlx::query(
                r#"
SELECT request_headers, request_body, response_body, response_error, client_ip, usage_json
FROM request_logs WHERE id = ?;
"#,
            )
            .bind(retained_id)
            .fetch_one(&pool)
            .await
            .expect("read retained request log");
            assert_eq!(
                details
                    .try_get::<Option<String>, _>("request_headers")
                    .ok()
                    .flatten(),
                None
            );
            assert_eq!(
                details
                    .try_get::<Option<String>, _>("request_body")
                    .ok()
                    .flatten(),
                None
            );
            assert_eq!(
                details
                    .try_get::<Option<String>, _>("response_body")
                    .ok()
                    .flatten(),
                None
            );
            assert_eq!(
                details
                    .try_get::<Option<String>, _>("client_ip")
                    .ok()
                    .flatten(),
                None
            );
            assert_eq!(
                details
                    .try_get::<Option<String>, _>("response_error")
                    .ok()
                    .flatten()
                    .as_deref(),
                Some(expected_error)
            );
            // usage_json 是长期统计原始事实，永不因 retention 清空。
            assert_eq!(
                details
                    .try_get::<Option<String>, _>("usage_json")
                    .ok()
                    .flatten()
                    .as_deref(),
                Some(expected_usage)
            );
        }
    }

    #[tokio::test]
    async fn request_logs_schema_includes_timing_cost_and_client_ip_columns() {
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
        assert!(columns.contains("client_ip"));
        assert!(columns.contains("upstream_response_headers_ms"));
        assert!(columns.contains("upstream_first_body_chunk_ms"));
        assert!(columns.contains("first_client_flush_ms"));
        assert!(columns.contains("first_output_ms"));
        assert!(columns.contains("cost_nano_usd"));
        assert!(columns.contains("cache_read_tokens"));
        assert!(columns.contains("cache_write_tokens"));
        assert!(columns.contains("uncached_input_tokens"));
        assert!(columns.contains("cache_write_5m_tokens"));
        assert!(columns.contains("cache_write_1h_tokens"));
        assert!(columns.contains("image_input_tokens"));
        assert!(columns.contains("image_output_tokens"));
        assert!(columns.contains("service_tier"));
        assert!(columns.contains("pricing_version"));
        assert!(columns.contains("pricing_model"));
        assert!(columns.contains("pricing_context_tier"));
        assert!(columns.contains("client_request_id"));
        assert!(columns.contains("attempt_index"));
        assert!(columns.contains("is_billable"));
        assert!(columns.contains("upstream_response_model"));

        let billing_index = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_request_logs_client_attempt';",
        )
        .fetch_optional(&pool)
        .await
        .expect("query billing index");
        assert!(billing_index.is_some());

        let pricing_table = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'model_pricing_catalog_cache';",
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
  uncached_input_tokens,
  cache_read_tokens,
  cache_write_tokens,
  cache_write_5m_tokens,
  cache_write_1h_tokens,
  image_input_tokens,
  image_output_tokens,
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
  800000,
  200000,
  0,
  0,
  0,
  0,
  0,
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
            Some(
                crate::pricing::default_model_pricing_settings()
                    .version
                    .as_str()
            )
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
    async fn init_schema_backfills_precise_usage_from_legacy_usage_json() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        init_schema(&pool).await.expect("init schema");
        let pricing_version = crate::pricing::default_model_pricing_settings().version;
        sqlx::query(
            r#"
INSERT INTO request_logs (
  ts_ms, path, provider, upstream_id, stream, status,
  input_tokens, output_tokens, total_tokens, usage_json, latency_ms,
  cost_nano_usd, pricing_version
) VALUES (
  1, '/v1/messages', 'anthropic', 'legacy', 0, 200,
  10, 2, 12,
  '{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":4,"cache_creation_input_tokens":8,"cache_creation":{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":2},"service_tier":"priority"}',
  30, 123, ?
);
"#,
        )
        .bind(&pricing_version)
        .execute(&pool)
        .await
        .expect("insert legacy usage row");

        init_schema(&pool).await.expect("backfill legacy usage");

        let row = sqlx::query(
            r#"
SELECT
  input_tokens,
  output_tokens,
  total_tokens,
  uncached_input_tokens,
  cache_read_tokens,
  cache_write_tokens,
  cache_write_5m_tokens,
  cache_write_1h_tokens,
  service_tier,
  cost_nano_usd,
  pricing_version
FROM request_logs
LIMIT 1;
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("select backfilled usage");

        assert_eq!(row.try_get::<i64, _>("input_tokens").ok(), Some(22));
        assert_eq!(row.try_get::<i64, _>("output_tokens").ok(), Some(2));
        assert_eq!(row.try_get::<i64, _>("total_tokens").ok(), Some(24));
        assert_eq!(
            row.try_get::<i64, _>("uncached_input_tokens").ok(),
            Some(10)
        );
        assert_eq!(row.try_get::<i64, _>("cache_read_tokens").ok(), Some(4));
        assert_eq!(row.try_get::<i64, _>("cache_write_tokens").ok(), Some(3));
        assert_eq!(row.try_get::<i64, _>("cache_write_5m_tokens").ok(), Some(3));
        assert_eq!(row.try_get::<i64, _>("cache_write_1h_tokens").ok(), Some(2));
        assert_eq!(
            row.try_get::<String, _>("service_tier").ok().as_deref(),
            Some("priority")
        );
        assert_eq!(row.try_get::<i64, _>("cost_nano_usd").ok(), Some(123));
        assert_eq!(
            row.try_get::<String, _>("pricing_version").ok().as_deref(),
            Some(pricing_version.as_str())
        );

        init_schema(&pool).await.expect("repeat usage backfill");
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_logs WHERE cache_read_tokens = 4;",
        )
        .fetch_one(&pool)
        .await
        .expect("count backfilled usage rows");
        assert_eq!(count, 1);
    }
}
