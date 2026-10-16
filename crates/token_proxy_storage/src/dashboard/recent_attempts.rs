//! 最近请求页的中间 attempt。
//!
//! 最近请求按最终计费行分页；同一 `client_request_id` 下被重试或 failover 取代的
//! attempt（`is_billable = 0`）只挂在对应最终请求下供排障展示，不参与分页与统计。

use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;

use super::{i64_to_u16, i64_to_u64};

/// 错误摘要上限（字符）。完整错误在详情面板按 id 读取。
const ATTEMPT_ERROR_PREVIEW_CHARS: i64 = 500;

/// 按 `client_request_id` 走 `idx_request_logs_client_attempt`；
/// 参数 ?1 为 JSON 字符串数组，避免拼接动态占位符。
const RECENT_ATTEMPTS: &str = r#"
SELECT
  id,
  ts_ms,
  client_request_id,
  attempt_index,
  provider,
  upstream_id,
  account_id,
  model,
  mapped_model,
  status,
  latency_ms,
  upstream_response_headers_ms,
  substr(response_error, 1, ?2) AS response_error
FROM request_logs
WHERE client_request_id IN (SELECT value FROM json_each(?1))
  AND is_billable = 0
ORDER BY client_request_id, attempt_index DESC, id DESC;
"#;

/// 最终请求之前失败或被取代的上游 attempt，按 attempt 序号倒序排列。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRequestAttempt {
    pub id: u64,
    pub ts_ms: u64,
    pub attempt_index: Option<u64>,
    pub provider: String,
    pub upstream_id: String,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub mapped_model: Option<String>,
    pub status: u16,
    pub latency_ms: u64,
    pub upstream_response_headers_ms: Option<u64>,
    /// 截断后的错误摘要。
    pub response_error: Option<String>,
}

/// 读取给定客户端请求的中间 attempt，按 `client_request_id` 分组。
pub(super) async fn query_recent_attempts(
    pool: &sqlx::SqlitePool,
    client_request_ids: &[&str],
) -> Result<HashMap<String, Vec<DashboardRequestAttempt>>, String> {
    let mut grouped: HashMap<String, Vec<DashboardRequestAttempt>> = HashMap::new();
    if client_request_ids.is_empty() {
        return Ok(grouped);
    }
    let ids_json = serde_json::to_string(client_request_ids)
        .map_err(|err| format!("Failed to encode client request ids: {err}"))?;
    let rows = sqlx::query(RECENT_ATTEMPTS)
        .bind(ids_json)
        .bind(ATTEMPT_ERROR_PREVIEW_CHARS)
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to query recent request attempts: {err}"))?;

    for row in rows {
        let Ok(client_request_id) = row.try_get::<String, _>("client_request_id") else {
            continue;
        };
        let attempt = DashboardRequestAttempt {
            id: i64_to_u64(row.try_get("id").unwrap_or(0)),
            ts_ms: i64_to_u64(row.try_get("ts_ms").unwrap_or(0)),
            attempt_index: row
                .try_get::<Option<i64>, _>("attempt_index")
                .ok()
                .flatten()
                .map(i64_to_u64),
            provider: row.try_get("provider").unwrap_or_default(),
            upstream_id: row.try_get("upstream_id").unwrap_or_default(),
            account_id: row.try_get("account_id").ok().flatten(),
            model: row.try_get("model").ok().flatten(),
            mapped_model: row.try_get("mapped_model").ok().flatten(),
            status: i64_to_u16(row.try_get("status").unwrap_or(0)),
            latency_ms: i64_to_u64(row.try_get("latency_ms").unwrap_or(0)),
            upstream_response_headers_ms: row
                .try_get::<Option<i64>, _>("upstream_response_headers_ms")
                .ok()
                .flatten()
                .map(i64_to_u64),
            response_error: row.try_get("response_error").ok().flatten(),
        };
        grouped.entry(client_request_id).or_default().push(attempt);
    }
    Ok(grouped)
}

#[cfg(test)]
pub(super) const PLAN_SQL: &str = RECENT_ATTEMPTS;
