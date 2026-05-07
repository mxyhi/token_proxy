use super::*;
use axum::body::Body;

const LEGACY_TEMP_FILE_THRESHOLD_BYTES: usize = 512 * 1024;

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
fn replayable_body_large_stays_in_memory_and_replays() {
    run_async(async {
        let input = vec![b'b'; LEGACY_TEMP_FILE_THRESHOLD_BYTES + 1];
        let body = ReplayableBody::from_body(Body::from(input.clone()))
            .await
            .expect("spool body");

        assert!(!body.is_temp_file());

        let bytes = body
            .read_bytes_if_small(LEGACY_TEMP_FILE_THRESHOLD_BYTES + 32)
            .await
            .expect("read bytes")
            .expect("bytes present");
        assert_eq!(bytes.as_ref(), input.as_slice());
    });
}

#[test]
fn replayable_body_clone_replays_after_original_drop() {
    run_async(async {
        let input = vec![b'c'; LEGACY_TEMP_FILE_THRESHOLD_BYTES + 1];
        let body = ReplayableBody::from_body(Body::from(input.clone()))
            .await
            .expect("spool body");
        let clone = body.clone();

        drop(body);

        let bytes = clone
            .read_bytes_if_small(LEGACY_TEMP_FILE_THRESHOLD_BYTES + 32)
            .await
            .expect("read bytes")
            .expect("bytes present");
        assert_eq!(bytes.as_ref(), input.as_slice());
    });
}
