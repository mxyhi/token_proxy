//! 历史日志回填的完成进度。
//!
//! 回填条件依赖排在大字段 `usage_json` 之后的列，扫描每行都要读溢出页，
//! 大库全表扫一次要十秒级。记录"某版本规则已处理到哪个 id"，之后只扫新增日志。

use sqlx::{Row, SqlitePool};

pub(crate) async fn init_table(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS request_log_backfill_marks (
  name TEXT PRIMARY KEY,
  version TEXT NOT NULL,
  last_id INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create request_log_backfill_marks table: {err}"))?;
    Ok(())
}

/// 返回同一规则版本下已完成回填的最大日志 id；版本变化或从未完成时返回 0（从头扫）。
pub(crate) async fn read(pool: &SqlitePool, name: &str, version: &str) -> Result<i64, String> {
    let row =
        sqlx::query("SELECT version, last_id FROM request_log_backfill_marks WHERE name = ?;")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(|err| format!("Failed to read {name} backfill mark: {err}"))?;
    let Some(row) = row else {
        return Ok(0);
    };
    let stored_version = row
        .try_get::<String, _>("version")
        .map_err(|err| format!("Failed to decode {name} backfill mark version: {err}"))?;
    if stored_version != version {
        return Ok(0);
    }
    row.try_get::<i64, _>("last_id")
        .map_err(|err| format!("Failed to decode {name} backfill mark id: {err}"))
}

pub(crate) async fn write(
    pool: &SqlitePool,
    name: &str,
    version: &str,
    last_id: i64,
) -> Result<(), String> {
    sqlx::query(
        r#"
INSERT INTO request_log_backfill_marks (name, version, last_id, updated_at_ms)
VALUES (?, ?, ?, ?)
ON CONFLICT(name) DO UPDATE SET
  version = excluded.version,
  last_id = excluded.last_id,
  updated_at_ms = excluded.updated_at_ms;
"#,
    )
    .bind(name)
    .bind(version)
    .bind(last_id)
    .bind(super::current_time_ms())
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to write {name} backfill mark: {err}"))?;
    Ok(())
}

/// 回填开始时的日志上界；之后写入的日志留给下一次回填。
pub(crate) async fn max_request_log_id(pool: &SqlitePool) -> Result<i64, String> {
    sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(id), 0) FROM request_logs;")
        .fetch_one(pool)
        .await
        .map_err(|err| format!("Failed to read max request log id: {err}"))
}
