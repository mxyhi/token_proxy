use serde::Serialize;
use serde_json::Value;
use sqlx::Row;

/// 请求日志详情，包含表格展示的基础字段和详情面板的扩展字段
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogDetail {
    pub id: u64,
    // 基础字段（与表格一致）
    pub ts_ms: i64,
    pub client_ip: Option<String>,
    pub path: String,
    pub provider: String,
    pub upstream_id: String,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub mapped_model: Option<String>,
    pub upstream_response_model: Option<String>,
    pub stream: bool,
    pub status: i32,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub uncached_input_tokens: Option<i64>,
    pub image_input_tokens: Option<i64>,
    pub image_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_write_5m_tokens: Option<i64>,
    pub cache_write_1h_tokens: Option<i64>,
    pub service_tier: Option<String>,
    pub cost_nano_usd: Option<i64>,
    pub pricing_version: Option<String>,
    pub pricing_model: Option<String>,
    pub pricing_context_tier: Option<String>,
    /// 同一客户端请求的上游 attempt 共享该标识，便于审计 failover。
    pub client_request_id: Option<String>,
    pub attempt_index: Option<i64>,
    pub is_billable: bool,
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
    pub response_body: Option<String>,
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
  client_ip,
  path,
  provider,
  upstream_id,
  account_id,
  model,
  mapped_model,
  upstream_response_model,
  stream,
  status,
  input_tokens,
  output_tokens,
  uncached_input_tokens,
  image_input_tokens,
  image_output_tokens,
  total_tokens,
  cache_read_tokens,
  cache_write_tokens,
  cache_write_5m_tokens,
  cache_write_1h_tokens,
  service_tier,
  cost_nano_usd,
  pricing_version,
  pricing_model,
  pricing_context_tier,
  client_request_id,
  attempt_index,
  is_billable,
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
  response_body,
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

    let usage_json = row
        .try_get::<Option<String>, _>("usage_json")
        .ok()
        .flatten();
    let image_output_tokens = row
        .try_get::<Option<i64>, _>("image_output_tokens")
        .ok()
        .flatten()
        .or_else(|| {
            usage_json
                .as_deref()
                .and_then(image_output_tokens_from_usage_json)
        });

    let cache_read_tokens = row
        .try_get::<Option<i64>, _>("cache_read_tokens")
        .ok()
        .flatten();
    let cache_write_tokens = row
        .try_get::<Option<i64>, _>("cache_write_tokens")
        .ok()
        .flatten();
    let cache_write_5m_tokens = row
        .try_get::<Option<i64>, _>("cache_write_5m_tokens")
        .ok()
        .flatten();
    let cache_write_1h_tokens = row
        .try_get::<Option<i64>, _>("cache_write_1h_tokens")
        .ok()
        .flatten();
    let cache_components = [
        cache_read_tokens,
        cache_write_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
    ];
    // 历史行没有缓存分量时保留 null；显式记录的零值仍返回 Some(0)。
    let cached_tokens = cache_components
        .iter()
        .any(Option::is_some)
        .then(|| cache_components.iter().flatten().copied().sum());

    Ok(RequestLogDetail {
        id: row.try_get::<i64, _>("id").unwrap_or_default().max(0) as u64,
        ts_ms: row.try_get::<i64, _>("ts_ms").unwrap_or_default(),
        client_ip: row.try_get::<Option<String>, _>("client_ip").ok().flatten(),
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
        upstream_response_model: row
            .try_get::<Option<String>, _>("upstream_response_model")
            .ok()
            .flatten(),
        stream: row.try_get::<i32, _>("stream").unwrap_or_default() != 0,
        status: row.try_get::<i32, _>("status").unwrap_or_default(),
        input_tokens: row.try_get::<Option<i64>, _>("input_tokens").ok().flatten(),
        output_tokens: row
            .try_get::<Option<i64>, _>("output_tokens")
            .ok()
            .flatten(),
        uncached_input_tokens: row
            .try_get::<Option<i64>, _>("uncached_input_tokens")
            .ok()
            .flatten(),
        image_input_tokens: row
            .try_get::<Option<i64>, _>("image_input_tokens")
            .ok()
            .flatten(),
        image_output_tokens,
        total_tokens: row.try_get::<Option<i64>, _>("total_tokens").ok().flatten(),
        cached_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
        service_tier: row
            .try_get::<Option<String>, _>("service_tier")
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
        client_request_id: row
            .try_get::<Option<String>, _>("client_request_id")
            .ok()
            .flatten(),
        attempt_index: row
            .try_get::<Option<i64>, _>("attempt_index")
            .ok()
            .flatten(),
        is_billable: row.try_get::<i64, _>("is_billable").unwrap_or(1) != 0,
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
        usage_json,
        request_headers: row
            .try_get::<Option<String>, _>("request_headers")
            .ok()
            .flatten(),
        request_body: row
            .try_get::<Option<String>, _>("request_body")
            .ok()
            .flatten(),
        response_body: row
            .try_get::<Option<String>, _>("response_body")
            .ok()
            .flatten(),
        response_error: row
            .try_get::<Option<String>, _>("response_error")
            .ok()
            .flatten(),
    })
}

fn image_output_tokens_from_usage_json(raw: &str) -> Option<i64> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let usage = value
        .get("usage")
        .filter(|candidate| candidate.is_object())
        .unwrap_or(&value);
    image_output_tokens_from_value(usage).or_else(|| image_output_tokens_from_value(&value))
}

fn image_output_tokens_from_value(value: &Value) -> Option<i64> {
    ["output_tokens_details", "completion_tokens_details"]
        .iter()
        .find_map(|details_key| {
            value
                .get(*details_key)
                .and_then(|details| details.get("image_tokens"))
                .and_then(json_integer_to_i64)
        })
}

fn json_integer_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
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

        crate::sqlite::init_schema(&pool)
            .await
            .expect("init schema");

        sqlx::query(
            r#"
            INSERT INTO request_logs (
              ts_ms,
              client_ip,
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
              '198.51.100.8',
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

        assert_eq!(detail.client_ip.as_deref(), Some("198.51.100.8"));
        assert_eq!(detail.account_id.as_deref(), Some("codex-a.json"));
        assert_eq!(detail.cost_nano_usd, Some(1_210_000_000));
        assert_eq!(
            detail.pricing_version.as_deref(),
            Some("2026-05-02.openai-openrouter-v1")
        );
        assert_eq!(detail.pricing_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(detail.pricing_context_tier.as_deref(), Some("short"));
        assert_eq!(detail.cached_tokens, None);
    }

    #[tokio::test]
    async fn read_request_log_detail_round_trips_precise_usage_components() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        crate::sqlite::init_schema(&pool)
            .await
            .expect("init schema");

        sqlx::query(
            r#"
            INSERT INTO request_logs (
              ts_ms, path, provider, upstream_id, stream, status,
              input_tokens, output_tokens, uncached_input_tokens,
              cache_read_tokens, cache_write_tokens,
              cache_write_5m_tokens, cache_write_1h_tokens,
              image_input_tokens, image_output_tokens, total_tokens,
              service_tier, latency_ms
            ) VALUES (
              123, '/responses', 'openai-response', 'airouter', 0, 200,
              121, 13, 80,
              20, 5,
              10, 6,
              4, 3, 134,
              'priority', 30
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("insert request log");

        let detail = read_request_log_detail(&pool, 1)
            .await
            .expect("read request log detail");

        assert_eq!(detail.uncached_input_tokens, Some(80));
        assert_eq!(detail.cache_read_tokens, Some(20));
        assert_eq!(detail.cache_write_tokens, Some(5));
        assert_eq!(detail.cache_write_5m_tokens, Some(10));
        assert_eq!(detail.cache_write_1h_tokens, Some(6));
        assert_eq!(detail.cached_tokens, Some(41));
        assert_eq!(detail.image_input_tokens, Some(4));
        assert_eq!(detail.image_output_tokens, Some(3));
        assert_eq!(detail.service_tier.as_deref(), Some("priority"));
    }

    #[tokio::test]
    async fn read_request_log_detail_extracts_image_output_tokens_from_usage_json() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        crate::sqlite::init_schema(&pool)
            .await
            .expect("init schema");

        sqlx::query(
            r#"
            INSERT INTO request_logs (
              ts_ms,
              path,
              provider,
              upstream_id,
              stream,
              status,
              output_tokens,
              usage_json,
              latency_ms
            ) VALUES (
              123,
              '/responses',
              'codex',
              'codex-default',
              0,
              200,
              9,
              '{"input_tokens":5,"output_tokens":9,"output_tokens_details":{"image_tokens":9}}',
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

        assert_eq!(detail.output_tokens, Some(9));
        assert_eq!(detail.image_output_tokens, Some(9));
    }

    #[test]
    fn image_output_tokens_falls_back_to_chat_completion_details() {
        let image_tokens = image_output_tokens_from_usage_json(
            r#"{"completion_tokens":9,"completion_tokens_details":{"image_tokens":4}}"#,
        );

        assert_eq!(image_tokens, Some(4));
    }
}
