use super::*;
use axum::body::Body;
use std::time::Duration;

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(future)
}

#[test]
fn replayable_body_small_stays_in_memory() {
    run_async(async {
        let input = vec![b'a'; 16];
        let body = ReplayableBody::from_body(Body::from(input.clone()))
            .await
            .expect("spool body");

        assert!(!body.is_temp_file());
        let bytes = body
            .read_bytes_if_small(1024)
            .await
            .expect("read bytes")
            .expect("bytes present");
        assert_eq!(bytes.as_ref(), input.as_slice());
    });
}

#[test]
fn replayable_body_large_spools_to_temp_file_and_cleans_up() {
    run_async(async {
        let input = vec![b'b'; IN_MEMORY_LIMIT_BYTES + 1];
        let body = ReplayableBody::from_body(Body::from(input.clone()))
            .await
            .expect("spool body");

        assert!(body.is_temp_file());
        let path = body.temp_path().expect("temp path");
        assert!(
            std::fs::metadata(&path).is_ok(),
            "temp file should exist: {path:?}"
        );

        let bytes = body
            .read_bytes_if_small(IN_MEMORY_LIMIT_BYTES + 32)
            .await
            .expect("read bytes")
            .expect("bytes present");
        assert_eq!(bytes.as_ref(), input.as_slice());

        drop(body);
        for _ in 0..50 {
            if std::fs::metadata(&path).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("temp file should be removed on drop: {path:?}");
    });
}

#[test]
fn replayable_body_clone_keeps_temp_file_until_last_drop() {
    run_async(async {
        let input = vec![b'c'; IN_MEMORY_LIMIT_BYTES + 1];
        let body = ReplayableBody::from_body(Body::from(input))
            .await
            .expect("spool body");
        let clone = body.clone();

        let path = body.temp_path().expect("temp path");
        drop(body);
        assert!(
            std::fs::metadata(&path).is_ok(),
            "temp file should survive while clone exists: {path:?}"
        );

        drop(clone);
        for _ in 0..50 {
            if std::fs::metadata(&path).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("temp file should be removed after last clone drops: {path:?}");
    });
}
