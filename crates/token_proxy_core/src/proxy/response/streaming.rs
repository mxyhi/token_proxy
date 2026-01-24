use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc};

use super::{
    PROVIDER_ANTHROPIC, PROVIDER_ANTIGRAVITY, PROVIDER_CODEX, PROVIDER_GEMINI, PROVIDER_OPENAI,
    PROVIDER_OPENAI_RESPONSES,
};
use super::super::log::{build_log_entry, LogContext, LogWriter};
use super::super::model;
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;

pub(super) fn stream_with_logging<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let collector = SseUsageCollector::new();
    let parser = SseEventParser::new();
    try_unfold(
        (upstream, collector, parser, log, context, token_tracker),
        |(mut upstream, mut collector, mut parser, log, mut context, token_tracker)| async move {
            match upstream.next().await {
                Some(Ok(chunk)) => {
                    if context.ttfb_ms.is_none() {
                        context.ttfb_ms = Some(context.start.elapsed().as_millis());
                    }
                    collector.push_chunk(&chunk);
                    let provider = context.provider.as_str();
                    let mut texts = Vec::new();
                    parser.push_chunk(&chunk, |data| {
                        if let Some(text) = extract_stream_text(provider, &data) {
                            texts.push(text);
                        }
                    });
                    for text in texts {
                        token_tracker.add_output_text(&text).await;
                    }
                    Ok(Some((chunk, (upstream, collector, parser, log, context, token_tracker))))
                }
                Some(Err(err)) => {
                    let provider = context.provider.as_str();
                    let mut texts = Vec::new();
                    parser.finish(|data| {
                        if let Some(text) = extract_stream_text(provider, &data) {
                            texts.push(text);
                        }
                    });
                    for text in texts {
                        token_tracker.add_output_text(&text).await;
                    }
                    let entry = build_log_entry(&context, collector.finish(), None);
                    log.clone().write_detached(entry);
                    Err(std::io::Error::new(std::io::ErrorKind::Other, err))
                }
                None => {
                    let provider = context.provider.as_str();
                    let mut texts = Vec::new();
                    parser.finish(|data| {
                        if let Some(text) = extract_stream_text(provider, &data) {
                            texts.push(text);
                        }
                    });
                    for text in texts {
                        token_tracker.add_output_text(&text).await;
                    }
                    let entry = build_log_entry(&context, collector.finish(), None);
                    log.clone().write_detached(entry);
                    Ok(None)
                }
            }
        },
    )
}

pub(super) fn stream_with_logging_and_model_override<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    model_override: String,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = ModelOverrideStreamState::new(upstream, context, log, model_override, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

struct ModelOverrideStreamState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    model_override: String,
    upstream_ended: bool,
    logged: bool,
}

impl<S, E> ModelOverrideStreamState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(
        upstream: S,
        context: LogContext,
        log: Arc<LogWriter>,
        model_override: String,
        token_tracker: RequestTokenTracker,
    ) -> Self {
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            context,
            token_tracker,
            out: VecDeque::new(),
            model_override,
            upstream_ended: false,
            logged: false,
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        loop {
            if let Some(next) = self.out.pop_front() {
                return Ok(Some((next, self)));
            }
            if self.upstream_ended {
                self.log_usage_once();
                return Ok(None);
            }

            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    if self.context.ttfb_ms.is_none() {
                        self.context.ttfb_ms = Some(self.context.start.elapsed().as_millis());
                    }
                    self.collector.push_chunk(&chunk);
                    let mut events = Vec::new();
                    self.parser.push_chunk(&chunk, |data| events.push(data));
                    let mut texts = Vec::new();
                    for data in events {
                        if let Some(text) = extract_stream_text(&self.context.provider, &data) {
                            texts.push(text);
                        }
                        self.push_event_output(&data);
                    }
                    for text in texts {
                        self.token_tracker.add_output_text(&text).await;
                    }
                }
                Some(Err(err)) => {
                    self.log_usage_once();
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
                }
                None => {
                    self.upstream_ended = true;
                    let mut events = Vec::new();
                    self.parser.finish(|data| events.push(data));
                    let mut texts = Vec::new();
                    for data in events {
                        if let Some(text) = extract_stream_text(&self.context.provider, &data) {
                            texts.push(text);
                        }
                        self.push_event_output(&data);
                    }
                    for text in texts {
                        self.token_tracker.add_output_text(&text).await;
                    }
                }
            }
        }
    }

    fn push_event_output(&mut self, data: &str) {
        let output = rewrite_sse_data(data, &self.model_override);
        self.out.push_back(Bytes::from(format!("data: {output}\n\n")));
    }

    fn log_usage_once(&mut self) {
        if self.logged {
            return;
        }
        let entry = build_log_entry(&self.context, self.collector.finish(), None);
        self.log.clone().write_detached(entry);
        self.logged = true;
    }
}

fn rewrite_sse_data(data: &str, model_override: &str) -> String {
    if data == "[DONE]" {
        return data.to_string();
    }
    let bytes = Bytes::copy_from_slice(data.as_bytes());
    model::rewrite_response_model(&bytes, model_override)
        .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
        .unwrap_or_else(|| data.to_string())
}

fn extract_stream_text(provider: &str, data: &str) -> Option<String> {
    if data == "[DONE]" {
        return None;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return None;
    };

    match provider {
        PROVIDER_OPENAI | PROVIDER_OPENAI_RESPONSES | PROVIDER_CODEX => {
            extract_openai_stream_text(&value)
        }
        PROVIDER_ANTHROPIC => extract_anthropic_stream_text(&value),
        PROVIDER_GEMINI | PROVIDER_ANTIGRAVITY => extract_gemini_stream_text(&value),
        _ => None,
    }
    .or_else(|| extract_fallback_stream_text(&value))
}

fn extract_openai_stream_text(value: &Value) -> Option<String> {
    let delta = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str);
    if let Some(delta) = delta {
        return Some(delta.to_string());
    }
    let event_type = value.get("type").and_then(Value::as_str)?;
    if event_type.ends_with("output_text.delta") {
        return value
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| text.to_string());
    }
    None
}

fn extract_anthropic_stream_text(value: &Value) -> Option<String> {
    if let Some(delta) = value.get("delta") {
        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            return Some(text.to_string());
        }
        if let Some(text) = delta.as_str() {
            return Some(text.to_string());
        }
    }
    value
        .get("content_block")
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

fn extract_gemini_stream_text(value: &Value) -> Option<String> {
    let candidates = value.get("candidates").and_then(Value::as_array)?;
    for candidate in candidates {
        if let Some(content) = candidate.get("content") {
            if let Some(parts) = content.get("parts").and_then(Value::as_array) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }
    None
}

fn extract_fallback_stream_text(value: &Value) -> Option<String> {
    value
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| value.get("text").and_then(Value::as_str))
        .map(|text| text.to_string())
}
