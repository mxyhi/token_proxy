use super::*;
use futures_util::StreamExt;

#[tokio::test]
async fn idle_timeout_returns_error() {
    let upstream = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let mut stream = with_idle_timeout(upstream, Duration::from_millis(10));

    let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("test timeout")
        .expect("item")
        .expect_err("timeout error");

    assert!(matches!(item, UpstreamStreamError::IdleTimeout(_)));
}

#[tokio::test]
async fn passes_through_success_chunks() {
    let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(
        Bytes::from_static(b"hello"),
    )]);
    let mut stream = with_idle_timeout(upstream, Duration::from_secs(1));

    let first = stream.next().await.expect("first").expect("ok");
    assert_eq!(first, Bytes::from_static(b"hello"));

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn propagates_upstream_errors() {
    let upstream = futures_util::stream::iter(vec![Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "boom",
    ))]);
    let mut stream = with_idle_timeout(upstream, Duration::from_secs(1));

    let err = stream.next().await.expect("first").expect_err("err");
    assert!(matches!(err, UpstreamStreamError::Upstream(_)));
}
