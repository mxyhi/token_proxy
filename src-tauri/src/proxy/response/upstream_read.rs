use axum::body::Bytes;
use futures_util::StreamExt;

use super::upstream_stream::{self, UpstreamStreamError};
use super::super::log::LogContext;

pub(super) async fn read_upstream_bytes_with_ttfb(
    upstream_res: reqwest::Response,
    context: &mut LogContext,
) -> Result<Bytes, UpstreamStreamError<reqwest::Error>> {
    let mut upstream = upstream_stream::with_idle_timeout(upstream_res.bytes_stream());
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
