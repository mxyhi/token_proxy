use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::AppHandle;
use std::collections::HashMap;

use super::sqlite;

const RECENT_PAGE_SIZE: u32 = 50;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardRange {
    pub(crate) from_ts_ms: Option<u64>,
    pub(crate) to_ts_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSummary {
    pub(crate) total_requests: u64,
    pub(crate) success_requests: u64,
    pub(crate) error_requests: u64,
    pub(crate) total_tokens: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cached_tokens: u64,
    pub(crate) avg_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardProviderStat {
    pub(crate) provider: String,
    pub(crate) requests: u64,
    pub(crate) total_tokens: u64,
    pub(crate) cached_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSeriesPoint {
    pub(crate) ts_ms: u64,
    pub(crate) total_requests: u64,
    pub(crate) error_requests: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cached_tokens: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardRequestItem {
    pub(crate) id: u64,
    pub(crate) ts_ms: u64,
    pub(crate) path: String,
    pub(crate) provider: String,
    pub(crate) upstream_id: String,
    pub(crate) model: Option<String>,
    pub(crate) stream: bool,
    pub(crate) status: u16,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) cached_tokens: Option<u64>,
    pub(crate) latency_ms: u64,
    pub(crate) upstream_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSnapshot {
    pub(crate) summary: DashboardSummary,
    pub(crate) providers: Vec<DashboardProviderStat>,
    pub(crate) series: Vec<DashboardSeriesPoint>,
    pub(crate) recent: Vec<DashboardRequestItem>,
    /// 是否只基于日志文件末尾片段做统计（Step1：true；Step2 SQLite 后应为 false）。
    pub(crate) truncated: bool,
}

pub(crate) async fn read_snapshot(
    app: AppHandle,
    range: DashboardRange,
    offset: Option<u32>,
) -> Result<DashboardSnapshot, String> {
    let offset = offset.unwrap_or(0);

    let pool = sqlite::open_read_pool(&app).await?;
    let from_ts_ms = range.from_ts_ms.map(|value| value as i64);
    let to_ts_ms = range.to_ts_ms.map(|value| value as i64);
    let bucket_ms = resolve_bucket_ms(&pool, from_ts_ms, to_ts_ms).await?;

    let summary = query_summary(&pool, from_ts_ms, to_ts_ms).await?;
    let providers = query_providers(&pool, from_ts_ms, to_ts_ms).await?;
    let series = query_series(&pool, from_ts_ms, to_ts_ms, bucket_ms).await?;
    let series = fill_series_buckets(series, from_ts_ms, to_ts_ms, bucket_ms);
    let recent = query_recent(&pool, from_ts_ms, to_ts_ms, offset).await?;

    Ok(DashboardSnapshot {
        summary,
        providers,
        series,
        recent,
        truncated: false,
    })
}

async fn query_summary(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
) -> Result<DashboardSummary, String> {
    let row = sqlx::query(
        r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE WHEN status BETWEEN 200 AND 299 THEN 1 ELSE 0 END), 0) AS success_requests,
  COALESCE(SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END), 0) AS error_requests,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(input_tokens, 0)), 0) AS input_tokens,
  COALESCE(SUM(COALESCE(output_tokens, 0)), 0) AS output_tokens,
  COALESCE(SUM(COALESCE(cached_tokens, 0)), 0) AS cached_tokens,
  COALESCE(SUM(latency_ms), 0) AS latency_sum_ms
FROM request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1) AND (?2 IS NULL OR ts_ms <= ?2);
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .fetch_one(pool)
    .await
    .map_err(|err| format!("Failed to query dashboard summary: {err}"))?;

    let total_requests = i64_to_u64(row.try_get("total_requests").unwrap_or(0));
    let success_requests = i64_to_u64(row.try_get("success_requests").unwrap_or(0));
    let error_requests = i64_to_u64(row.try_get("error_requests").unwrap_or(0));
    let total_tokens = i64_to_u64(row.try_get("total_tokens").unwrap_or(0));
    let input_tokens = i64_to_u64(row.try_get("input_tokens").unwrap_or(0));
    let output_tokens = i64_to_u64(row.try_get("output_tokens").unwrap_or(0));
    let cached_tokens = i64_to_u64(row.try_get("cached_tokens").unwrap_or(0));
    let latency_sum_ms = i64_to_u64(row.try_get("latency_sum_ms").unwrap_or(0));

    let avg_latency_ms = if total_requests == 0 {
        0
    } else {
        latency_sum_ms / total_requests
    };

    Ok(DashboardSummary {
        total_requests,
        success_requests,
        error_requests,
        total_tokens,
        input_tokens,
        output_tokens,
        cached_tokens,
        avg_latency_ms,
    })
}

async fn query_providers(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
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
  COALESCE(SUM(COALESCE(cached_tokens, 0)), 0) AS cached_tokens
FROM request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1) AND (?2 IS NULL OR ts_ms <= ?2)
GROUP BY provider
ORDER BY total_tokens DESC;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query provider stats: {err}"))?
    .into_iter()
    .filter_map(|row| {
        let provider: String = row.try_get("provider").ok()?;
        let requests: i64 = row.try_get("requests").ok()?;
        let total_tokens: i64 = row.try_get("total_tokens").ok()?;
        let cached_tokens: i64 = row.try_get("cached_tokens").ok()?;
        Some(DashboardProviderStat {
            provider,
            requests: i64_to_u64(requests),
            total_tokens: i64_to_u64(total_tokens),
            cached_tokens: i64_to_u64(cached_tokens),
        })
    })
    .collect::<Vec<_>>();

    Ok(providers)
}

async fn query_series(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    bucket_ms: u64,
) -> Result<Vec<DashboardSeriesPoint>, String> {
    let series = sqlx::query(
        r#"
SELECT
  (ts_ms / ?3) * ?3 AS bucket_ts_ms,
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END), 0) AS error_requests,
  COALESCE(SUM(COALESCE(input_tokens, 0)), 0) AS input_tokens,
  COALESCE(SUM(COALESCE(output_tokens, 0)), 0) AS output_tokens,
  COALESCE(SUM(COALESCE(cached_tokens, 0)), 0) AS cached_tokens,
  COALESCE(SUM(CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE 0
  END), 0) AS total_tokens
FROM request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1) AND (?2 IS NULL OR ts_ms <= ?2)
GROUP BY bucket_ts_ms
ORDER BY bucket_ts_ms ASC;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(i64::try_from(bucket_ms).unwrap_or(i64::MAX))
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
        let cached_tokens: i64 = row.try_get("cached_tokens").ok()?;
        let total_tokens: i64 = row.try_get("total_tokens").ok()?;
        Some(DashboardSeriesPoint {
            ts_ms: i64_to_u64(ts_ms),
            total_requests: i64_to_u64(total_requests),
            error_requests: i64_to_u64(error_requests),
            input_tokens: i64_to_u64(input_tokens),
            output_tokens: i64_to_u64(output_tokens),
            cached_tokens: i64_to_u64(cached_tokens),
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
                cached_tokens: 0,
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

async fn query_recent(
    pool: &sqlx::SqlitePool,
    from_ts_ms: Option<i64>,
    to_ts_ms: Option<i64>,
    offset: u32,
) -> Result<Vec<DashboardRequestItem>, String> {
    let recent = sqlx::query(
        r#"
SELECT
  id,
  ts_ms,
  path,
  provider,
  upstream_id,
  model,
  stream,
  status,
  CASE
    WHEN total_tokens IS NOT NULL THEN total_tokens
    WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ELSE NULL
  END AS total_tokens,
  cached_tokens,
  latency_ms,
  upstream_request_id
FROM request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1) AND (?2 IS NULL OR ts_ms <= ?2)
ORDER BY ts_ms DESC
LIMIT ?3 OFFSET ?4;
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
    .bind(i64::from(RECENT_PAGE_SIZE))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to query recent requests: {err}"))?
    .into_iter()
    .filter_map(|row| {
        let id: i64 = row.try_get("id").ok()?;
        let ts_ms: i64 = row.try_get("ts_ms").ok()?;
        let path: String = row.try_get("path").ok()?;
        let provider: String = row.try_get("provider").ok()?;
        let upstream_id: String = row.try_get("upstream_id").ok()?;
        let model: Option<String> = row.try_get("model").ok()?;
        let stream: bool = row.try_get("stream").unwrap_or(false);
        let status: i64 = row.try_get("status").unwrap_or(0);
        let total_tokens: Option<i64> = row.try_get("total_tokens").ok()?;
        let cached_tokens: Option<i64> = row.try_get("cached_tokens").ok()?;
        let latency_ms: i64 = row.try_get("latency_ms").unwrap_or(0);
        let upstream_request_id: Option<String> = row.try_get("upstream_request_id").ok()?;
        Some(DashboardRequestItem {
            id: i64_to_u64(id),
            ts_ms: i64_to_u64(ts_ms),
            path,
            provider,
            upstream_id,
            model,
            stream,
            status: i64_to_u16(status),
            total_tokens: total_tokens.map(i64_to_u64),
            cached_tokens: cached_tokens.map(i64_to_u64),
            latency_ms: i64_to_u64(latency_ms),
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
FROM request_logs
WHERE (?1 IS NULL OR ts_ms >= ?1) AND (?2 IS NULL OR ts_ms <= ?2);
"#,
    )
    .bind(from_ts_ms)
    .bind(to_ts_ms)
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

fn i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn i64_to_u16(value: i64) -> u16 {
    value.clamp(0, u16::MAX as i64) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series_point(ts_ms: u64, total_requests: u64) -> DashboardSeriesPoint {
        DashboardSeriesPoint {
            ts_ms,
            total_requests,
            error_requests: 0,
            input_tokens: total_requests,
            output_tokens: 0,
            cached_tokens: 0,
            total_tokens: total_requests,
        }
    }

    #[test]
    fn fill_series_buckets_inserts_missing_points() {
        let bucket_ms = 60_000;
        let series = vec![series_point(0, 1), series_point(120_000, 2)];
        let filled = fill_series_buckets(series, Some(0), Some(120_000), bucket_ms);
        assert_eq!(filled.len(), 3);
        assert_eq!(filled[0].ts_ms, 0);
        assert_eq!(filled[0].total_requests, 1);
        assert_eq!(filled[1].ts_ms, 60_000);
        assert_eq!(filled[1].total_requests, 0);
        assert_eq!(filled[2].ts_ms, 120_000);
        assert_eq!(filled[2].total_requests, 2);
    }

    #[test]
    fn fill_series_buckets_pads_start_and_end_of_range() {
        let bucket_ms = 60_000;
        let series = vec![series_point(120_000, 3)];
        let filled = fill_series_buckets(series, Some(0), Some(180_000), bucket_ms);
        assert_eq!(filled.len(), 4);
        assert_eq!(filled[0].ts_ms, 0);
        assert_eq!(filled[0].total_requests, 0);
        assert_eq!(filled[1].ts_ms, 60_000);
        assert_eq!(filled[1].total_requests, 0);
        assert_eq!(filled[2].ts_ms, 120_000);
        assert_eq!(filled[2].total_requests, 3);
        assert_eq!(filled[3].ts_ms, 180_000);
        assert_eq!(filled[3].total_requests, 0);
    }

    #[test]
    fn fill_series_buckets_handles_empty_series_with_explicit_range() {
        let bucket_ms = 60_000;
        let filled = fill_series_buckets(Vec::new(), Some(0), Some(120_000), bucket_ms);
        assert_eq!(filled.len(), 3);
        assert_eq!(filled[0].ts_ms, 0);
        assert_eq!(filled[1].ts_ms, 60_000);
        assert_eq!(filled[2].ts_ms, 120_000);
        assert!(filled.iter().all(|point| point.total_requests == 0));
    }

    #[test]
    fn fill_series_buckets_returns_original_when_range_unknown_and_empty() {
        let bucket_ms = 60_000;
        let filled = fill_series_buckets(Vec::new(), None, None, bucket_ms);
        assert!(filled.is_empty());
    }
}
