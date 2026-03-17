use axum::body::Bytes;
use futures_util::StreamExt;
use std::time::Duration;

use super::super::log::LogContext;
use super::upstream_stream::{self, UpstreamStreamError};

pub(super) async fn read_upstream_bytes_with_ttfb(
    upstream_res: reqwest::Response,
    context: &mut LogContext,
    upstream_no_data_timeout: Duration,
) -> Result<Bytes, UpstreamStreamError<reqwest::Error>> {
    let mut upstream =
        upstream_stream::with_idle_timeout(upstream_res.bytes_stream(), upstream_no_data_timeout);
    let mut out = Vec::new();

    while let Some(item) = upstream.next().await {
        let chunk = item?;
        if context.ttfb_ms.is_none() {
            context.ttfb_ms = Some(context.start.elapsed().as_millis());
        }
        out.extend_from_slice(chunk.as_ref());
    }

    Ok(Bytes::from(out))
}
