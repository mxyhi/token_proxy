use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(debug_assertions)]
macro_rules! debug_log_error {
    ($($arg:tt)*) => {
        eprintln!($($arg)*);
    };
}

#[cfg(not(debug_assertions))]
macro_rules! debug_log_error {
    ($($arg:tt)*) => {};
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TokenUsage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct UsageSnapshot {
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) cached_tokens: Option<u64>,
    pub(crate) usage_json: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct LogEntry {
    pub(crate) ts_ms: u128,
    pub(crate) path: String,
    pub(crate) provider: String,
    pub(crate) upstream_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) mapped_model: Option<String>,
    pub(crate) stream: bool,
    pub(crate) status: u16,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) cached_tokens: Option<u64>,
    pub(crate) usage_json: Option<Value>,
    pub(crate) upstream_request_id: Option<String>,
    pub(crate) request_headers: Option<String>,
    pub(crate) request_body: Option<String>,
    pub(crate) response_error: Option<String>,
    pub(crate) latency_ms: u128,
}

#[derive(Clone)]
pub(crate) struct LogContext {
    pub(crate) path: String,
    pub(crate) provider: String,
    pub(crate) upstream_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) mapped_model: Option<String>,
    pub(crate) stream: bool,
    pub(crate) status: u16,
    pub(crate) upstream_request_id: Option<String>,
    pub(crate) request_headers: Option<String>,
    pub(crate) request_body: Option<String>,
    // Time-to-first-byte (TTFB) measured from `start`.
    // For streaming responses, this is recorded when we receive the first upstream chunk.
    pub(crate) ttfb_ms: Option<u128>,
    pub(crate) start: Instant,
}

pub(crate) struct LogWriter {
    sqlite: Option<SqlitePool>,
}

impl LogWriter {
    pub(crate) fn new(sqlite: Option<SqlitePool>) -> Self {
        Self { sqlite }
    }

    // Fire-and-forget logging to avoid blocking the request path.
    pub(crate) fn write_detached(self: Arc<Self>, entry: LogEntry) {
        tokio::spawn(async move {
            self.write(&entry).await;
        });
    }

    pub(crate) fn write_account_state_detached(
        self: Arc<Self>,
        entry: crate::proxy::logs::AccountStateLogEntry,
    ) {
        tokio::spawn(async move {
            self.write_account_state(&entry).await;
        });
    }

    pub(crate) async fn write(&self, entry: &LogEntry) {
        let Some(pool) = self.sqlite.as_ref() else {
            return;
        };
        if let Err(_err) = insert_log_entry(pool, entry).await {
            debug_log_error!("proxy sqlite write failed: {_err}");
        }
    }

    pub(crate) async fn write_account_state(
        &self,
        entry: &crate::proxy::logs::AccountStateLogEntry,
    ) {
        let Some(pool) = self.sqlite.as_ref() else {
            return;
        };
        if let Err(_err) = crate::proxy::logs::write_account_state_log(pool, entry).await {
            debug_log_error!("proxy sqlite account state write failed: {_err}");
        }
    }
}

pub(crate) fn build_log_entry(
    context: &LogContext,
    usage: UsageSnapshot,
    response_error: Option<String>,
) -> LogEntry {
    LogEntry {
        ts_ms: now_ms(),
        path: context.path.clone(),
        provider: context.provider.clone(),
        upstream_id: context.upstream_id.clone(),
        account_id: context.account_id.clone(),
        model: context.model.clone(),
        mapped_model: context.mapped_model.clone(),
        stream: context.stream,
        status: context.status,
        usage: usage.usage,
        cached_tokens: usage.cached_tokens,
        usage_json: usage.usage_json,
        upstream_request_id: context.upstream_request_id.clone(),
        request_headers: context.request_headers.clone(),
        request_body: context.request_body.clone(),
        response_error,
        latency_ms: context
            .ttfb_ms
            .unwrap_or_else(|| context.start.elapsed().as_millis()),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

async fn insert_log_entry(pool: &SqlitePool, entry: &LogEntry) -> Result<(), sqlx::Error> {
    let usage = entry.usage.as_ref();
    let input_tokens = usage.and_then(|usage| usage.input_tokens).map(to_i64_u64);
    let output_tokens = usage.and_then(|usage| usage.output_tokens).map(to_i64_u64);
    let total_tokens = usage.and_then(|usage| usage.total_tokens).map(to_i64_u64);
    let cached_tokens = entry.cached_tokens.map(to_i64_u64);
    let usage_json = entry.usage_json.as_ref().map(Value::to_string);

    sqlx::query(
        r#"
INSERT INTO request_logs (
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
  usage_json,
  upstream_request_id,
  request_headers,
  request_body,
  response_error,
  latency_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
"#,
    )
    .bind(to_i64_u128(entry.ts_ms))
    .bind(entry.path.as_str())
    .bind(entry.provider.as_str())
    .bind(entry.upstream_id.as_str())
    .bind(entry.account_id.as_deref())
    .bind(entry.model.as_deref())
    .bind(entry.mapped_model.as_deref())
    .bind(entry.stream)
    .bind(i64::from(entry.status))
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(total_tokens)
    .bind(cached_tokens)
    .bind(usage_json.as_deref())
    .bind(entry.upstream_request_id.as_deref())
    .bind(entry.request_headers.as_deref())
    .bind(entry.request_body.as_deref())
    .bind(entry.response_error.as_deref())
    .bind(to_i64_u128(entry.latency_ms))
    .execute(pool)
    .await?;

    Ok(())
}

fn to_i64_u128(value: u128) -> i64 {
    value.min(i64::MAX as u128) as i64
}

fn to_i64_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
