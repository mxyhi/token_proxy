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
        let body = ReplayableBody::from_body(Body::from(input.clone()), input.len())
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
        let body = ReplayableBody::from_body(Body::from(input.clone()), input.len())
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
        let body = ReplayableBody::from_body(Body::from(input.clone()), input.len())
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

#[test]
fn replayable_body_limit_stops_chunked_reads_without_consuming_tail() {
    run_async(async {
        let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = polls.clone();
        let stream = futures_util::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"1234")),
            Ok(Bytes::from_static(b"56789")),
            Err(std::io::Error::other("tail must not be read")),
        ])
        .inspect(move |_| {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        let result = ReplayableBody::from_body(Body::from_stream(stream), 8).await;
        assert!(matches!(
            result,
            Err(RequestBodyError::TooLarge { limit: 8 })
        ));
        assert_eq!(polls.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert!(ReplayableBody::from_body(Body::empty(), 0).await.is_ok());
        assert!(matches!(
            ReplayableBody::from_body(Body::from("x"), 0).await,
            Err(RequestBodyError::TooLarge { limit: 0 })
        ));
    });
}
