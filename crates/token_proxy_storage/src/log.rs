use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::Arc;

use super::pricing::{calculate_request_cost, default_model_pricing_settings, BillableUsage};

use crate::body_capture::{CapturedBody, ResponseBodyCapture, BODY_PAGE_BYTES};

#[derive(Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub usage: Option<TokenUsage>,
    pub billable_usage: BillableUsage,
    pub service_tier: Option<String>,
    pub usage_json: Option<Value>,
    /// 上游响应里声明的模型。只用于审计，不参与计价。
    pub response_model: Option<String>,
}

impl UsageSnapshot {
    pub fn from_uncached_usage(usage: Option<TokenUsage>, usage_json: Option<Value>) -> Self {
        let billable_usage = usage
            .as_ref()
            .map(|usage| BillableUsage {
                uncached_input_tokens: usage.input_tokens.unwrap_or(0),
                output_tokens: usage.output_tokens.unwrap_or(0),
                ..BillableUsage::default()
            })
            .unwrap_or_default();
        Self {
            usage,
            billable_usage,
            service_tier: None,
            usage_json,
            response_model: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.usage.is_none()
            && self.billable_usage == BillableUsage::default()
            && self.usage_json.is_none()
            && self.response_model.is_none()
    }
}

#[derive(Serialize, Deserialize)]
pub struct LogEntry {
    pub ts_ms: u128,
    pub client_ip: Option<String>,
    pub path: String,
    pub provider: String,
    pub upstream_id: String,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub mapped_model: Option<String>,
    /// 上游响应体里的 model。空表示这次响应没解析到模型。
    pub upstream_response_model: Option<String>,
    pub stream: bool,
    pub status: u16,
    pub usage: Option<TokenUsage>,
    pub billable_usage: BillableUsage,
    pub service_tier: Option<String>,
    pub usage_json: Option<Value>,
    pub upstream_request_id: Option<String>,
    pub request_headers: Option<String>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub response_error: Option<String>,
    pub latency_ms: u128,
    pub upstream_first_byte_ms: Option<u128>,
    pub upstream_response_headers_ms: Option<u128>,
    pub upstream_first_body_chunk_ms: Option<u128>,
    pub first_client_flush_ms: Option<u128>,
    pub first_output_ms: Option<u128>,
    pub cost_nano_usd: Option<u64>,
    pub pricing_version: String,
    pub pricing_model: Option<String>,
    pub pricing_context_tier: Option<String>,
    pub client_request_id: Option<String>,
    pub attempt_index: Option<u64>,
    /// 鉴权通过的本地 API Key ID；未启用本地鉴权或鉴权前失败时为空。
    pub local_api_key_id: Option<String>,
}

pub struct LogWriter {
    sqlite: Option<SqlitePool>,
}

impl LogWriter {
    pub fn new(sqlite: Option<SqlitePool>) -> Self {
        Self { sqlite }
    }

    // Fire-and-forget logging to avoid blocking the request path.
    pub fn write_detached(self: Arc<Self>, entry: LogEntry) {
        tokio::spawn(async move {
            self.write(&entry).await;
        });
    }

    pub fn write_detached_with_body(self: Arc<Self>, entry: LogEntry, body: ResponseBodyCapture) {
        tokio::spawn(async move {
            if let Err(error) = self.write_with_body(&entry, body).await {
                // release 构建禁用了 tracing，落库失败仍必须可见。
                eprintln!("proxy sqlite response detail write failed: {error}");
            }
        });
    }

    pub async fn write_with_body(
        &self,
        entry: &LogEntry,
        body: ResponseBodyCapture,
    ) -> Result<(), sqlx::Error> {
        let Some(pool) = self.sqlite.as_ref() else {
            return Ok(());
        };
        insert_log_entry_with_body(pool, entry, Some(body.finish().await)).await
    }

    pub async fn write(&self, entry: &LogEntry) {
        let Some(pool) = self.sqlite.as_ref() else {
            return;
        };
        if let Err(_err) = insert_log_entry(pool, entry).await {
            eprintln!("proxy sqlite write failed: {_err}");
        }
    }
}

pub fn attach_response_body(entry: &mut LogEntry, response_body: &str) {
    if !captures_request_detail(&entry.request_headers, &entry.request_body) {
        return;
    }
    if response_body.is_empty() {
        return;
    }
    entry.response_body = Some(response_body.to_string());
}

fn captures_request_detail(
    request_headers: &Option<String>,
    request_body: &Option<String>,
) -> bool {
    request_headers.is_some() || request_body.is_some()
}

async fn insert_log_entry(pool: &SqlitePool, entry: &LogEntry) -> Result<(), sqlx::Error> {
    insert_log_entry_with_body(pool, entry, None).await
}

async fn insert_log_entry_with_body(
    pool: &SqlitePool,
    entry: &LogEntry,
    mut body: Option<CapturedBody>,
) -> Result<(), sqlx::Error> {
    let usage = entry.usage.as_ref();
    let input_tokens = usage.and_then(|usage| usage.input_tokens).map(to_i64_u64);
    let output_tokens = usage.and_then(|usage| usage.output_tokens).map(to_i64_u64);
    let total_tokens = usage.and_then(|usage| usage.total_tokens).map(to_i64_u64);
    let billable = &entry.billable_usage;
    let billable_value = |value| usage.map(|_| to_i64_u64(value));
    let pricing_settings = super::pricing::read_model_pricing_settings(pool)
        .await
        .unwrap_or_else(|_| default_model_pricing_settings());
    let request_cost = calculate_request_cost(
        &pricing_settings,
        entry.model.as_deref(),
        entry.mapped_model.as_deref(),
        entry.service_tier.as_deref(),
        billable,
    );
    let cost_nano_usd = request_cost
        .as_ref()
        .map(|cost| to_i64_u64(cost.cost_nano_usd));
    let pricing_model = request_cost
        .as_ref()
        .map(|cost| cost.pricing_model.as_str());
    let pricing_context_tier = request_cost.as_ref().map(|cost| cost.context_tier.as_str());
    let usage_json = entry.usage_json.as_ref().map(Value::to_string);

    let mut transaction = pool.begin().await?;
    let inserted = sqlx::query(
        r#"
INSERT INTO request_logs (
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
  total_tokens,
  uncached_input_tokens,
  cache_read_tokens,
  cache_write_tokens,
  cache_write_5m_tokens,
  cache_write_1h_tokens,
  image_input_tokens,
  image_output_tokens,
  service_tier,
  usage_json,
  upstream_request_id,
  request_headers,
  request_body,
  response_body,
  response_error,
  latency_ms,
  upstream_first_byte_ms,
  upstream_response_headers_ms,
  upstream_first_body_chunk_ms,
  first_client_flush_ms,
  first_output_ms,
  cost_nano_usd,
  pricing_version,
  pricing_model,
  pricing_context_tier,
  client_request_id,
  attempt_index,
  local_api_key_id,
  is_billable
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1);
"#,
    )
    .bind(to_i64_u128(entry.ts_ms))
    .bind(entry.client_ip.as_deref())
    .bind(entry.path.as_str())
    .bind(entry.provider.as_str())
    .bind(entry.upstream_id.as_str())
    .bind(entry.account_id.as_deref())
    .bind(entry.model.as_deref())
    .bind(entry.mapped_model.as_deref())
    .bind(entry.upstream_response_model.as_deref())
    .bind(entry.stream)
    .bind(i64::from(entry.status))
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(total_tokens)
    .bind(billable_value(billable.uncached_input_tokens))
    .bind(billable_value(billable.cache_read_tokens))
    .bind(billable_value(billable.cache_write_tokens))
    .bind(billable_value(billable.cache_write_5m_tokens))
    .bind(billable_value(billable.cache_write_1h_tokens))
    .bind(billable_value(billable.image_input_tokens))
    .bind(billable_value(billable.image_output_tokens))
    .bind(entry.service_tier.as_deref())
    .bind(usage_json.as_deref())
    .bind(entry.upstream_request_id.as_deref())
    .bind(entry.request_headers.as_deref())
    .bind(entry.request_body.as_deref())
    .bind(entry.response_body.as_deref())
    .bind(entry.response_error.as_deref())
    .bind(to_i64_u128(entry.latency_ms))
    .bind(entry.upstream_first_byte_ms.map(to_i64_u128))
    .bind(entry.upstream_response_headers_ms.map(to_i64_u128))
    .bind(entry.upstream_first_body_chunk_ms.map(to_i64_u128))
    .bind(entry.first_client_flush_ms.map(to_i64_u128))
    .bind(entry.first_output_ms.map(to_i64_u128))
    .bind(cost_nano_usd)
    .bind(pricing_settings.version.as_str())
    .bind(pricing_model)
    .bind(pricing_context_tier)
    .bind(entry.client_request_id.as_deref())
    .bind(entry.attempt_index.map(to_i64_u64))
    .bind(entry.local_api_key_id.as_deref())
    .execute(&mut *transaction)
    .await?;

    if let Some(body) = body.as_mut() {
        let id = inserted.last_insert_rowid();
        // 非 UTF-8 小正文也用 BLOB，避免 lossy 转换改变正文与分页字节偏移。
        let invalid_inline = std::str::from_utf8(&body.memory).is_err();
        let chunked = body.file.is_some() || invalid_inline;
        let mut length = body.memory.len() as u64;
        if invalid_inline {
            body.error.get_or_insert_with(|| "Response detail contains invalid UTF-8; original bytes are preserved but cannot be displayed as text".to_owned());
            for (ordinal, chunk) in body.memory.chunks(BODY_PAGE_BYTES).enumerate() {
                sqlx::query(
                    "INSERT INTO response_body_chunks (log_id, ordinal, data) VALUES (?, ?, ?);",
                )
                .bind(id)
                .bind(ordinal as i64)
                .bind(chunk)
                .execute(&mut *transaction)
                .await?;
            }
        } else if chunked {
            length = 0;
            let mut buffer = vec![0; BODY_PAGE_BYTES];
            let mut ordinal = 0i64;
            loop {
                match body.read_chunk(&mut buffer).await {
                    Ok(0) => break,
                    Ok(count) => {
                        sqlx::query("INSERT INTO response_body_chunks (log_id, ordinal, data) VALUES (?, ?, ?);")
                            .bind(id).bind(ordinal).bind(&buffer[..count])
                            .execute(&mut *transaction).await?;
                        length += count as u64;
                        ordinal += 1;
                    }
                    Err(_) => {
                        body.error.get_or_insert_with(|| {
                            "Failed to read response detail temporary file".to_owned()
                        });
                        break;
                    }
                }
            }
        }
        let text = (!chunked && !body.memory.is_empty())
            .then(|| std::str::from_utf8(&body.memory).expect("inline body validated above"));
        sqlx::query("UPDATE request_logs SET response_body = ?, response_body_bytes = ?, response_body_chunked = ?, response_capture_error = ? WHERE id = ?;")
            .bind(text).bind(to_i64_u64(length)).bind(chunked).bind(body.error.as_deref()).bind(id)
            .execute(&mut *transaction).await?;
    }

    if let Some(client_request_id) = entry.client_request_id.as_deref() {
        // attempt 完成顺序与异步落库顺序都可能不同；按完成序号重算唯一账单记录。
        sqlx::query(
            r#"
UPDATE request_logs
SET is_billable = CASE
  WHEN id = (
    SELECT id
    FROM request_logs
    WHERE client_request_id = ?
    ORDER BY
      attempt_index DESC,
      id DESC
    LIMIT 1
  ) THEN 1
  ELSE 0
END
WHERE client_request_id = ?;
"#,
        )
        .bind(client_request_id)
        .bind(client_request_id)
        .execute(&mut *transaction)
        .await?;
        tracing::debug!(
            client_request_id,
            attempt_index = entry.attempt_index,
            "reconciled client request billing attribution"
        );
    }
    transaction.commit().await?;

    Ok(())
}

fn to_i64_u128(value: u128) -> i64 {
    value.min(i64::MAX as u128) as i64
}

fn to_i64_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pricing::PRICE_MULTIPLIER_SCALE, sqlite};
    use sqlx::{sqlite::SqlitePoolOptions, Row};

    fn sample_entry(status: u16, input_tokens: u64, attempt_index: u64) -> LogEntry {
        LogEntry {
            ts_ms: 1,
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "retry-upstream".to_string(),
            account_id: None,
            model: Some("gpt-5".to_string()),
            mapped_model: None,
            upstream_response_model: None,
            stream: false,
            status,
            usage: Some(TokenUsage {
                input_tokens: Some(input_tokens),
                output_tokens: Some(1),
                total_tokens: Some(input_tokens + 1),
            }),
            billable_usage: BillableUsage {
                uncached_input_tokens: input_tokens,
                output_tokens: 1,
                ..BillableUsage::default()
            },
            service_tier: None,
            usage_json: None,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            response_body: None,
            response_error: None,
            latency_ms: 1,
            upstream_first_byte_ms: None,
            upstream_response_headers_ms: None,
            upstream_first_body_chunk_ms: None,
            first_client_flush_ms: None,
            first_output_ms: None,
            cost_nano_usd: None,
            pricing_version: default_model_pricing_settings().version,
            pricing_model: None,
            pricing_context_tier: None,
            client_request_id: Some("request-1".to_string()),
            attempt_index: Some(attempt_index),
            local_api_key_id: None,
        }
    }

    #[tokio::test]
    async fn invalid_utf8_preserves_original_bytes_and_surfaces_detail_error() {
        use crate::body_capture::ResponseBodyCapture;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlite::init_schema(&pool).await.unwrap();
        let original = [b'a', 0xff, b'z'];
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(&original).await;
        LogWriter::new(Some(pool.clone()))
            .write_with_body(&sample_entry(200, 1, 0), capture)
            .await
            .unwrap();
        let stored: Vec<u8> = sqlx::query_scalar(
            "SELECT data FROM response_body_chunks WHERE log_id = 1 AND ordinal = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored, original);
        let detail = crate::logs::read_request_log_detail(&pool, 1)
            .await
            .unwrap();
        assert_eq!(detail.response_body_bytes, 3);
        assert!(detail.response_body.is_none());
        assert!(detail
            .response_capture_error
            .unwrap()
            .contains("invalid UTF-8"));
        assert!(crate::logs::read_request_log_body_page(&pool, 1, 0)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn missing_short_or_invalid_chunks_are_reported_without_shifting_offsets() {
        use crate::body_capture::{ResponseBodyCapture, BODY_MEMORY_LIMIT, BODY_PAGE_BYTES};
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlite::init_schema(&pool).await.unwrap();
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(&vec![b'a'; BODY_MEMORY_LIMIT + 1]).await;
        LogWriter::new(Some(pool.clone()))
            .write_with_body(&sample_entry(200, 1, 0), capture)
            .await
            .unwrap();
        sqlx::query("DELETE FROM response_body_chunks WHERE log_id = 1 AND ordinal = 0")
            .execute(&pool)
            .await
            .unwrap();
        assert!(crate::logs::read_request_log_body_page(&pool, 1, 0)
            .await
            .unwrap_err()
            .contains("missing chunks"));
        let detail = crate::logs::read_request_log_detail(&pool, 1)
            .await
            .unwrap();
        assert!(detail
            .response_capture_error
            .unwrap()
            .contains("missing chunks"));
        sqlx::query(
            "UPDATE response_body_chunks SET data = X'61' WHERE log_id = 1 AND ordinal = 1",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            crate::logs::read_request_log_body_page(&pool, 1, BODY_PAGE_BYTES as u64)
                .await
                .unwrap_err()
                .contains("invalid chunk length")
        );
        let mut invalid = vec![b'a'; BODY_PAGE_BYTES];
        invalid[12] = 0xff;
        sqlx::query("UPDATE response_body_chunks SET data = ? WHERE log_id = 1 AND ordinal = 2")
            .bind(invalid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            crate::logs::read_request_log_body_page(&pool, 1, 2 * BODY_PAGE_BYTES as u64)
                .await
                .unwrap_err()
                .contains("invalid UTF-8")
        );
    }

    #[tokio::test]
    async fn cancelled_capture_error_and_prefix_are_visible_in_detail() {
        use crate::body_capture::{ResponseBodyCapture, BODY_MEMORY_LIMIT};
        use std::future::Future;
        use std::task::Poll;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlite::init_schema(&pool).await.unwrap();
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(&vec![b'p'; BODY_MEMORY_LIMIT]).await;
        let mut push = Box::pin(capture.push(b"new"));
        let pending =
            std::future::poll_fn(|cx| Poll::Ready(push.as_mut().poll(cx).is_pending())).await;
        drop(push);
        let writer = LogWriter::new(Some(pool.clone()));
        writer
            .write_with_body(&sample_entry(200, 1, 0), capture)
            .await
            .unwrap();
        let detail = crate::logs::read_request_log_detail(&pool, 1)
            .await
            .unwrap();
        assert_eq!(detail.status, 200);
        assert!(detail.response_body_bytes >= BODY_MEMORY_LIMIT as u64);
        assert!(detail
            .response_body
            .unwrap()
            .bytes()
            .all(|byte| byte == b'p'));
        if pending {
            assert!(detail
                .response_capture_error
                .unwrap()
                .contains("interrupted"));
        }
    }

    #[tokio::test]
    async fn chunk_insert_failure_rolls_back_parent_log_and_chunks() {
        use crate::body_capture::{ResponseBodyCapture, BODY_MEMORY_LIMIT};
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlite::init_schema(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_chunk BEFORE INSERT ON response_body_chunks WHEN NEW.ordinal = 1 BEGIN SELECT RAISE(ABORT, 'injected chunk failure'); END;").execute(&pool).await.unwrap();
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(&vec![b'a'; BODY_MEMORY_LIMIT + 1]).await;
        let writer = LogWriter::new(Some(pool.clone()));
        assert!(writer
            .write_with_body(&sample_entry(200, 1, 0), capture)
            .await
            .is_err());
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        let chunks: i64 = sqlx::query_scalar("SELECT count(*) FROM response_body_chunks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((rows, chunks), (0, 0));
    }

    #[tokio::test]
    async fn legacy_text_is_paginated_without_losing_utf8() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlite::init_schema(&pool).await.unwrap();
        let writer = LogWriter::new(Some(pool.clone()));
        let mut entry = sample_entry(200, 1, 0);
        let content = "中🙂a".repeat(20000);
        entry.response_body = Some(content.clone());
        writer.write(&entry).await;
        let mut actual = String::new();
        let mut offset = 0;
        loop {
            let page = crate::logs::read_request_log_body_page(&pool, 1, offset)
                .await
                .unwrap();
            actual.push_str(&page.text);
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(actual, content);
        assert!(crate::logs::read_request_log_body_page(&pool, 1, 1)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn chunked_body_round_trips_with_bounded_utf8_pages() {
        use crate::body_capture::{ResponseBodyCapture, BODY_MEMORY_LIMIT, BODY_PAGE_BYTES};
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlite::init_schema(&pool).await.unwrap();
        let mut capture = ResponseBodyCapture::new(true);
        let content = "中🙂a".repeat(BODY_MEMORY_LIMIT / 4);
        capture.push(content.as_bytes()).await;
        let writer = LogWriter::new(Some(pool.clone()));
        writer
            .write_with_body(&sample_entry(200, 1, 0), capture)
            .await
            .unwrap();
        let detail = crate::logs::read_request_log_detail(&pool, 1)
            .await
            .unwrap();
        assert_eq!(detail.response_body_bytes, content.len() as u64);
        assert!(detail.response_body.as_ref().unwrap().len() <= BODY_PAGE_BYTES);
        let mut actual = detail.response_body.unwrap();
        let mut offset = detail.response_body_next_offset;
        while let Some(next) = offset {
            let page = crate::logs::read_request_log_body_page(&pool, 1, next)
                .await
                .unwrap();
            assert!(page.text.len() <= BODY_PAGE_BYTES);
            actual.push_str(&page.text);
            offset = page.next_offset;
        }
        assert_eq!(actual, content);
        let sizes: Vec<i64> = sqlx::query_scalar("SELECT length(data) FROM response_body_chunks")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(sizes.len() > 1);
        assert!(sizes.iter().all(|size| *size <= BODY_PAGE_BYTES as i64));
    }

    #[tokio::test]
    async fn out_of_order_writes_bill_only_highest_completed_attempt() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        sqlite::init_schema(&pool).await.expect("init schema");
        let writer = LogWriter::new(Some(pool.clone()));

        writer.write(&sample_entry(200, 22, 1)).await;
        writer.write(&sample_entry(429, 11, 0)).await;

        let rows = sqlx::query(
            "SELECT status, attempt_index, is_billable FROM request_logs ORDER BY attempt_index ASC;",
        )
        .fetch_all(&pool)
        .await
        .expect("query attempts");
        assert_eq!(rows[0].try_get::<i64, _>("status").ok(), Some(429));
        assert_eq!(rows[0].try_get::<i64, _>("is_billable").ok(), Some(0));
        assert_eq!(rows[1].try_get::<i64, _>("status").ok(), Some(200));
        assert_eq!(rows[1].try_get::<i64, _>("is_billable").ok(), Some(1));
    }

    #[test]
    fn response_body_requires_explicit_detail_capture() {
        let mut hidden = sample_entry(200, 1, 0);
        attach_response_body(&mut hidden, "secret");
        assert!(hidden.response_body.is_none());

        let mut captured = sample_entry(200, 1, 0);
        captured.request_headers = Some("[]".to_string());
        attach_response_body(&mut captured, "captured");
        assert_eq!(captured.response_body.as_deref(), Some("captured"));
        assert_eq!(PRICE_MULTIPLIER_SCALE, 1_000_000_000_000);
    }

    #[tokio::test]
    async fn write_persists_upstream_response_model() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        sqlite::init_schema(&pool).await.expect("init schema");
        let writer = LogWriter::new(Some(pool.clone()));
        let mut entry = sample_entry(200, 1, 0);
        entry.model = Some("gpt-6-astra".to_string());
        entry.upstream_response_model = Some("gpt-5.6-luna".to_string());
        entry.local_api_key_id = Some("key-1".to_string());

        writer.write(&entry).await;

        let row = sqlx::query(
            "SELECT upstream_response_model, local_api_key_id FROM request_logs LIMIT 1;",
        )
        .fetch_one(&pool)
        .await
        .expect("query response model");
        assert_eq!(
            row.try_get::<Option<String>, _>("upstream_response_model")
                .ok()
                .flatten()
                .as_deref(),
            Some("gpt-5.6-luna")
        );
        assert_eq!(
            row.try_get::<Option<String>, _>("local_api_key_id")
                .ok()
                .flatten()
                .as_deref(),
            Some("key-1")
        );
    }
}
