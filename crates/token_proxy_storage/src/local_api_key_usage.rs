//! 按本地 API Key 汇总的累计用量。
//!
//! 只统计计费行：同一客户端请求的多次 attempt 只有最后一次 `is_billable = 1`，
//! 否则重试会重复计入。升级前写入的日志没有 Key ID，不参与统计。

use std::time::Instant;

use serde::Serialize;
use sqlx::{Row, SqlitePool};

/// 条件与部分覆盖索引 `idx_request_logs_local_api_key` 的 WHERE 一致，聚合只扫描索引。
const USAGE_BY_KEY: &str = r#"
SELECT
  local_api_key_id,
  COUNT(*) AS requests,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(cost_nano_usd, 0)), 0) AS cost_nano_usd,
  MAX(ts_ms) AS last_used_ms
FROM billable_request_logs
WHERE local_api_key_id IS NOT NULL
GROUP BY local_api_key_id;
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiKeyUsage {
    pub key_id: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub cost_nano_usd: u64,
    pub last_used_ms: u64,
}

pub async fn read_local_api_key_usage(pool: &SqlitePool) -> Result<Vec<LocalApiKeyUsage>, String> {
    let started_at = Instant::now();
    let usage = sqlx::query(USAGE_BY_KEY)
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to query local api key usage: {err}"))?
        .into_iter()
        .map(|row| {
            Ok(LocalApiKeyUsage {
                key_id: row.try_get("local_api_key_id")?,
                requests: to_u64(row.try_get("requests")?),
                total_tokens: to_u64(row.try_get("total_tokens")?),
                cost_nano_usd: to_u64(row.try_get("cost_nano_usd")?),
                last_used_ms: to_u64(row.try_get("last_used_ms")?),
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|err| format!("Failed to decode local api key usage: {err}"))?;
    tracing::debug!(
        keys = usage.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "local api key usage loaded"
    );
    Ok(usage)
}

fn to_u64(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("Failed to create in-memory database");
        crate::sqlite::init_schema(&pool)
            .await
            .expect("Failed to initialize schema");
        pool
    }

    async fn insert_log(
        pool: &SqlitePool,
        ts_ms: i64,
        key_id: Option<&str>,
        total_tokens: Option<i64>,
        cost_nano_usd: i64,
        is_billable: bool,
    ) {
        sqlx::query(
            r#"
INSERT INTO request_logs (
  ts_ms, path, provider, upstream_id, stream, status, input_tokens, output_tokens,
  total_tokens, cost_nano_usd, latency_ms, is_billable, local_api_key_id
) VALUES (?, '/v1/responses', 'openai-response', 'test', 0, 200, 3, 4, ?, ?, 10, ?, ?);
"#,
        )
        .bind(ts_ms)
        .bind(total_tokens)
        .bind(cost_nano_usd)
        .bind(i64::from(is_billable))
        .bind(key_id)
        .execute(pool)
        .await
        .expect("insert log");
    }

    #[tokio::test]
    async fn sums_billable_rows_per_key() {
        let pool = setup_test_db().await;
        insert_log(&pool, 1_000, Some("a"), Some(100), 5, true).await;
        insert_log(&pool, 3_000, Some("a"), None, 7, true).await;
        // 被后续 attempt 取代的重试行不能重复计入。
        insert_log(&pool, 2_000, Some("a"), Some(999), 999, false).await;
        insert_log(&pool, 4_000, Some("b"), Some(50), 1, true).await;
        // 未启用本地鉴权时没有 Key ID。
        insert_log(&pool, 5_000, None, Some(10), 1, true).await;

        let mut usage = read_local_api_key_usage(&pool).await.expect("usage");
        usage.sort_by(|left, right| left.key_id.cmp(&right.key_id));

        assert_eq!(
            usage,
            vec![
                LocalApiKeyUsage {
                    key_id: "a".into(),
                    requests: 2,
                    // total_tokens 为空时回退 input + output，与 Dashboard 一致。
                    total_tokens: 107,
                    cost_nano_usd: 12,
                    last_used_ms: 3_000,
                },
                LocalApiKeyUsage {
                    key_id: "b".into(),
                    requests: 1,
                    total_tokens: 50,
                    cost_nano_usd: 1,
                    last_used_ms: 4_000,
                },
            ]
        );
    }

    #[tokio::test]
    async fn usage_query_only_reads_covering_index() {
        let pool = setup_test_db().await;
        let details = sqlx::query(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {USAGE_BY_KEY}"
        )))
        .fetch_all(&pool)
        .await
        .expect("explain")
        .into_iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>();
        let table_steps = details
            .iter()
            .filter(|detail| detail.contains("request_logs"))
            .collect::<Vec<_>>();
        assert!(!table_steps.is_empty(), "plan: {details:?}");
        for detail in table_steps {
            assert!(
                detail.contains("COVERING INDEX idx_request_logs_local_api_key"),
                "usage query must only read idx_request_logs_local_api_key, got {details:?}"
            );
        }
    }
}
