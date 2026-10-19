use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use super::backfill_mark;

/// 每批扫描的 id 窗口；按窗口而非按命中行分批，单条查询占用写连接的时间有上界。
const BATCH_SIZE: i64 = 2_000;
const MARK_NAME: &str = "request_log_usage";
/// 解析规则变化时递增，让仍缺规范用量列的历史日志重新回填。
const MARK_VERSION: &str = "usage.v1";

struct UsageBackfillRow {
    id: i64,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    uncached_input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cache_write_5m_tokens: i64,
    cache_write_1h_tokens: i64,
    image_input_tokens: i64,
    image_output_tokens: i64,
    service_tier: Option<String>,
}

// 历史 usage_json 是原始用量事实；按批次重建规范分量，避免一次加载全部日志。
pub(super) async fn backfill_request_log_usage(pool: &SqlitePool) -> Result<(), String> {
    let end_id = backfill_mark::max_request_log_id(pool).await?;
    let mut window_start = backfill_mark::read(pool, MARK_NAME, MARK_VERSION).await?;
    if window_start >= end_id {
        return Ok(());
    }
    create_staging_table(pool).await?;
    let mut migrated_rows = 0_u64;
    let mut skipped_rows = 0_u64;

    while window_start < end_id {
        let window_end = window_start.saturating_add(BATCH_SIZE).min(end_id);
        let rows = read_batch(pool, window_start, window_end).await?;
        window_start = window_end;

        let mut updates = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row
                .try_get::<i64, _>("id")
                .map_err(|err| format!("Failed to decode request_logs.id: {err}"))?;
            let raw = row
                .try_get::<String, _>("usage_json")
                .map_err(|err| format!("Failed to decode request_logs.usage_json: {err}"))?;
            let Some(update) = parse_update(id, &raw) else {
                skipped_rows = skipped_rows.saturating_add(1);
                continue;
            };
            updates.push(update);
        }
        if updates.is_empty() {
            continue;
        }

        merge_batch(pool, &updates).await?;
        migrated_rows = migrated_rows.saturating_add(updates.len() as u64);
    }

    sqlx::query("DROP TABLE IF EXISTS request_log_usage_backfill;")
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to drop request log usage backfill table: {err}"))?;
    backfill_mark::write(pool, MARK_NAME, MARK_VERSION, end_id).await?;

    if migrated_rows > 0 || skipped_rows > 0 {
        tracing::info!(
            migrated_rows,
            skipped_rows,
            "request log usage backfill completed"
        );
    }
    Ok(())
}

async fn create_staging_table(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TEMP TABLE IF NOT EXISTS request_log_usage_backfill (
  id INTEGER PRIMARY KEY,
  input_tokens INTEGER,
  output_tokens INTEGER,
  total_tokens INTEGER,
  uncached_input_tokens INTEGER NOT NULL,
  cache_read_tokens INTEGER NOT NULL,
  cache_write_tokens INTEGER NOT NULL,
  cache_write_5m_tokens INTEGER NOT NULL,
  cache_write_1h_tokens INTEGER NOT NULL,
  image_input_tokens INTEGER NOT NULL,
  image_output_tokens INTEGER NOT NULL,
  service_tier TEXT
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create request log usage backfill table: {err}"))?;
    Ok(())
}

async fn read_batch(
    pool: &SqlitePool,
    after_id: i64,
    through_id: i64,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    sqlx::query(
        r#"
SELECT id, usage_json
FROM request_logs
WHERE id > ?
  AND id <= ?
  AND usage_json IS NOT NULL
  AND usage_json != 'null'
  AND uncached_input_tokens IS NULL
  AND cache_read_tokens IS NULL
  AND cache_write_tokens IS NULL
  AND cache_write_5m_tokens IS NULL
  AND cache_write_1h_tokens IS NULL
  AND image_input_tokens IS NULL
  AND image_output_tokens IS NULL
ORDER BY id ASC;
"#,
    )
    .bind(after_id)
    .bind(through_id)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to read request log usage for backfill: {err}"))
}

fn parse_update(id: i64, raw: &str) -> Option<UsageBackfillRow> {
    let snapshot = super::super::usage::extract_usage_from_stored_json(raw)?;
    let usage = snapshot.usage?;
    let billable = snapshot.billable_usage;
    Some(UsageBackfillRow {
        id,
        input_tokens: usage.input_tokens.map(u64_to_i64),
        output_tokens: usage.output_tokens.map(u64_to_i64),
        total_tokens: usage.total_tokens.map(u64_to_i64),
        uncached_input_tokens: u64_to_i64(billable.uncached_input_tokens),
        cache_read_tokens: u64_to_i64(billable.cache_read_tokens),
        cache_write_tokens: u64_to_i64(billable.cache_write_tokens),
        cache_write_5m_tokens: u64_to_i64(billable.cache_write_5m_tokens),
        cache_write_1h_tokens: u64_to_i64(billable.cache_write_1h_tokens),
        image_input_tokens: u64_to_i64(billable.image_input_tokens),
        image_output_tokens: u64_to_i64(billable.image_output_tokens),
        service_tier: snapshot.service_tier,
    })
}

async fn merge_batch(pool: &SqlitePool, updates: &[UsageBackfillRow]) -> Result<(), String> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| format!("Failed to begin request log usage backfill transaction: {err}"))?;
    sqlx::query("DELETE FROM request_log_usage_backfill;")
        .execute(&mut *tx)
        .await
        .map_err(|err| format!("Failed to clear request log usage backfill table: {err}"))?;

    let mut insert = QueryBuilder::<Sqlite>::new(
        r#"
INSERT INTO request_log_usage_backfill (
  id, input_tokens, output_tokens, total_tokens,
  uncached_input_tokens, cache_read_tokens, cache_write_tokens,
  cache_write_5m_tokens, cache_write_1h_tokens,
  image_input_tokens, image_output_tokens, service_tier
) "#,
    );
    insert.push_values(updates, |mut row, update| {
        row.push_bind(update.id)
            .push_bind(update.input_tokens)
            .push_bind(update.output_tokens)
            .push_bind(update.total_tokens)
            .push_bind(update.uncached_input_tokens)
            .push_bind(update.cache_read_tokens)
            .push_bind(update.cache_write_tokens)
            .push_bind(update.cache_write_5m_tokens)
            .push_bind(update.cache_write_1h_tokens)
            .push_bind(update.image_input_tokens)
            .push_bind(update.image_output_tokens)
            .push_bind(update.service_tier.as_deref());
    });
    insert
        .build()
        .execute(&mut *tx)
        .await
        .map_err(|err| format!("Failed to stage request log usage backfill: {err}"))?;

    sqlx::query(
        r#"
UPDATE request_logs
SET (
  input_tokens, output_tokens, total_tokens,
  uncached_input_tokens, cache_read_tokens, cache_write_tokens,
  cache_write_5m_tokens, cache_write_1h_tokens,
  image_input_tokens, image_output_tokens, service_tier
) = (
  SELECT
    source.input_tokens, source.output_tokens, source.total_tokens,
    source.uncached_input_tokens, source.cache_read_tokens, source.cache_write_tokens,
    source.cache_write_5m_tokens, source.cache_write_1h_tokens,
    source.image_input_tokens, source.image_output_tokens, source.service_tier
  FROM request_log_usage_backfill AS source
  WHERE source.id = request_logs.id
)
WHERE id IN (SELECT id FROM request_log_usage_backfill);
"#,
    )
    .execute(&mut *tx)
    .await
    .map_err(|err| format!("Failed to merge request log usage backfill: {err}"))?;
    tx.commit()
        .await
        .map_err(|err| format!("Failed to commit request log usage backfill: {err}"))
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
