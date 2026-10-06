use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};

use crate::proxy::sse::SseEventParser;

pub(crate) fn chat_stream_error(
    value: &Value,
) -> Option<token_proxy_protocol::responses_error::ResponsesStreamError> {
    if value.get("error").is_some_and(|error| !error.is_null()) && value.get("type").is_none() {
        let mut event = value.clone();
        event["type"] = Value::String("error".to_string());
        token_proxy_protocol::responses_error::responses_stream_error(&event)
    } else {
        token_proxy_protocol::responses_error::responses_stream_error(value)
    }
}

pub(crate) const CHAT_TRUNCATED_ERROR: &str =
    "Upstream Chat stream truncated before a valid finish_reason.";

pub(crate) fn valid_finish_reason(reason: &str) -> bool {
    matches!(
        reason,
        "stop" | "tool_calls" | "function_call" | "length" | "max_tokens" | "content_filter"
    )
}

/// 保留原始 SSE 帧，只在完整终止帧交付前检查 finish_reason；拆分的 [DONE] 也不能提前泄露成功终态。
pub(crate) fn with_chat_finish_validation<E>(
    upstream: impl futures_util::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    enabled: bool,
) -> futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    if !enabled {
        return upstream
            .map(|chunk| chunk.map_err(std::io::Error::other))
            .boxed();
    }
    let state = ChatStreamState {
        upstream,
        pending: Vec::new(),
        out: VecDeque::new(),
        choices: BTreeSet::new(),
        finished: BTreeSet::new(),
        ended: false,
        error: None,
    };
    try_unfold(state, |mut state| async move {
        loop {
            if let Some(chunk) = state.out.pop_front() {
                return Ok(Some((chunk, state)));
            }
            if let Some(error) = state.error.take() {
                return Err(error);
            }
            if state.ended {
                return Ok(None);
            }
            match state.upstream.next().await {
                Some(Ok(chunk)) => {
                    state.pending.extend_from_slice(&chunk);
                    while let Some(end) = frame_end(&state.pending) {
                        let frame = state.pending.drain(..end).collect();
                        state.push_frame(frame);
                        if state.ended {
                            break;
                        }
                    }
                }
                item => {
                    if !state.pending.is_empty() {
                        let mut frame = std::mem::take(&mut state.pending);
                        frame.extend_from_slice(b"\n\n");
                        state.push_frame(frame);
                    }
                    state.ended = true;
                    if state.error.is_none() {
                        state.error = match item {
                            Some(Err(error)) => Some(std::io::Error::other(error)),
                            None if !state.complete() => Some(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                CHAT_TRUNCATED_ERROR,
                            )),
                            _ => None,
                        };
                    }
                }
            }
        }
    })
    .boxed()
}

struct ChatStreamState<S> {
    upstream: S,
    pending: Vec<u8>,
    out: VecDeque<Bytes>,
    choices: BTreeSet<u64>,
    finished: BTreeSet<u64>,
    ended: bool,
    error: Option<std::io::Error>,
}

impl<S> ChatStreamState<S> {
    fn complete(&self) -> bool {
        !self.choices.is_empty() && self.choices == self.finished
    }

    fn push_frame(&mut self, frame: Vec<u8>) {
        let mut parser = SseEventParser::new();
        let mut events = Vec::new();
        parser.push_chunk(&frame, |data| events.push(data));
        parser.finish(|data| events.push(data));
        for data in events {
            if data.trim() == "[DONE]" {
                self.ended = true;
                if !self.complete() {
                    self.error = Some(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        CHAT_TRUNCATED_ERROR,
                    ));
                    return;
                }
            }
            if let Ok(value) = serde_json::from_str::<Value>(&data) {
                if chat_stream_error(&value).is_some() {
                    // 显式上游错误本身就是失败终态，继续 EOF 校验会掩盖真实原因。
                    self.ended = true;
                }
                if let Some(choices) = value["choices"].as_array() {
                    for (offset, choice) in choices.iter().enumerate() {
                        let index = choice["index"].as_u64().unwrap_or(offset as u64);
                        self.choices.insert(index);
                        if choice["finish_reason"]
                            .as_str()
                            .is_some_and(valid_finish_reason)
                        {
                            self.finished.insert(index);
                        }
                    }
                }
            }
        }
        self.out.push_back(Bytes::from(frame));
    }
}

fn frame_end(bytes: &[u8]) -> Option<usize> {
    for index in 0..bytes.len().saturating_sub(1) {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some(index + 4);
        }
        if bytes[index..].starts_with(b"\n\n") || bytes[index..].starts_with(b"\r\r") {
            return Some(index + 2);
        }
    }
    None
}
