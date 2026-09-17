use super::*;

fn series_point(ts_ms: u64, total_requests: u64) -> DashboardSeriesPoint {
    DashboardSeriesPoint {
        ts_ms,
        total_requests,
        error_requests: 0,
        input_tokens: total_requests,
        output_tokens: 0,
        cost_nano_usd: 0,
        usage: DashboardUsageBreakdown::default(),
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

// ============================================================================
// query_median_latency 测试
// ============================================================================

use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

/// 创建内存数据库并初始化 schema
async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");

    crate::sqlite::init_schema(&pool)
        .await
        .expect("Failed to initialize schema");

    pool
}

/// 插入测试数据，只需指定 latency_ms
async fn insert_latency(pool: &SqlitePool, latency_ms: i64) {
    sqlx::query(
        r#"
        INSERT INTO request_logs (ts_ms, path, provider, upstream_id, stream, status, latency_ms)
        VALUES (0, '/test', 'test', 'test', 0, 200, ?)
        "#,
    )
    .bind(latency_ms)
    .execute(pool)
    .await
    .expect("Failed to insert test data");
}

#[allow(clippy::too_many_arguments)]
async fn insert_request(
    pool: &SqlitePool,
    ts_ms: i64,
    provider: &str,
    upstream_id: &str,
    account_id: Option<&str>,
    status: i64,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    latency_ms: i64,
) {
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
            input_tokens,
            output_tokens,
            total_tokens,
            cache_read_tokens,
            cache_write_tokens,
            latency_ms
        )
        VALUES (?, '/test', ?, ?, ?, 0, ?, ?, ?, ?, ?, 0, ?)
        "#,
    )
    .bind(ts_ms)
    .bind(provider)
    .bind(upstream_id)
    .bind(account_id)
    .bind(status)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(total_tokens)
    .bind(cache_read_tokens)
    .bind(latency_ms)
    .execute(pool)
    .await
    .expect("Failed to insert request");
}

async fn insert_priced_request(
    pool: &SqlitePool,
    ts_ms: i64,
    upstream_id: &str,
    cost_nano_usd: Option<i64>,
    pricing_version: Option<&str>,
    pricing_model: Option<&str>,
    pricing_context_tier: Option<&str>,
) {
    sqlx::query(
        r#"
        INSERT INTO request_logs (
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
            cache_read_tokens,
            cache_write_tokens,
            latency_ms,
            cost_nano_usd,
            pricing_version,
            pricing_model,
            pricing_context_tier
        )
        VALUES (?, '/test', 'openai', ?, 'gpt-5.4', 'gpt-5.4', 0, 200, 100, 50, 150, 10, 0, 30, ?, ?, ?, ?)
        "#,
    )
    .bind(ts_ms)
    .bind(upstream_id)
    .bind(cost_nano_usd)
    .bind(pricing_version)
    .bind(pricing_model)
    .bind(pricing_context_tier)
    .execute(pool)
    .await
    .expect("Failed to insert priced request");
}

async fn insert_request_with_client_ip(pool: &SqlitePool, ts_ms: i64, client_ip: &str) {
    sqlx::query(
        r#"
        INSERT INTO request_logs (
            ts_ms,
            client_ip,
            path,
            provider,
            upstream_id,
            stream,
            status,
            latency_ms
        )
        VALUES (?, ?, '/test', 'openai', 'alpha', 0, 200, 30)
        "#,
    )
    .bind(ts_ms)
    .bind(client_ip)
    .execute(pool)
    .await
    .expect("Failed to insert request with client IP");
}

#[tokio::test]
async fn median_latency_empty_table_returns_zero() {
    let pool = setup_test_db().await;
    let result = query_median_latency(&pool, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(result, 0, "Empty table should return 0");
}

#[tokio::test]
async fn median_latency_single_value() {
    let pool = setup_test_db().await;
    insert_latency(&pool, 100).await;

    let result = query_median_latency(&pool, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(result, 100, "Single value should be the median");
}

#[tokio::test]
async fn median_latency_odd_count() {
    let pool = setup_test_db().await;
    // 插入 3 个值: 10, 20, 30 -> 中位数应为 20
    insert_latency(&pool, 10).await;
    insert_latency(&pool, 30).await;
    insert_latency(&pool, 20).await;

    let result = query_median_latency(&pool, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(result, 20, "Odd count median should be middle value");
}

#[tokio::test]
async fn median_latency_even_count() {
    let pool = setup_test_db().await;
    // 插入 4 个值: 10, 20, 30, 40 -> 中位数应为 (20+30)/2 = 25
    insert_latency(&pool, 10).await;
    insert_latency(&pool, 40).await;
    insert_latency(&pool, 20).await;
    insert_latency(&pool, 30).await;

    let result = query_median_latency(&pool, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(
        result, 25,
        "Even count median should be average of two middle values"
    );
}

#[tokio::test]
async fn median_latency_even_count_rounds_down() {
    let pool = setup_test_db().await;
    // 插入 2 个值: 10, 21 -> 中位数应为 (10+21)/2 = 15 (整数除法向下取整)
    insert_latency(&pool, 10).await;
    insert_latency(&pool, 21).await;

    let result = query_median_latency(&pool, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(result, 15, "Median should use integer division");
}

#[tokio::test]
async fn median_latency_with_time_range_filter() {
    let pool = setup_test_db().await;

    // 插入不同时间戳的数据
    sqlx::query(
        "INSERT INTO request_logs (ts_ms, path, provider, upstream_id, stream, status, latency_ms) VALUES (100, '/test', 'test', 'test', 0, 200, 50)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO request_logs (ts_ms, path, provider, upstream_id, stream, status, latency_ms) VALUES (200, '/test', 'test', 'test', 0, 200, 100)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO request_logs (ts_ms, path, provider, upstream_id, stream, status, latency_ms) VALUES (300, '/test', 'test', 'test', 0, 200, 150)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // 只查询 ts_ms 在 150-250 范围内的数据，应该只有 latency_ms=100 的记录
    let result = query_median_latency(&pool, Some(150), Some(250), None, None)
        .await
        .unwrap();
    assert_eq!(result, 100, "Should filter by time range");

    // 查询所有数据，中位数应为 100
    let result_all = query_median_latency(&pool, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(result_all, 100, "All data median should be 100");
}

#[tokio::test]
async fn read_snapshot_filters_by_channel_and_preserves_account_identity() {
    let pool = setup_test_db().await;
    insert_request(
        &pool,
        100,
        "openai",
        "alpha",
        Some("codex-a.json"),
        200,
        Some(10),
        Some(20),
        None,
        Some(5),
        30,
    )
    .await;
    insert_request(
        &pool,
        150,
        "openai-response",
        "alpha",
        None,
        200,
        Some(2),
        Some(3),
        None,
        Some(1),
        40,
    )
    .await;
    insert_request(
        &pool,
        200,
        "anthropic",
        "beta",
        None,
        500,
        Some(3),
        Some(4),
        None,
        Some(1),
        90,
    )
    .await;

    let snapshot = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        Some(String::from("alpha")),
        None,
    )
    .await
    .unwrap();

    assert_eq!(snapshot.summary.total_requests, 2);
    assert_eq!(snapshot.summary.success_requests, 2);
    assert_eq!(snapshot.summary.error_requests, 0);
    assert_eq!(snapshot.summary.cost_nano_usd, 0);
    assert_eq!(snapshot.summary.total_tokens, 35);
    assert_eq!(snapshot.summary.usage.cache_read_tokens, 6);
    assert_eq!(snapshot.summary.usage.cache_write_tokens, 0);
    assert_eq!(snapshot.summary.avg_latency_ms, 35);
    assert_eq!(snapshot.summary.median_latency_ms, 35);

    assert_eq!(snapshot.providers.len(), 2);
    assert_eq!(snapshot.providers[0].provider, "openai");
    assert_eq!(snapshot.providers[0].requests, 1);

    assert_eq!(snapshot.recent.len(), 2);
    assert_eq!(snapshot.recent[0].upstream_id, "alpha");
    assert_eq!(snapshot.recent[0].account_id, None);
    assert_eq!(
        snapshot.recent[1].account_id.as_deref(),
        Some("codex-a.json")
    );
    assert_eq!(snapshot.recent[1].output_tokens, Some(20));
    assert_eq!(snapshot.recent[1].cost_nano_usd, None);
    assert!(
        snapshot
            .series
            .iter()
            .map(|point| point.total_requests)
            .sum::<u64>()
            >= 1
    );

    assert_eq!(snapshot.upstreams.len(), 2);
    assert!(snapshot
        .upstreams
        .iter()
        .any(|item| item.upstream_id == "alpha" && item.requests == 2));
    assert!(snapshot
        .upstreams
        .iter()
        .any(|item| item.upstream_id == "beta" && item.requests == 1));
}

#[tokio::test]
async fn channel_snapshot_combines_time_model_and_pagination_across_account_identities() {
    let pool = setup_test_db().await;
    // 同渠道历史账户和无账户请求都计入统计；分页只限制 recent，不截断聚合。
    sqlx::query(
        r#"
        INSERT INTO request_logs (
          ts_ms, path, provider, upstream_id, account_id, model, stream, status,
          input_tokens, output_tokens, total_tokens, latency_ms
        ) VALUES
          (100, '/test', 'openai', 'alpha', 'old-account', 'gpt-5.4', 0, 200, 7, 3, 10, 10),
          (110, '/test', 'openai', 'alpha', 'current-account', 'gpt-5.4', 0, 200, 7, 3, 10, 20),
          (120, '/test', 'openai', 'alpha', NULL, 'gpt-5.4', 0, 200, 7, 3, 10, 30),
          (130, '/test', 'openai', 'beta', NULL, 'gpt-5.4', 0, 200, 70, 30, 100, 90),
          (140, '/test', 'openai', 'alpha', NULL, 'other-model', 0, 200, 70, 30, 100, 90),
          (99, '/test', 'openai', 'alpha', NULL, 'outside-before', 0, 200, 70, 30, 100, 90),
          (201, '/test', 'openai', 'alpha', NULL, 'outside-after', 0, 200, 70, 30, 100, 90)
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert channel filter regression rows");

    let snapshot = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: Some(100),
            to_ts_ms: Some(200),
        },
        Some(1),
        Some(String::from("alpha")),
        Some(String::from("gpt-5.4")),
    )
    .await
    .expect("read channel snapshot");

    assert_eq!(snapshot.summary.total_requests, 3);
    assert_eq!(snapshot.summary.total_tokens, 30);
    assert_eq!(snapshot.summary.avg_latency_ms, 20);
    assert_eq!(snapshot.summary.median_latency_ms, 20);
    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].requests, 3);
    assert_eq!(snapshot.models.len(), 1);
    assert_eq!(snapshot.models[0].requests, 3);
    assert_eq!(
        snapshot
            .series
            .iter()
            .map(|point| point.total_requests)
            .sum::<u64>(),
        3
    );
    assert_eq!(snapshot.model_options, ["other-model", "gpt-5.4"]);
    assert_eq!(snapshot.recent.len(), 2);
    assert_eq!(snapshot.recent[0].ts_ms, 110);
    assert_eq!(
        snapshot.recent[0].account_id.as_deref(),
        Some("current-account")
    );
    assert_eq!(
        snapshot.recent[1].account_id.as_deref(),
        Some("old-account")
    );
}

#[tokio::test]
async fn read_snapshot_sums_logged_costs_and_returns_recent_pricing_fields() {
    let pool = setup_test_db().await;
    insert_priced_request(
        &pool,
        100,
        "alpha",
        Some(1_210_000_000),
        Some("2026-05-02.openai-openrouter-v1"),
        Some("gpt-5.5"),
        Some("short"),
    )
    .await;
    insert_priced_request(
        &pool,
        200,
        "alpha",
        Some(4_325_000_000),
        Some("2026-05-02.openai-openrouter-v1"),
        Some("gpt-5.4"),
        Some("long"),
    )
    .await;
    insert_priced_request(
        &pool,
        300,
        "beta",
        Some(42),
        Some("other"),
        Some("gpt-5.4-mini"),
        None,
    )
    .await;

    let snapshot = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        Some(String::from("alpha")),
        None,
    )
    .await
    .unwrap();

    assert_eq!(snapshot.summary.total_requests, 2);
    assert_eq!(snapshot.summary.cost_nano_usd, 5_535_000_000);
    assert_eq!(snapshot.recent.len(), 2);
    assert_eq!(snapshot.recent[0].cost_nano_usd, Some(4_325_000_000));
    assert_eq!(
        snapshot.recent[0].pricing_version.as_deref(),
        Some("2026-05-02.openai-openrouter-v1")
    );
    assert_eq!(snapshot.recent[0].pricing_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(
        snapshot.recent[0].pricing_context_tier.as_deref(),
        Some("long")
    );
    assert_eq!(snapshot.recent[1].cost_nano_usd, Some(1_210_000_000));
    assert_eq!(snapshot.recent[1].pricing_model.as_deref(), Some("gpt-5.5"));

    // insert_priced_request 固定 model=gpt-5.4，两笔应合并到同一模型桶。
    assert_eq!(snapshot.models.len(), 1);
    assert_eq!(snapshot.models[0].model, "gpt-5.4");
    assert_eq!(snapshot.models[0].requests, 2);
    assert_eq!(snapshot.models[0].total_tokens, 300);
    assert_eq!(snapshot.models[0].cost_nano_usd, 5_535_000_000);
    assert_eq!(snapshot.providers[0].cost_nano_usd, 5_535_000_000);
    assert!(
        snapshot
            .series
            .iter()
            .map(|point| point.cost_nano_usd)
            .sum::<u64>()
            >= 5_535_000_000
    );
}

#[tokio::test]
async fn read_snapshot_excludes_non_billable_upstream_attempts() {
    let pool = setup_test_db().await;
    sqlx::query(
        r#"
INSERT INTO request_logs (
  ts_ms, path, provider, upstream_id, model, stream, status,
  input_tokens, output_tokens, total_tokens, latency_ms, cost_nano_usd,
  client_request_id, attempt_index, is_billable
) VALUES
  (100, '/v1/responses', 'codex', 'account-a', 'gpt-5', 0, 401, 11, 1, 12, 10, 1100, 'billing-test', 0, 0),
  (200, '/v1/responses', 'codex', 'account-b', 'gpt-5', 0, 200, 22, 2, 24, 20, 2200, 'billing-test', 1, 1)
"#,
    )
    .execute(&pool)
    .await
    .expect("insert attempt rows");

    let snapshot = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        None,
    )
    .await
    .expect("read billable snapshot");

    assert_eq!(snapshot.summary.total_requests, 1);
    assert_eq!(snapshot.summary.total_tokens, 24);
    assert_eq!(snapshot.summary.cost_nano_usd, 2200);
    assert_eq!(snapshot.recent.len(), 1);
    assert_eq!(snapshot.recent[0].upstream_id, "account-b");
}

#[tokio::test]
async fn read_snapshot_groups_models_with_fallback_and_filters() {
    let pool = setup_test_db().await;

    // 显式 model
    sqlx::query(
        r#"
        INSERT INTO request_logs (
          ts_ms, path, provider, upstream_id, model, mapped_model, stream, status,
          input_tokens, output_tokens, total_tokens, latency_ms, cost_nano_usd
        ) VALUES
          (100, '/test', 'openai', 'alpha', 'gpt-5.4', 'gpt-5.4-mapped', 0, 200, 100, 50, 150, 30, 1000),
          (110, '/test', 'openai', 'alpha', 'gpt-5.4', NULL, 0, 200, 200, 50, 250, 30, 2000),
          (120, '/test', 'anthropic', 'beta', 'claude-4', 'claude-4', 0, 200, 10, 5, 15, 30, 100)
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert model rows");

    // 空 model，回退 mapped_model
    sqlx::query(
        r#"
        INSERT INTO request_logs (
          ts_ms, path, provider, upstream_id, model, mapped_model, stream, status,
          input_tokens, output_tokens, total_tokens, latency_ms, cost_nano_usd
        ) VALUES
          (130, '/test', 'openai', 'alpha', '', 'fallback-model', 0, 200, 40, 10, 50, 30, 500),
          (140, '/test', 'openai', 'alpha', NULL, NULL, 0, 200, 1, 1, 2, 30, 10)
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert fallback model rows");

    let all = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        None,
    )
    .await
    .unwrap();

    // tokens: gpt-5.4=400, fallback=50, claude=15, unknown=2
    assert_eq!(all.models.len(), 4);
    assert_eq!(all.models[0].model, "gpt-5.4");
    assert_eq!(all.models[0].requests, 2);
    assert_eq!(all.models[0].total_tokens, 400);
    assert_eq!(all.models[0].input_tokens, 300);
    assert_eq!(all.models[0].output_tokens, 100);
    assert_eq!(all.models[0].cost_nano_usd, 3000);
    assert_eq!(all.models[1].model, "fallback-model");
    assert_eq!(all.models[1].total_tokens, 50);
    assert_eq!(all.models[2].model, "claude-4");
    assert_eq!(all.models[3].model, "(unknown)");
    assert_eq!(all.models[3].total_tokens, 2);

    let filtered = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        Some(String::from("alpha")),
        None,
    )
    .await
    .unwrap();

    assert_eq!(filtered.models.len(), 3);
    assert!(filtered.models.iter().all(|item| item.model != "claude-4"));
    assert_eq!(filtered.models[0].model, "gpt-5.4");
    // 无上游筛选时，时间范围内全部模型均出现在选项中。
    assert!(all.model_options.contains(&String::from("claude-4")));
    assert!(all.model_options.contains(&String::from("gpt-5.4")));
    assert!(all.model_options.contains(&String::from("fallback-model")));
    assert!(all.model_options.contains(&String::from("(unknown)")));
    // 上游筛选会收窄模型选项，但不影响“全部模型”以外的切换能力。
    assert_eq!(filtered.model_options.len(), 3);
    assert!(!filtered.model_options.iter().any(|item| item == "claude-4"));
}

#[tokio::test]
async fn read_snapshot_filters_by_model_key_and_keeps_model_options() {
    let pool = setup_test_db().await;
    sqlx::query(
        r#"
        INSERT INTO request_logs (
          ts_ms, path, provider, upstream_id, model, mapped_model, stream, status,
          input_tokens, output_tokens, total_tokens, latency_ms, cost_nano_usd
        ) VALUES
          (100, '/test', 'openai', 'alpha', 'gpt-5.4', 'gpt-5.4-mapped', 0, 200, 100, 50, 150, 30, 1000),
          (110, '/test', 'openai', 'alpha', '', 'fallback-model', 0, 200, 40, 10, 50, 30, 500),
          (120, '/test', 'anthropic', 'beta', 'claude-4', 'claude-4', 0, 200, 10, 5, 15, 30, 100),
          (130, '/test', 'openai', 'alpha', NULL, NULL, 0, 200, 1, 1, 2, 30, 10)
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert model filter rows");

    // 显式 model
    let by_model = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        Some(String::from("gpt-5.4")),
    )
    .await
    .expect("filter by model");

    assert_eq!(by_model.summary.total_requests, 1);
    assert_eq!(by_model.summary.total_tokens, 150);
    assert_eq!(by_model.models.len(), 1);
    assert_eq!(by_model.models[0].model, "gpt-5.4");
    assert_eq!(by_model.recent.len(), 1);
    assert_eq!(by_model.recent[0].model.as_deref(), Some("gpt-5.4"));
    // 选项不受当前 model 筛选影响，仍可切换到其它模型。
    assert_eq!(by_model.model_options.len(), 4);
    assert!(by_model
        .model_options
        .iter()
        .any(|item| item == "fallback-model"));
    assert!(by_model.model_options.iter().any(|item| item == "claude-4"));

    // 空 model 回退 mapped_model
    let by_fallback = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        Some(String::from("fallback-model")),
    )
    .await
    .expect("filter by fallback model");
    assert_eq!(by_fallback.summary.total_requests, 1);
    assert_eq!(by_fallback.summary.total_tokens, 50);

    // unknown key
    let by_unknown = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        Some(String::from("(unknown)")),
    )
    .await
    .expect("filter by unknown model");
    assert_eq!(by_unknown.summary.total_requests, 1);
    assert_eq!(by_unknown.summary.total_tokens, 2);

    // 空串视为未筛选
    let empty_model = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        Some(String::from("   ")),
    )
    .await
    .expect("empty model means no filter");
    assert_eq!(empty_model.summary.total_requests, 4);
}

#[tokio::test]
async fn read_snapshot_round_trips_precise_usage_components() {
    let pool = setup_test_db().await;
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
          100, '/responses', 'openai-response', 'alpha', 0, 200,
          121, 13, 80,
          20, 5,
          10, 6,
          4, 3, 134,
          'priority', 30
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert usage request");

    let snapshot = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        None,
    )
    .await
    .expect("read dashboard snapshot");

    assert_eq!(snapshot.summary.usage.uncached_input_tokens, 80);
    assert_eq!(snapshot.summary.usage.cache_read_tokens, 20);
    assert_eq!(snapshot.summary.usage.cache_write_tokens, 5);
    assert_eq!(snapshot.summary.usage.cache_write_5m_tokens, 10);
    assert_eq!(snapshot.summary.usage.cache_write_1h_tokens, 6);
    assert_eq!(snapshot.summary.usage.cached_tokens, 41);
    assert_eq!(snapshot.summary.usage.image_input_tokens, 4);
    assert_eq!(snapshot.summary.usage.image_output_tokens, 3);

    let recent = snapshot.recent.first().expect("recent request");
    assert_eq!(recent.cached_tokens, Some(41));
    assert_eq!(recent.uncached_input_tokens, Some(80));
    assert_eq!(recent.cache_read_tokens, Some(20));
    assert_eq!(recent.cache_write_tokens, Some(5));
    assert_eq!(recent.cache_write_5m_tokens, Some(10));
    assert_eq!(recent.cache_write_1h_tokens, Some(6));
    assert_eq!(recent.image_input_tokens, Some(4));
    assert_eq!(recent.image_output_tokens, Some(3));
    assert_eq!(recent.service_tier.as_deref(), Some("priority"));
}

#[tokio::test]
async fn read_snapshot_returns_recent_client_ip() {
    let pool = setup_test_db().await;
    insert_request_with_client_ip(&pool, 100, "203.0.113.5").await;

    let snapshot = read_snapshot(
        &pool,
        DashboardRange {
            from_ts_ms: None,
            to_ts_ms: None,
        },
        Some(0),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(snapshot.recent.len(), 1);
    assert_eq!(snapshot.recent[0].client_ip.as_deref(), Some("203.0.113.5"));
    assert_eq!(snapshot.recent[0].cached_tokens, None);
}
