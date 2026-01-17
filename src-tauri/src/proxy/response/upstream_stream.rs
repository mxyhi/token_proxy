use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use std::{error::Error, fmt, time::Duration};

use crate::proxy::UPSTREAM_NO_DATA_TIMEOUT;

#[derive(Debug)]
pub(crate) enum UpstreamStreamError<E> {
    IdleTimeout(Duration),
    Upstream(E),
}

impl<E: fmt::Display> fmt::Display for UpstreamStreamError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdleTimeout(duration) => {
                write!(f, "Upstream stream idle timeout after {}s.", duration.as_secs())
            }
            Self::Upstream(err) => write!(f, "{err}"),
        }
    }
}

impl<E: Error + 'static> Error for UpstreamStreamError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IdleTimeout(_) => None,
            Self::Upstream(err) => Some(err),
        }
    }
}

pub(super) fn with_idle_timeout<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
) -> futures_util::stream::BoxStream<'static, Result<Bytes, UpstreamStreamError<E>>>
where
    E: Error + Send + Sync + 'static,
{
    try_unfold(upstream, |mut upstream| async move {
        match tokio::time::timeout(UPSTREAM_NO_DATA_TIMEOUT, upstream.next()).await {
            Ok(Some(Ok(chunk))) => Ok(Some((chunk, upstream))),
            Ok(Some(Err(err))) => Err(UpstreamStreamError::Upstream(err)),
            Ok(None) => Ok(None),
            Err(_) => Err(UpstreamStreamError::IdleTimeout(UPSTREAM_NO_DATA_TIMEOUT)),
        }
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn idle_timeout_returns_error() {
        let upstream = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
        let mut stream = with_idle_timeout(upstream);

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
        let mut stream = with_idle_timeout(upstream);

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
        let mut stream = with_idle_timeout(upstream);

        let err = stream
            .next()
            .await
            .expect("first")
            .expect_err("err");
        assert!(matches!(err, UpstreamStreamError::Upstream(_)));
    }
}
