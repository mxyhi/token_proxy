//! Dashboard 查询 SQL。
//!
//! 时间条件必须保持 `ts_ms >= ?1 AND ts_ms <= ?2` 形式（未指定端点时绑定 i64 边界）。
//! `(?1 IS NULL OR ts_ms >= ?1)` 会让 SQLite 放弃索引，退化为扫描含大字段 `usage_json`
//! 的整张 `request_logs`；聚合列均由部分覆盖索引 `idx_request_logs_dashboard` 提供。

/// 汇总卡片：请求数、成本、token 分量与延迟总和。
pub(super) const SUMMARY: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  );
"#;

/// 中位数延迟；CTE 保证 count 与排序数据来自同一快照。
pub(super) const MEDIAN_LATENCY: &str = r#"
WITH filtered AS (
    SELECT latency_ms
    FROM billable_request_logs
    WHERE ts_ms >= ?1
      AND ts_ms <= ?2
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
"#;

/// 按 provider 聚合用量。
pub(super) const PROVIDERS: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  )
GROUP BY provider
ORDER BY total_tokens DESC, requests DESC, provider ASC;
"#;

/// 按客户端请求模型（空则回退 mapped_model）聚合的用量排行。
pub(super) const MODELS: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  )
GROUP BY model_key
ORDER BY total_tokens DESC, requests DESC, model_key ASC
LIMIT ?5;
"#;

/// 模型筛选选项；不受当前 model 筛选影响。
pub(super) const MODEL_OPTIONS: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?3 IS NULL OR upstream_id = ?3)
GROUP BY model_key
ORDER BY total_tokens DESC, requests DESC, model_key ASC
LIMIT ?4;
"#;

/// 渠道选项与用量；只受时间范围限制。
pub(super) const UPSTREAMS: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
GROUP BY upstream_id
ORDER BY total_tokens DESC, requests DESC, upstream_id ASC;
"#;

/// 按时间桶聚合的趋势序列。
pub(super) const SERIES: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?4 IS NULL OR upstream_id = ?4)
  AND (
    ?5 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?5
  )
GROUP BY bucket_ts_ms
ORDER BY bucket_ts_ms ASC;
"#;

/// 最近请求分页。
pub(super) const RECENT: &str = r#"
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
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?5 IS NULL OR upstream_id = ?5)
  AND (
    ?6 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?6
  )
ORDER BY ts_ms DESC
LIMIT ?3 OFFSET ?4;
"#;

/// range=all 时推导实际时间跨度，用于选择桶大小。
pub(super) const TS_RANGE: &str = r#"
SELECT
  MIN(ts_ms) AS min_ts,
  MAX(ts_ms) AS max_ts
FROM billable_request_logs
WHERE ts_ms >= ?1
  AND ts_ms <= ?2
  AND (?3 IS NULL OR upstream_id = ?3)
  AND (
    ?4 IS NULL
    OR COALESCE(NULLIF(TRIM(model), ''), NULLIF(TRIM(mapped_model), ''), '(unknown)') = ?4
  );
"#;

#[cfg(test)]
pub(super) const ALL: [(&str, &str); 9] = [
    ("summary", SUMMARY),
    ("median_latency", MEDIAN_LATENCY),
    ("providers", PROVIDERS),
    ("models", MODELS),
    ("model_options", MODEL_OPTIONS),
    ("upstreams", UPSTREAMS),
    ("series", SERIES),
    ("recent", RECENT),
    ("ts_range", TS_RANGE),
];
