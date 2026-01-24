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
