use crate::paths::TokenProxyPaths;
use serde::Serialize;
use sqlx::Row;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStateLogEntry {
    pub ts_ms: u128,
    pub provider: String,
    pub account_id: String,
    pub event_kind: String,
    pub trigger_kind: String,
    pub status: String,
    pub reason_detail: Option<String>,
    pub cooldown_until_ms: Option<u128>,
}

/// 请求日志详情，包含表格展示的基础字段和详情面板的扩展字段
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogDetail {
    pub id: u64,
    // 基础字段（与表格一致）
    pub ts_ms: i64,
    pub path: String,
    pub provider: String,
    pub upstream_id: String,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub mapped_model: Option<String>,
    pub stream: bool,
    pub status: i32,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub latency_ms: i64,
    pub upstream_request_id: Option<String>,
    // 详情扩展字段
    pub usage_json: Option<String>,
    pub request_headers: Option<String>,
    pub request_body: Option<String>,
    pub response_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStateLogItem {
    pub id: u64,
    pub ts_ms: i64,
    pub provider: String,
    pub account_id: String,
    pub event_kind: String,
    pub trigger_kind: String,
    pub status: String,
    pub reason_detail: Option<String>,
    pub cooldown_until_ms: Option<i64>,
}

pub fn build_account_state_log_entry(
    provider: &str,
    account_id: &str,
    event_kind: &str,
    trigger_kind: &str,
    status: &str,
    reason_detail: Option<String>,
    cooldown_until_ms: Option<u128>,
) -> AccountStateLogEntry {
    AccountStateLogEntry {
        ts_ms: now_ms(),
        provider: provider.to_string(),
        account_id: account_id.to_string(),
        event_kind: event_kind.to_string(),
        trigger_kind: trigger_kind.to_string(),
        status: status.to_string(),
        reason_detail,
        cooldown_until_ms,
    }
}

pub async fn read_request_log_detail(
    pool: &sqlx::SqlitePool,
    id: u64,
) -> Result<RequestLogDetail, String> {
    let row = sqlx::query(
        r#"
SELECT
  id,
  ts_ms,
  path,
  provider,
  upstream_id,
  account_id,
  model,
  mapped_model,
  stream,
  status,
  input_tokens,
  output_tokens,
  total_tokens,
  cached_tokens,
  latency_ms,
  upstream_request_id,
  usage_json,
  request_headers,
  request_body,
  response_error
FROM request_logs
WHERE id = ?
LIMIT 1;
"#,
    )
    .bind(id as i64)
    .fetch_optional(pool)
    .await
    .map_err(|err| format!("Failed to query request log detail: {err}"))?;

    let Some(row) = row else {
        return Err("Request log not found.".to_string());
    };

    Ok(RequestLogDetail {
        id: row.try_get::<i64, _>("id").unwrap_or_default().max(0) as u64,
        ts_ms: row.try_get::<i64, _>("ts_ms").unwrap_or_default(),
        path: row.try_get::<String, _>("path").unwrap_or_default(),
        provider: row.try_get::<String, _>("provider").unwrap_or_default(),
        upstream_id: row.try_get::<String, _>("upstream_id").unwrap_or_default(),
        account_id: row.try_get::<Option<String>, _>("account_id").ok().flatten(),
        model: row.try_get::<Option<String>, _>("model").ok().flatten(),
        mapped_model: row
            .try_get::<Option<String>, _>("mapped_model")
            .ok()
            .flatten(),
        stream: row.try_get::<i32, _>("stream").unwrap_or_default() != 0,
        status: row.try_get::<i32, _>("status").unwrap_or_default(),
        input_tokens: row.try_get::<Option<i64>, _>("input_tokens").ok().flatten(),
        output_tokens: row
            .try_get::<Option<i64>, _>("output_tokens")
            .ok()
            .flatten(),
        total_tokens: row.try_get::<Option<i64>, _>("total_tokens").ok().flatten(),
        cached_tokens: row
            .try_get::<Option<i64>, _>("cached_tokens")
            .ok()
            .flatten(),
        latency_ms: row.try_get::<i64, _>("latency_ms").unwrap_or_default(),
        upstream_request_id: row
            .try_get::<Option<String>, _>("upstream_request_id")
            .ok()
            .flatten(),
        usage_json: row
            .try_get::<Option<String>, _>("usage_json")
            .ok()
            .flatten(),
        request_headers: row
            .try_get::<Option<String>, _>("request_headers")
            .ok()
            .flatten(),
        request_body: row
            .try_get::<Option<String>, _>("request_body")
            .ok()
            .flatten(),
        response_error: row
            .try_get::<Option<String>, _>("response_error")
            .ok()
            .flatten(),
    })
}

pub async fn read_recent_account_state_logs(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    limit: u32,
) -> Result<Vec<AccountStateLogItem>, String> {
    let items = sqlx::query(
        r#"
SELECT
  id,
  ts_ms,
  provider,
  account_id,
  event_kind,
  trigger_kind,
  status,
  reason_detail,
  cooldown_until_ms
FROM account_state_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
ORDER BY ts_ms DESC, id DESC
LIMIT ?3;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query account state logs: {err}"))?
    .into_iter()
    .map(|row| AccountStateLogItem {
        id: row.try_get::<i64, _>("id").unwrap_or_default().max(0) as u64,
        ts_ms: row.try_get::<i64, _>("ts_ms").unwrap_or_default(),
        provider: row.try_get::<String, _>("provider").unwrap_or_default(),
        account_id: row.try_get::<String, _>("account_id").unwrap_or_default(),
        event_kind: row.try_get::<String, _>("event_kind").unwrap_or_default(),
        trigger_kind: row.try_get::<String, _>("trigger_kind").unwrap_or_default(),
        status: row.try_get::<String, _>("status").unwrap_or_default(),
        reason_detail: row
            .try_get::<Option<String>, _>("reason_detail")
            .ok()
            .flatten(),
        cooldown_until_ms: row
            .try_get::<Option<i64>, _>("cooldown_until_ms")
            .ok()
            .flatten(),
    })
    .collect::<Vec<_>>();

    Ok(items)
}

pub async fn write_account_state_log(
    pool: &sqlx::SqlitePool,
    entry: &AccountStateLogEntry,
) -> Result<(), String> {
    sqlx::query(
        r#"
INSERT INTO account_state_logs (
  ts_ms,
  provider,
  account_id,
  event_kind,
  trigger_kind,
  status,
  reason_detail,
  cooldown_until_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?);
"#,
    )
    .bind(to_i64_u128(entry.ts_ms))
    .bind(entry.provider.as_str())
    .bind(entry.account_id.as_str())
    .bind(entry.event_kind.as_str())
    .bind(entry.trigger_kind.as_str())
    .bind(entry.status.as_str())
    .bind(entry.reason_detail.as_deref())
    .bind(entry.cooldown_until_ms.map(to_i64_u128))
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to insert account state log: {err}"))?;
    Ok(())
}

pub async fn write_account_state_log_for_paths(
    paths: &TokenProxyPaths,
    entry: &AccountStateLogEntry,
) -> Result<(), String> {
    let pool = crate::proxy::sqlite::open_write_pool(paths).await?;
    write_account_state_log(&pool, entry).await
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn to_i64_u128(value: u128) -> i64 {
    value.min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn read_request_log_detail_reads_account_id() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        crate::proxy::sqlite::init_schema(&pool)
            .await
            .expect("init schema");

        sqlx::query(
            r#"
            INSERT INTO request_logs (
              ts_ms,
              path,
              provider,
              upstream_id,
              account_id,
              stream,
              status,
              latency_ms
            ) VALUES (123, '/responses', 'codex', 'codex-default', 'codex-a.json', 0, 200, 30);
            "#,
        )
        .execute(&pool)
        .await
        .expect("insert request log");

        let detail = read_request_log_detail(&pool, 1)
            .await
            .expect("read request log detail");

        assert_eq!(detail.account_id.as_deref(), Some("codex-a.json"));
    }

    #[tokio::test]
    async fn read_recent_account_state_logs_reads_latest_events() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        crate::proxy::sqlite::init_schema(&pool)
            .await
            .expect("init schema");

        sqlx::query(
            r#"
            INSERT INTO account_state_logs (
              ts_ms,
              provider,
              account_id,
              event_kind,
              trigger_kind,
              status,
              reason_detail,
              cooldown_until_ms
            ) VALUES
              (100, 'codex', 'codex-a.json', 'cooldown_started', 'http_status', 'cooling_down', '429 retry-after=30', 130),
              (140, 'codex', 'codex-a.json', 'cooldown_cleared', 'success', 'active', NULL, NULL);
            "#,
        )
        .execute(&pool)
        .await
        .expect("insert account state logs");

        let items = read_recent_account_state_logs(&pool, Some(90), Some(150), 10)
            .await
            .expect("read account state logs");

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].event_kind, "cooldown_cleared");
        assert_eq!(items[0].status, "active");
        assert_eq!(items[1].trigger_kind, "http_status");
        assert_eq!(items[1].reason_detail.as_deref(), Some("429 retry-after=30"));
    }
}
