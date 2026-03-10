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

    sqlx::query(
        r#"
        CREATE TABLE request_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_ms INTEGER NOT NULL,
            path TEXT NOT NULL,
            provider TEXT NOT NULL,
            upstream_id TEXT NOT NULL,
            model TEXT,
            mapped_model TEXT,
            stream INTEGER NOT NULL,
            status INTEGER NOT NULL,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            cached_tokens INTEGER,
            usage_json TEXT,
            upstream_request_id TEXT,
            request_headers TEXT,
            request_body TEXT,
            response_error TEXT,
            latency_ms INTEGER NOT NULL
        );
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create table");

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

async fn insert_request(
    pool: &SqlitePool,
    ts_ms: i64,
    provider: &str,
    upstream_id: &str,
    status: i64,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cached_tokens: Option<i64>,
    latency_ms: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO request_logs (
            ts_ms,
            path,
            provider,
            upstream_id,
            stream,
            status,
            input_tokens,
            output_tokens,
            total_tokens,
            cached_tokens,
            latency_ms
        )
        VALUES (?, '/test', ?, ?, 0, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(ts_ms)
    .bind(provider)
    .bind(upstream_id)
    .bind(status)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(total_tokens)
    .bind(cached_tokens)
    .bind(latency_ms)
    .execute(pool)
    .await
    .expect("Failed to insert request");
}

#[tokio::test]
async fn median_latency_empty_table_returns_zero() {
    let pool = setup_test_db().await;
    let result = query_median_latency(&pool, None, None, None)
        .await
        .unwrap();
    assert_eq!(result, 0, "Empty table should return 0");
}

#[tokio::test]
async fn median_latency_single_value() {
    let pool = setup_test_db().await;
    insert_latency(&pool, 100).await;

    let result = query_median_latency(&pool, None, None, None)
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

    let result = query_median_latency(&pool, None, None, None)
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

    let result = query_median_latency(&pool, None, None, None)
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

    let result = query_median_latency(&pool, None, None, None)
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
    let result = query_median_latency(&pool, Some(150), Some(250), None)
        .await
        .unwrap();
    assert_eq!(result, 100, "Should filter by time range");

    // 查询所有数据，中位数应为 100
    let result_all = query_median_latency(&pool, None, None, None)
        .await
        .unwrap();
    assert_eq!(result_all, 100, "All data median should be 100");
}

#[tokio::test]
async fn read_snapshot_filters_by_upstream_and_keeps_all_upstream_options() {
    let pool = setup_test_db().await;
    insert_request(
        &pool,
        100,
        "openai",
        "alpha",
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
        200,
        "anthropic",
        "beta",
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
    )
    .await
    .unwrap();

    assert_eq!(snapshot.summary.total_requests, 1);
    assert_eq!(snapshot.summary.success_requests, 1);
    assert_eq!(snapshot.summary.error_requests, 0);
    assert_eq!(snapshot.summary.total_tokens, 30);
    assert_eq!(snapshot.summary.cached_tokens, 5);
    assert_eq!(snapshot.summary.avg_latency_ms, 30);
    assert_eq!(snapshot.summary.median_latency_ms, 30);

    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].provider, "openai");
    assert_eq!(snapshot.providers[0].requests, 1);

    assert_eq!(snapshot.recent.len(), 1);
    assert_eq!(snapshot.recent[0].upstream_id, "alpha");
    assert!(snapshot
        .series
        .iter()
        .map(|point| point.total_requests)
        .sum::<u64>()
        >= 1);

    assert_eq!(snapshot.upstreams.len(), 2);
    assert!(snapshot
        .upstreams
        .iter()
        .any(|item| item.upstream_id == "alpha" && item.provider == "openai"));
    assert!(snapshot
        .upstreams
        .iter()
        .any(|item| item.upstream_id == "beta" && item.provider == "anthropic"));
}
