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
        model: row.try_get::<Option<String>, _>("model").ok().flatten(),
        mapped_model: row.try_get::<Option<String>, _>("mapped_model").ok().flatten(),
        stream: row.try_get::<i32, _>("stream").unwrap_or_default() != 0,
        status: row.try_get::<i32, _>("status").unwrap_or_default(),
        input_tokens: row.try_get::<Option<i64>, _>("input_tokens").ok().flatten(),
        output_tokens: row.try_get::<Option<i64>, _>("output_tokens").ok().flatten(),
        total_tokens: row.try_get::<Option<i64>, _>("total_tokens").ok().flatten(),
        cached_tokens: row.try_get::<Option<i64>, _>("cached_tokens").ok().flatten(),
        latency_ms: row.try_get::<i64, _>("latency_ms").unwrap_or_default(),
        upstream_request_id: row.try_get::<Option<String>, _>("upstream_request_id").ok().flatten(),
        usage_json: row.try_get::<Option<String>, _>("usage_json").ok().flatten(),
        request_headers: row.try_get::<Option<String>, _>("request_headers").ok().flatten(),
        request_body: row.try_get::<Option<String>, _>("request_body").ok().flatten(),
        response_error: row.try_get::<Option<String>, _>("response_error").ok().flatten(),
    })
}
