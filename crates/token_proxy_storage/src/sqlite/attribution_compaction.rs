//! 把升级前写入的 `usage_json.attribution.items` 压缩为按类型汇总。
//!
//! 新日志在落库前已压缩；这里只处理历史数据，按 id 窗口分批改写，完成后写进度标记，
//! 之后每次启动只检查新增日志。

use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::time::Instant;

use super::backfill_mark;
use crate::usage::compact_attribution_items;

/// 单行 usage_json 最大可达数百 KB；窗口取小一些，限制单批内存与写连接占用时间。
const WINDOW: i64 = 200;
const MARK_NAME: &str = "request_log_attribution";
/// 压缩规则变化时递增，让历史日志重新检查。
const MARK_VERSION: &str = "attribution.v1";

pub async fn compact_request_log_attribution(pool: &SqlitePool) -> Result<(), String> {
    let end_id = backfill_mark::max_request_log_id(pool).await?;
    let mut window_start = backfill_mark::read(pool, MARK_NAME, MARK_VERSION).await?;
    if window_start >= end_id {
        return Ok(());
    }
    let started_at = Instant::now();
    let mut compacted_rows = 0_u64;
    let mut bytes_before = 0_u64;
    let mut bytes_after = 0_u64;

    while window_start < end_id {
        let window_end = window_start.saturating_add(WINDOW).min(end_id);
        let rows = sqlx::query(
            r#"
SELECT id, usage_json
FROM request_logs
WHERE id > ? AND id <= ? AND instr(usage_json, '"attribution"') > 0;
"#,
        )
        .bind(window_start)
        .bind(window_end)
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to read request log attribution: {err}"))?;
        window_start = window_end;

        let mut updates = Vec::new();
        for row in rows {
            let id = row
                .try_get::<i64, _>("id")
                .map_err(|err| format!("Failed to decode request_logs.id: {err}"))?;
            let raw = row
                .try_get::<String, _>("usage_json")
                .map_err(|err| format!("Failed to decode request_logs.usage_json: {err}"))?;
            // 解析失败或结构无法识别的行保持原样。
            let Ok(mut usage) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if !compact_attribution_items(&mut usage) {
                continue;
            }
            let compacted = usage.to_string();
            bytes_before = bytes_before.saturating_add(raw.len() as u64);
            bytes_after = bytes_after.saturating_add(compacted.len() as u64);
            updates.push((id, compacted));
        }
        if updates.is_empty() {
            continue;
        }

        let mut tx = pool
            .begin()
            .await
            .map_err(|err| format!("Failed to begin attribution compaction: {err}"))?;
        for (id, usage_json) in &updates {
            sqlx::query("UPDATE request_logs SET usage_json = ? WHERE id = ?;")
                .bind(usage_json)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| format!("Failed to compact attribution for {id}: {err}"))?;
        }
        tx.commit()
            .await
            .map_err(|err| format!("Failed to commit attribution compaction: {err}"))?;
        compacted_rows = compacted_rows.saturating_add(updates.len() as u64);
    }

    backfill_mark::write(pool, MARK_NAME, MARK_VERSION, end_id).await?;
    if compacted_rows > 0 {
        tracing::info!(
            compacted_rows,
            bytes_before,
            bytes_after,
            elapsed_ms = started_at.elapsed().as_millis(),
            "request log attribution compaction completed"
        );
    }
    Ok(())
}
