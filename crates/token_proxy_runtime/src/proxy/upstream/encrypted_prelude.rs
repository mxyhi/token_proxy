use axum::body::Bytes;
use futures_util::{stream, StreamExt};
use reqwest::ResponseBuilderExt;
use std::time::{Duration, Instant};
use token_proxy_protocol::{
    responses_error::{ResponsesPreludeDecision, ResponsesPreludeInspector},
    sse::SseEventParser,
};

/// 在既有首输出预算内观察，不吞掉任何原始字节。业务事件/未知事件立即释放，之后绝不修复。
pub(super) async fn inspect_encrypted_prelude(
    response: reqwest::Response,
    start: Instant,
    budget: Duration,
) -> (reqwest::Response, Option<Vec<u8>>) {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let url = response.url().clone();
    let mut upstream = response.bytes_stream().boxed();
    let mut buffered = Vec::new();
    let mut bytes = 0usize;
    let mut parser = SseEventParser::new();
    let mut inspector = ResponsesPreludeInspector::new();
    let mut rejection = None;
    let deadline = tokio::time::Instant::from_std(start + budget);
    'read: loop {
        let next = match tokio::time::timeout_at(deadline, upstream.next()).await {
            Ok(next) => next,
            Err(_) => break,
        };
        match next {
            Some(Ok(chunk)) => {
                bytes = bytes.saturating_add(chunk.len());
                buffered.push(Ok::<Bytes, reqwest::Error>(chunk.clone()));
                // 预算超限交回原流水线，不扫描大正文或把大 chunk 当作可恢复 prelude。
                if bytes >= 1024 * 1024 {
                    break;
                }
                let mut events = Vec::new();
                if parser.push_chunk(&chunk, |data| events.push(data)).is_err() {
                    break;
                }
                let (stop, error) = inspect_events(events, &mut inspector);
                if stop {
                    rejection = error;
                    break 'read;
                }
            }
            Some(Err(error)) => {
                buffered.push(Err(error));
                break;
            }
            None => {
                let mut events = Vec::new();
                if parser.finish(|data| events.push(data)).is_ok() {
                    rejection = inspect_events(events, &mut inspector).1;
                }
                break;
            }
        }
    }
    let body = reqwest::Body::wrap_stream(stream::iter(buffered).chain(upstream));
    let mut rebuilt = axum::http::Response::builder()
        .status(status)
        .version(version)
        .url(url)
        .body(body)
        .expect("existing response metadata is valid");
    *rebuilt.headers_mut() = headers;
    (reqwest::Response::from(rebuilt), rejection)
}

fn inspect_events(
    events: Vec<String>,
    inspector: &mut ResponsesPreludeInspector,
) -> (bool, Option<Vec<u8>>) {
    for data in events {
        let framed = format!("data: {data}\n\n");
        match inspector.inspect_chunk(framed.as_bytes()) {
            ResponsesPreludeDecision::Pending => {}
            ResponsesPreludeDecision::TerminalError(error)
            | ResponsesPreludeDecision::RetryableError(error) => {
                let rejection = (error.code.as_ref().and_then(serde_json::Value::as_str)
                    == Some("invalid_encrypted_content"))
                .then(|| data.into_bytes());
                return (true, rejection);
            }
            ResponsesPreludeDecision::ReadyForPassThrough => return (true, None),
        }
    }
    (false, None)
}
