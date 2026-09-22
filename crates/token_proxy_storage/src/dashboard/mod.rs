use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_PAGE_SIZE: u32 = 50;
/// 模型用量排行上限；本地代理模型种类通常很少，20 足够扫一眼。
const MODEL_USAGE_TOP_LIMIT: u32 = 20;
/// 模型筛选下拉选项上限；与排行同一 key，不受当前 model 筛选影响。
const MODEL_OPTIONS_LIMIT: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamModelProbeStatus {
    Pending,
    Ok,
    Failed,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModelProbe {
    pub upstream_id: String,
    pub provider: String,
    pub account_id: Option<String>,
    pub status: UpstreamModelProbeStatus,
    pub checked_at_ts_ms: Option<u64>,
    pub error: Option<String>,
    pub models: Vec<String>,
}

impl UpstreamModelProbe {
    pub fn pending(upstream_id: &str, provider: &str, account_id: Option<String>) -> Self {
        Self {
            upstream_id: upstream_id.to_string(),
            provider: provider.to_string(),
            account_id,
            status: UpstreamModelProbeStatus::Pending,
            checked_at_ts_ms: None,
            error: None,
            models: Vec::new(),
        }
    }

    pub fn completed(
        upstream_id: &str,
        provider: &str,
        account_id: Option<String>,
        status: UpstreamModelProbeStatus,
        error: Option<String>,
        models: Vec<String>,
    ) -> Self {
        Self {
            upstream_id: upstream_id.to_string(),
            provider: provider.to_string(),
            account_id,
            status,
            checked_at_ts_ms: Some(now_ts_ms()),
            error,
            models,
        }
    }
}

fn now_ts_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardUsageBreakdown {
    /// 扫描型 UI 使用的缓存总量；精确计费仍使用下方独立分量。
    pub cached_tokens: u64,
    pub uncached_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub image_input_tokens: u64,
    pub image_output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRange {
    pub from_ts_ms: Option<u64>,
    pub to_ts_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub total_requests: u64,
    pub success_requests: u64,
    pub error_requests: u64,
    pub cost_nano_usd: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(flatten)]
    pub usage: DashboardUsageBreakdown,
    pub avg_latency_ms: u64,
    pub median_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardProviderStat {
    pub provider: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub cost_nano_usd: u64,
    #[serde(flatten)]
    pub usage: DashboardUsageBreakdown,
}

/// 按客户端请求模型（空则回退 mapped_model）聚合的用量排行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardModelStat {
    pub model: String,
    pub requests: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_nano_usd: u64,
    #[serde(flatten)]
    pub usage: DashboardUsageBreakdown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardUpstreamStat {
    pub upstream_id: String,
    pub requests: u64,
    pub total_tokens: u64,
    #[serde(flatten)]
    pub usage: DashboardUsageBreakdown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSeriesPoint {
    pub ts_ms: u64,
    pub total_requests: u64,
    pub error_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_nano_usd: u64,
    #[serde(flatten)]
    pub usage: DashboardUsageBreakdown,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRequestItem {
    pub id: u64,
    pub ts_ms: u64,
    pub client_ip: Option<String>,
    pub path: String,
    pub provider: String,
    pub upstream_id: String,
    pub account_id: Option<String>,
    pub model: Option<String>,
    pub mapped_model: Option<String>,
    pub upstream_response_model: Option<String>,
    pub stream: bool,
    pub status: u16,
    pub total_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
    pub image_input_tokens: Option<u64>,
    pub image_output_tokens: Option<u64>,
    pub service_tier: Option<String>,
    pub cost_nano_usd: Option<u64>,
    pub pricing_version: Option<String>,
    pub pricing_model: Option<String>,
    pub pricing_context_tier: Option<String>,
    pub latency_ms: u64,
    pub upstream_first_byte_ms: Option<u64>,
    pub upstream_response_headers_ms: Option<u64>,
    pub upstream_first_body_chunk_ms: Option<u64>,
    pub first_client_flush_ms: Option<u64>,
    pub first_output_ms: Option<u64>,
    pub upstream_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub summary: DashboardSummary,
    pub providers: Vec<DashboardProviderStat>,
    /// 模型用量 Top N（按 total_tokens 降序）。
    pub models: Vec<DashboardModelStat>,
    /// 模型筛选选项（时间/渠道收窄，不受当前 model 筛选影响）。
    pub model_options: Vec<String>,
    pub upstreams: Vec<DashboardUpstreamStat>,
    pub series: Vec<DashboardSeriesPoint>,
    pub recent: Vec<DashboardRequestItem>,
    pub model_probes: Vec<UpstreamModelProbe>,
    /// 是否只基于日志文件末尾片段做统计（Step1：true；Step2 SQLite 后应为 false）。
    pub truncated: bool,
}

pub async fn read_snapshot(
    pool: &sqlx::SqlitePool,
    range: DashboardRange,
    offset: Option<u32>,
    upstream_id: Option<String>,
    model: Option<String>,
) -> Result<DashboardSnapshot, String> {
    let offset = offset.unwrap_or(0);

    let from_ts_ms = range.from_ts_ms.map(|value| value as i64);
    let to_ts_ms = range.to_ts_ms.map(|value| value as i64);
    let upstream_id = upstream_id.as_deref();
    // 空串视为未筛选，避免前端误传 "" 时匹配不到任何行。
    let model = model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let bucket_ms = resolve_bucket_ms(pool, from_ts_ms, to_ts_ms, upstream_id, model).await?;

    let summary = query_summary(pool, from_ts_ms, to_ts_ms, upstream_id, model).await?;
    let providers = query_providers(pool, from_ts_ms, to_ts_ms, upstream_id, model).await?;
    let models = query_models(pool, from_ts_ms, to_ts_ms, upstream_id, model).await?;
    // 模型选项受时间/渠道限制，不受当前 model 筛选影响，便于切换其它模型。
    let model_options = query_model_options(pool, from_ts_ms, to_ts_ms, upstream_id).await?;
    // 选项列表只受时间范围限制，切换筛选时仍可看到同一范围内的其它上游。
    let upstreams = query_upstreams(pool, from_ts_ms, to_ts_ms).await?;
    let series = query_series(pool, from_ts_ms, to_ts_ms, bucket_ms, upstream_id, model).await?;
    let series = fill_series_buckets(series, from_ts_ms, to_ts_ms, bucket_ms);
    let recent = query_recent(pool, from_ts_ms, to_ts_ms, offset, upstream_id, model).await?;

    tracing::debug!(
        upstream_id = upstream_id,
        model = model,
        model_option_count = model_options.len(),
        "dashboard snapshot filters applied"
    );

    Ok(DashboardSnapshot {
        summary,
        providers,
        models,
        model_options,
        upstreams,
        series,
        recent,
        model_probes: Vec::new(),
        truncated: false,
    })
}

async fn query_summary(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<DashboardSummary, String> {
    let row = sqlx::query(
        r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE WHEN status BETWEEN 200 AND 299 THEN 1 ELSE 0 END), 0) AS success_requests,
  COALESCE(SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END), 0) AS error_requests,
  COALESCE(SUM(COALESCE(cost_nano_usd, 0)), 0) AS cost_nano_usd,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(input_tokens, 0)), 0) AS input_tokens,
  COALESCE(SUM(COALESCE(output_tokens, 0)), 0) AS output_tokens,
  COALESCE(SUM(COALESCE(uncached_input_tokens, 0)), 0) AS uncached_input_tokens,
  COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(cache_write_tokens, 0)), 0) AS cache_write_tokens,
  COALESCE(SUM(COALESCE(cache_write_5m_tokens, 0)), 0) AS cache_write_5m_tokens,
  COALESCE(SUM(COALESCE(cache_write_1h_tokens, 0)), 0) AS cache_write_1h_tokens,
  COALESCE(SUM(COALESCE(image_input_tokens, 0)), 0) AS image_input_tokens,
  COALESCE(SUM(COALESCE(image_output_tokens, 0)), 0) AS image_output_tokens,
  COALESCE(SUM(latency_ms), 0) AS latency_sum_ms
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  );
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(upstream_id)
    .bind(model)
    .fetch_one(pool)
    .await
    .map_err(|err| format!("Failed to query dashboard summary: {err}"))?;

    let total_requests = i64_to_u64(row.try_get("total_requests").unwrap_or(0));
    let success_requests = i64_to_u64(row.try_get("success_requests").unwrap_or(0));
    let error_requests = i64_to_u64(row.try_get("error_requests").unwrap_or(0));
    let cost_nano_usd = i64_to_u64(row.try_get("cost_nano_usd").unwrap_or(0));
    let total_tokens = i64_to_u64(row.try_get("total_tokens").unwrap_or(0));
    let input_tokens = i64_to_u64(row.try_get("input_tokens").unwrap_or(0));
    let output_tokens = i64_to_u64(row.try_get("output_tokens").unwrap_or(0));
    let usage = usage_breakdown_from_row(&row);
    let latency_sum_ms = i64_to_u64(row.try_get("latency_sum_ms").unwrap_or(0));

    let avg_latency_ms = latency_sum_ms.checked_div(total_requests).unwrap_or(0);

    // 中位数查询：使用 LIMIT/OFFSET 取中间值
    let median_latency_ms =
        query_median_latency(pool, from_ts_ms, to_ts_ms, upstream_id, model).await?;

    Ok(DashboardSummary {
        total_requests,
        success_requests,
        error_requests,
        cost_nano_usd,
        total_tokens,
        input_tokens,
        output_tokens,
        usage,
        avg_latency_ms,
        median_latency_ms,
    })
}

/// 计算中位数延迟（SQLite 无内置 MEDIAN，使用单条子查询避免并发写入时的 count/offset 错位）
async fn query_median_latency(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<u64, String> {
    // 单条 SQL 完成中位数计算：
    // - 使用 CTE 保证 count 和数据在同一快照内
    // - 奇数个取中间值，偶数个取中间两个值的整数除法平均
    let row = sqlx::query(
        r#"
WITH filtered AS (
    SELECT latency_ms
    FROM billable_request_logs
    WHERE (?1 IS NULL OR ts_ms >= ?1)
      AND (?2 IS NULL OR ts_ms <= ?2)
      AND (?3 IS NULL OR upstream_id = ?3)
      AND (
        ?4 IS NULL
        OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
      )
),
cnt AS (
    SELECT COUNT(*) AS n FROM filtered
),
ordered AS (
    SELECT latency_ms, ROW_NUMBER() OVER (ORDER BY latency_ms) AS rn
    FROM filtered
)
SELECT COALESCE(
    CASE
        WHEN (SELECT n FROM cnt) = 0 THEN 0
        WHEN (SELECT n FROM cnt) % 2 = 1 THEN
            (SELECT latency_ms FROM ordered WHERE rn = ((SELECT n FROM cnt) + 1) / 2)
        ELSE
            (SELECT (o1.latency_ms + o2.latency_ms) / 2
             FROM ordered o1, ordered o2
             WHERE o1.rn = (SELECT n FROM cnt) / 2 AND o2.rn = (SELECT n FROM cnt) / 2 + 1)
    END,
    0
) AS median_latency;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(upstream_id)
    .bind(model)
    .fetch_one(pool)
    .await
    .map_err(|err| format!("Failed to query median latency: {err}"))?;

    let median: i64 = row.try_get("median_latency").unwrap_or(0);
    Ok(i64_to_u64(median))
}

async fn query_providers(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<Vec<DashboardProviderStat>, String> {
    let providers = sqlx::query(
        r#"
SELECT
  provider,
  COUNT(*) AS requests,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(cost_nano_usd, 0)), 0) AS cost_nano_usd,
  COALESCE(SUM(COALESCE(uncached_input_tokens, 0)), 0) AS uncached_input_tokens,
  COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(cache_write_tokens, 0)), 0) AS cache_write_tokens,
  COALESCE(SUM(COALESCE(cache_write_5m_tokens, 0)), 0) AS cache_write_5m_tokens,
  COALESCE(SUM(COALESCE(cache_write_1h_tokens, 0)), 0) AS cache_write_1h_tokens,
  COALESCE(SUM(COALESCE(image_input_tokens, 0)), 0) AS image_input_tokens,
  COALESCE(SUM(COALESCE(image_output_tokens, 0)), 0) AS image_output_tokens
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  )
GROUP BY provider
ORDER BY total_tokens DESC, requests DESC, provider ASC;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(upstream_id)
    .bind(model)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query provider stats: {err}"))?
    .into_iter()
    .filter_map(|row| {
        let provider: String = row.try_get("provider").ok()?;
        let requests: i64 = row.try_get("requests").ok()?;
        let total_tokens: i64 = row.try_get("total_tokens").ok()?;
        let cost_nano_usd: i64 = row.try_get("cost_nano_usd").ok()?;
        let usage = usage_breakdown_from_row(&row);
        Some(DashboardProviderStat {
            provider,
            requests: i64_to_u64(requests),
            total_tokens: i64_to_u64(total_tokens),
            cost_nano_usd: i64_to_u64(cost_nano_usd),
            usage,
        })
    })
    .collect::<Vec<_>>();

    Ok(providers)
}

/// 按客户端请求模型聚合用量；空 model 回退 mapped_model，再 unknown。
async fn query_models(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<Vec<DashboardModelStat>, String> {
    let models = sqlx::query(
        r#"
SELECT
  COALESCE(
    NULLIF(TRIM(model), ''),
    NULLIF(TRIM(mapped_model), ''),
    '(unknown)'
  ) AS model_key,
  COUNT(*) AS requests,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(input_tokens, 0)), 0) AS input_tokens,
  COALESCE(SUM(COALESCE(output_tokens, 0)), 0) AS output_tokens,
  COALESCE(SUM(COALESCE(cost_nano_usd, 0)), 0) AS cost_nano_usd,
  COALESCE(SUM(COALESCE(uncached_input_tokens, 0)), 0) AS uncached_input_tokens,
  COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(cache_write_tokens, 0)), 0) AS cache_write_tokens,
  COALESCE(SUM(COALESCE(cache_write_5m_tokens, 0)), 0) AS cache_write_5m_tokens,
  COALESCE(SUM(COALESCE(cache_write_1h_tokens, 0)), 0) AS cache_write_1h_tokens,
  COALESCE(SUM(COALESCE(image_input_tokens, 0)), 0) AS image_input_tokens,
  COALESCE(SUM(COALESCE(image_output_tokens, 0)), 0) AS image_output_tokens
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  )
GROUP BY model_key
ORDER BY total_tokens DESC, requests DESC, model_key ASC
LIMIT ?5;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(upstream_id)
    .bind(model)
    .bind(i64::from(MODEL_USAGE_TOP_LIMIT))
    .fetch_all(pool)
    .await
    .map_err(|err| {
        tracing::warn!(error = %err, "dashboard model usage query failed");
        format!("Failed to query model stats: {err}")
    })?
    .into_iter()
    .filter_map(|row| {
        let model: String = row.try_get("model_key").ok()?;
        let requests: i64 = row.try_get("requests").ok()?;
        let total_tokens: i64 = row.try_get("total_tokens").ok()?;
        let input_tokens: i64 = row.try_get("input_tokens").ok()?;
        let output_tokens: i64 = row.try_get("output_tokens").ok()?;
        let cost_nano_usd: i64 = row.try_get("cost_nano_usd").ok()?;
        let usage = usage_breakdown_from_row(&row);
        Some(DashboardModelStat {
            model,
            requests: i64_to_u64(requests),
            total_tokens: i64_to_u64(total_tokens),
            input_tokens: i64_to_u64(input_tokens),
            output_tokens: i64_to_u64(output_tokens),
            cost_nano_usd: i64_to_u64(cost_nano_usd),
            usage,
        })
    })
    .collect::<Vec<_>>();

    tracing::debug!(
        model_count = models.len(),
        "dashboard model usage aggregation ready"
    );
    Ok(models)
}

/// 模型筛选下拉选项；key 与用量排行一致，不受当前 model 筛选影响。
async fn query_model_options(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    upstream_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let options = sqlx::query(
        r#"
SELECT
  COALESCE(
    NULLIF(TRIM(model), ''),
    NULLIF(TRIM(mapped_model), ''),
    '(unknown)'
  ) AS model_key,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COUNT(*) AS requests
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?3 IS NULL OR upstream_id = ?3)
GROUP BY model_key
ORDER BY total_tokens DESC, requests DESC, model_key ASC
LIMIT ?4;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(upstream_id)
    .bind(i64::from(MODEL_OPTIONS_LIMIT))
    .fetch_all(pool)
    .await
    .map_err(|err| {
        tracing::warn!(error = %err, "dashboard model options query failed");
        format!("Failed to query model options: {err}")
    })?
    .into_iter()
    .filter_map(|row| row.try_get::<String, _>("model_key").ok())
    .collect::<Vec<_>>();

    tracing::debug!(
        model_option_count = options.len(),
        "dashboard model filter options ready"
    );
    Ok(options)
}

async fn query_upstreams(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
) -> Result<Vec<DashboardUpstreamStat>, String> {
    let upstreams = sqlx::query(
        r#"
SELECT
  upstream_id,
  COUNT(*) AS requests,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(uncached_input_tokens, 0)), 0) AS uncached_input_tokens,
  COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(cache_write_tokens, 0)), 0) AS cache_write_tokens,
  COALESCE(SUM(COALESCE(cache_write_5m_tokens, 0)), 0) AS cache_write_5m_tokens,
  COALESCE(SUM(COALESCE(cache_write_1h_tokens, 0)), 0) AS cache_write_1h_tokens,
  COALESCE(SUM(COALESCE(image_input_tokens, 0)), 0) AS image_input_tokens,
  COALESCE(SUM(COALESCE(image_output_tokens, 0)), 0) AS image_output_tokens
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
GROUP BY upstream_id
ORDER BY total_tokens DESC, requests DESC, upstream_id ASC;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query dashboard upstreams: {err}"))?
    .into_iter()
    .filter_map(|row| {
        let upstream_id: String = row.try_get("upstream_id").ok()?;
        let requests: i64 = row.try_get("requests").ok()?;
        let total_tokens: i64 = row.try_get("total_tokens").ok()?;
        let usage = usage_breakdown_from_row(&row);
        Some(DashboardUpstreamStat {
            upstream_id,
            requests: i64_to_u64(requests),
            total_tokens: i64_to_u64(total_tokens),
            usage,
        })
    })
    .collect::<Vec<_>>();

    Ok(upstreams)
}

async fn query_series(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    bucket_ms: u64,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<Vec<DashboardSeriesPoint>, String> {
    let series = sqlx::query(
        r#"
SELECT
  (ts_ms / ?3) * ?3 AS bucket_ts_ms,
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END), 0) AS error_requests,
  COALESCE(SUM(COALESCE(input_tokens, 0)), 0) AS input_tokens,
  COALESCE(SUM(COALESCE(output_tokens, 0)), 0) AS output_tokens,
  COALESCE(SUM(COALESCE(cost_nano_usd, 0)), 0) AS cost_nano_usd,
  COALESCE(SUM(COALESCE(uncached_input_tokens, 0)), 0) AS uncached_input_tokens,
  COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(cache_write_tokens, 0)), 0) AS cache_write_tokens,
  COALESCE(SUM(COALESCE(cache_write_5m_tokens, 0)), 0) AS cache_write_5m_tokens,
  COALESCE(SUM(COALESCE(cache_write_1h_tokens, 0)), 0) AS cache_write_1h_tokens,
  COALESCE(SUM(COALESCE(image_input_tokens, 0)), 0) AS image_input_tokens,
  COALESCE(SUM(COALESCE(image_output_tokens, 0)), 0) AS image_output_tokens,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?4 IS NULL OR upstream_id = ?4)
  AND (
    ?5 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?5
  )
GROUP BY bucket_ts_ms
ORDER BY bucket_ts_ms ASC;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(i64::try_from(bucket_ms).unwrap_or(i64::MAX))
    .bind(upstream_id)
    .bind(model)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query dashboard series: {err}"))?
    .into_iter()
    .filter_map(|row| {
        let ts_ms: i64 = row.try_get("bucket_ts_ms").ok()?;
        let total_requests: i64 = row.try_get("total_requests").ok()?;
        let error_requests: i64 = row.try_get("error_requests").ok()?;
        let input_tokens: i64 = row.try_get("input_tokens").ok()?;
        let output_tokens: i64 = row.try_get("output_tokens").ok()?;
        let cost_nano_usd: i64 = row.try_get("cost_nano_usd").ok()?;
        let usage = usage_breakdown_from_row(&row);
        let total_tokens: i64 = row.try_get("total_tokens").ok()?;
        Some(DashboardSeriesPoint {
            ts_ms: i64_to_u64(ts_ms),
            total_requests: i64_to_u64(total_requests),
            error_requests: i64_to_u64(error_requests),
            input_tokens: i64_to_u64(input_tokens),
            output_tokens: i64_to_u64(output_tokens),
            cost_nano_usd: i64_to_u64(cost_nano_usd),
            usage,
            total_tokens: i64_to_u64(total_tokens),
        })
    })
    .collect::<Vec<_>>();

    Ok(series)
}

fn fill_series_buckets(
    series: Vec<DashboardSeriesPoint>,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    bucket_ms: u64,
) -> Vec<DashboardSeriesPoint> {
    if bucket_ms == 0 {
        return series;
    }

    let resolved_from_ts_ms = from_ts_ms.or_else(|| {
        series
            .first()
            .and_then(|point| i64::try_from(point.ts_ms).ok())
    });
    let resolved_to_ts_ms = to_ts_ms.or_else(|| {
        series
            .last()
            .and_then(|point| i64::try_from(point.ts_ms).ok())
    });

    // range=all 且没有任何数据时交给前端兜底（最近 7 天 0 线）。
    let (resolved_from_ts_ms, resolved_to_ts_ms) = match (resolved_from_ts_ms, resolved_to_ts_ms) {
        (Some(from), Some(to)) => (from, to),
        _ => return series,
    };

    let start_bucket_ts_ms = align_down_bucket_ts_ms(resolved_from_ts_ms, bucket_ms);
    let end_bucket_ts_ms = align_down_bucket_ts_ms(resolved_to_ts_ms, bucket_ms);

    let (start_bucket_ts_ms, end_bucket_ts_ms) = if end_bucket_ts_ms < start_bucket_ts_ms {
        (start_bucket_ts_ms, start_bucket_ts_ms)
    } else {
        (start_bucket_ts_ms, end_bucket_ts_ms)
    };

    let by_bucket: HashMap<u64, DashboardSeriesPoint> = series
        .into_iter()
        .map(|point| (point.ts_ms, point))
        .collect();

    let expected_len = ((end_bucket_ts_ms - start_bucket_ts_ms) / bucket_ms).saturating_add(1);
    let mut filled = Vec::with_capacity(usize::try_from(expected_len).unwrap_or(usize::MAX));

    let mut cursor_ts_ms = start_bucket_ts_ms;
    while cursor_ts_ms <= end_bucket_ts_ms {
        if let Some(point) = by_bucket.get(&cursor_ts_ms) {
            filled.push(point.clone());
        } else {
            filled.push(DashboardSeriesPoint {
                ts_ms: cursor_ts_ms,
                total_requests: 0,
                error_requests: 0,
                input_tokens: 0,
                output_tokens: 0,
                cost_nano_usd: 0,
                usage: DashboardUsageBreakdown::default(),
                total_tokens: 0,
            });
        }

        match cursor_ts_ms.checked_add(bucket_ms) {
            Some(next) => cursor_ts_ms = next,
            None => break,
        }
    }

    filled
}

fn align_down_bucket_ts_ms(ts_ms: i64, bucket_ms: u64) -> u64 {
    let ts_ms = i64_to_u64(ts_ms);
    if bucket_ms == 0 {
        return ts_ms;
    }
    (ts_ms / bucket_ms) * bucket_ms
}

#[allow(clippy::too_many_arguments)]
async fn query_recent(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    offset: u32,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<Vec<DashboardRequestItem>, String> {
    let recent = sqlx::query(
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
  CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE NULL
  END AS total_tokens,
  output_tokens,
  CASE
    WHEN cache_read_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL
      OR cache_write_5m_tokens IS NOT NULL OR cache_write_1h_tokens IS NOT NULL
    THEN COALESCE(cache_read_tokens, 0) + COALESCE(cache_write_tokens, 0)
      + COALESCE(cache_write_5m_tokens, 0) + COALESCE(cache_write_1h_tokens, 0)
    ELSE NULL
  END AS cached_tokens,
  uncached_input_tokens,
  cache_read_tokens,
  cache_write_tokens,
  cache_write_5m_tokens,
  cache_write_1h_tokens,
  image_input_tokens,
  image_output_tokens,
  service_tier,
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
  upstream_request_id
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?5 IS NULL OR upstream_id = ?5)
  AND (
    ?6 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?6
  )
ORDER BY ts_ms DESC
LIMIT ?3 OFFSET ?4;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(i64::from(RECENT_PAGE_SIZE))
    .bind(i64::from(offset))
    .bind(upstream_id)
    .bind(model)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query recent requests: {err}"))?
    .into_iter()
    .filter_map(|row| {
        let id: i64 = row.try_get("id").ok()?;
        let ts_ms: i64 = row.try_get("ts_ms").ok()?;
        let client_ip: Option<String> = row.try_get("client_ip").ok()?;
        let path: String = row.try_get("path").ok()?;
        let provider: String = row.try_get("provider").ok()?;
        let upstream_id: String = row.try_get("upstream_id").ok()?;
        let account_id: Option<String> = row.try_get("account_id").ok()?;
        let model: Option<String> = row.try_get("model").ok()?;
        let mapped_model: Option<String> = row.try_get("mapped_model").ok()?;
        let upstream_response_model: Option<String> =
            row.try_get("upstream_response_model").ok()?;
        let stream: bool = row.try_get("stream").unwrap_or(false);
        let status: i64 = row.try_get("status").unwrap_or(0);
        let total_tokens: Option<i64> = row.try_get("total_tokens").ok()?;
        let output_tokens: Option<i64> = row.try_get("output_tokens").ok()?;
        let cached_tokens: Option<i64> = row.try_get("cached_tokens").ok()?;
        let uncached_input_tokens: Option<i64> = row.try_get("uncached_input_tokens").ok()?;
        let cache_read_tokens: Option<i64> = row.try_get("cache_read_tokens").ok()?;
        let cache_write_tokens: Option<i64> = row.try_get("cache_write_tokens").ok()?;
        let cache_write_5m_tokens: Option<i64> = row.try_get("cache_write_5m_tokens").ok()?;
        let cache_write_1h_tokens: Option<i64> = row.try_get("cache_write_1h_tokens").ok()?;
        let image_input_tokens: Option<i64> = row.try_get("image_input_tokens").ok()?;
        let image_output_tokens: Option<i64> = row.try_get("image_output_tokens").ok()?;
        let service_tier: Option<String> = row.try_get("service_tier").ok()?;
        let cost_nano_usd: Option<i64> = row.try_get("cost_nano_usd").ok()?;
        let pricing_version: Option<String> = row.try_get("pricing_version").ok()?;
        let pricing_model: Option<String> = row.try_get("pricing_model").ok()?;
        let pricing_context_tier: Option<String> = row.try_get("pricing_context_tier").ok()?;
        let latency_ms: i64 = row.try_get("latency_ms").unwrap_or(0);
        let upstream_first_byte_ms: Option<i64> = row.try_get("upstream_first_byte_ms").ok()?;
        let upstream_response_headers_ms: Option<i64> =
            row.try_get("upstream_response_headers_ms").ok()?;
        let upstream_first_body_chunk_ms: Option<i64> =
            row.try_get("upstream_first_body_chunk_ms").ok()?;
        let first_client_flush_ms: Option<i64> = row.try_get("first_client_flush_ms").ok()?;
        let first_output_ms: Option<i64> = row.try_get("first_output_ms").ok()?;
        let upstream_request_id: Option<String> = row.try_get("upstream_request_id").ok()?;
        Some(DashboardRequestItem {
            id: i64_to_u64(id),
            ts_ms: i64_to_u64(ts_ms),
            client_ip,
            path,
            provider,
            upstream_id,
            account_id,
            model,
            mapped_model,
            upstream_response_model,
            stream,
            status: i64_to_u16(status),
            total_tokens: total_tokens.map(i64_to_u64),
            output_tokens: output_tokens.map(i64_to_u64),
            cached_tokens: cached_tokens.map(i64_to_u64),
            uncached_input_tokens: uncached_input_tokens.map(i64_to_u64),
            cache_read_tokens: cache_read_tokens.map(i64_to_u64),
            cache_write_tokens: cache_write_tokens.map(i64_to_u64),
            cache_write_5m_tokens: cache_write_5m_tokens.map(i64_to_u64),
            cache_write_1h_tokens: cache_write_1h_tokens.map(i64_to_u64),
            image_input_tokens: image_input_tokens.map(i64_to_u64),
            image_output_tokens: image_output_tokens.map(i64_to_u64),
            service_tier,
            cost_nano_usd: cost_nano_usd.map(i64_to_u64),
            pricing_version,
            pricing_model,
            pricing_context_tier,
            latency_ms: i64_to_u64(latency_ms),
            upstream_first_byte_ms: upstream_first_byte_ms.map(i64_to_u64),
            upstream_response_headers_ms: upstream_response_headers_ms.map(i64_to_u64),
            upstream_first_body_chunk_ms: upstream_first_body_chunk_ms.map(i64_to_u64),
            first_client_flush_ms: first_client_flush_ms.map(i64_to_u64),
            first_output_ms: first_output_ms.map(i64_to_u64),
            upstream_request_id,
        })
    })
    .collect::<Vec<_>>();

    Ok(recent)
}

async fn resolve_bucket_ms(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    upstream_id: Option<&str>,
    model: Option<&str>,
) -> Result<u64, String> {
    if let (Some(from), Some(to)) = (from_ts_ms, to_ts_ms) {
        let span_ms = (to - from).max(0) as u64;
        return Ok(select_bucket_ms(span_ms));
    }

    let row = sqlx::query(
        r#"
SELECT
  MIN(ts_ms) AS min_ts,
  MAX(ts_ms) AS max_ts
FROM billable_request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1)
  AND (?2 IS NULL OR ts_ms <= ?2)
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  );
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(upstream_id)
    .bind(model)
    .fetch_one(pool)
    .await
    .map_err(|err| format!("Failed to query dashboard range: {err}"))?;

    let min_ts: Option<i64> = row.try_get("min_ts").ok();
    let max_ts: Option<i64> = row.try_get("max_ts").ok();
    let start = from_ts_ms.or(min_ts).unwrap_or(0);
    let end = to_ts_ms.or(max_ts).unwrap_or(start);
    let span_ms = (end - start).max(0) as u64;
    Ok(select_bucket_ms(span_ms))
}

fn select_bucket_ms(span_ms: u64) -> u64 {
    // 根据跨度选择合适的桶大小，避免点数过多或过少。
    if span_ms <= 60 * 60 * 1000 {
        return 5 * 60 * 1000;
    }
    if span_ms <= 6 * 60 * 60 * 1000 {
        return 15 * 60 * 1000;
    }
    if span_ms <= 24 * 60 * 60 * 1000 {
        return 30 * 60 * 1000;
    }
    if span_ms <= 7 * 24 * 60 * 60 * 1000 {
        return 2 * 60 * 60 * 1000;
    }
    if span_ms <= 31 * 24 * 60 * 60 * 1000 {
        return 24 * 60 * 60 * 1000;
    }
    7 * 24 * 60 * 60 * 1000
}

fn usage_breakdown_from_row(row: &sqlx::sqlite::SqliteRow) -> DashboardUsageBreakdown {
    let value = |column| i64_to_u64(row.try_get::<i64, _>(column).unwrap_or(0));
    DashboardUsageBreakdown {
        cached_tokens: value("cache_read_tokens")
            .saturating_add(value("cache_write_tokens"))
            .saturating_add(value("cache_write_5m_tokens"))
            .saturating_add(value("cache_write_1h_tokens")),
        uncached_input_tokens: value("uncached_input_tokens"),
        cache_read_tokens: value("cache_read_tokens"),
        cache_write_tokens: value("cache_write_tokens"),
        cache_write_5m_tokens: value("cache_write_5m_tokens"),
        cache_write_1h_tokens: value("cache_write_1h_tokens"),
        image_input_tokens: value("image_input_tokens"),
        image_output_tokens: value("image_output_tokens"),
    }
}

fn i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn i64_to_u16(value: i64) -> u16 {
    value.clamp(0, u16::MAX as i64) as u16
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
