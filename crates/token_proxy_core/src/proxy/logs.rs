use serde::Serialize;
use sqlx::Row;

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
    pub cost_nano_usd: Option<i64>,
    pub pricing_version: Option<String>,
    pub pricing_model: Option<String>,
    pub pricing_context_tier: Option<String>,
    pub latency_ms: i64,
    pub upstream_first_byte_ms: Option<i64>,
    pub upstream_response_headers_ms: Option<i64>,
    pub upstream_first_body_chunk_ms: Option<i64>,
    pub first_client_flush_ms: Option<i64>,
    pub first_output_ms: Option<i64>,
    pub upstream_request_id: Option<String>,
    // 详情扩展字段
    pub usage_json: Option<String>,
    pub request_headers: Option<String>,
    pub request_body: Option<String>,
    pub response_error: Option<String>,
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
  cost_nano_usd,
  pricing_version,
  pricing_model,
  pricing_context_tier,
  latency_ms,
  upstream_first_byte_ms,
  upstream_response_headers_ms,
  COALESCE(upstream_first_body_chunk_ms, upstream_first_byte_ms) AS upstream_first_body_chunk_ms,
  first_client_flush_ms,
  first_output_ms,
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
        account_id: row
            .try_get::<Option<String>, _>("account_id")
            .ok()
            .flatten(),
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
        cost_nano_usd: row
            .try_get::<Option<i64>, _>("cost_nano_usd")
            .ok()
            .flatten(),
        pricing_version: row
            .try_get::<Option<String>, _>("pricing_version")
            .ok()
            .flatten(),
        pricing_model: row
            .try_get::<Option<String>, _>("pricing_model")
            .ok()
            .flatten(),
        pricing_context_tier: row
            .try_get::<Option<String>, _>("pricing_context_tier")
            .ok()
            .flatten(),
        latency_ms: row.try_get::<i64, _>("latency_ms").unwrap_or_default(),
        upstream_first_byte_ms: row
            .try_get::<Option<i64>, _>("upstream_first_byte_ms")
            .ok()
            .flatten(),
        upstream_response_headers_ms: row
            .try_get::<Option<i64>, _>("upstream_response_headers_ms")
            .ok()
            .flatten(),
        upstream_first_body_chunk_ms: row
            .try_get::<Option<i64>, _>("upstream_first_body_chunk_ms")
            .ok()
            .flatten(),
        first_client_flush_ms: row
            .try_get::<Option<i64>, _>("first_client_flush_ms")
            .ok()
            .flatten(),
        first_output_ms: row
            .try_get::<Option<i64>, _>("first_output_ms")
            .ok()
            .flatten(),
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
              cost_nano_usd,
              pricing_version,
              pricing_model,
              pricing_context_tier,
              latency_ms
            ) VALUES (
              123,
              '/responses',
              'codex',
              'codex-default',
              'codex-a.json',
              0,
              200,
              1210000000,
              '2026-05-02.openai-openrouter-v1',
              'gpt-5.5',
              'short',
              30
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("insert request log");

        let detail = read_request_log_detail(&pool, 1)
            .await
            .expect("read request log detail");

        assert_eq!(detail.account_id.as_deref(), Some("codex-a.json"));
        assert_eq!(detail.cost_nano_usd, Some(1_210_000_000));
        assert_eq!(
            detail.pricing_version.as_deref(),
            Some("2026-05-02.openai-openrouter-v1")
        );
        assert_eq!(detail.pricing_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(detail.pricing_context_tier.as_deref(), Some("short"));
    }
}
