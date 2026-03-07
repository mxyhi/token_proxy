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
                write!(
                    f,
                    "Upstream stream idle timeout after {}s.",
                    duration.as_secs()
                )
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

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "upstream_stream.test.rs"]
mod tests;
