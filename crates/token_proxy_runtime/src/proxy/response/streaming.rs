use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use super::super::log::{build_log_entry, LogContext, LogWriter};
use super::super::model;
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;
use super::{
    responses_error::{responses_stream_error, ResponsesStreamError},
    responses_failure,
    sequence::ResponsesEventSequence,
    PROVIDER_ANTHROPIC, PROVIDER_CODEX, PROVIDER_GEMINI, PROVIDER_OPENAI,
    PROVIDER_OPENAI_RESPONSES, PROVIDER_XAI,
};

pub(crate) const STREAM_DROPPED_ERROR: &str = "stream dropped before completion";

pub(super) fn stream_with_logging<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    stream_with_logging_and_semantic_timeout(upstream, context, log, token_tracker, None)
}

pub(super) fn stream_with_logging_and_semantic_timeout<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
    semantic_timeout: Option<Duration>,
) -> futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let upstream = super::upstream_stream::with_chat_finish_validation(
        upstream,
        is_chat_stream(&context.provider, &context.path),
    );
    if openai_stream_semantics(&context.provider, &context.path).responses_events {
        let state = ModelOverrideStreamState::new(
            upstream,
            context,
            log,
            None,
            token_tracker,
            semantic_timeout,
        );
        return try_unfold(state, |state| async move { state.step().await }).boxed();
    }
    let state = LoggingStreamState::new(upstream, context, log, token_tracker, semantic_timeout);
    try_unfold(state, |state| async move { state.step().await }).boxed()
}

struct LoggingStreamState<S> {
    upstream: S,
    collector: SseUsageCollector,
    parser: SseEventParser,
    log: Arc<LogWriter>,
    context: LogContext,
    _token_tracker: RequestTokenTracker,
    logged: bool,
    terminal_seen: bool,
    terminal_error: Option<String>,
    semantic_timeout: Option<Duration>,
    last_semantic_event_at: Instant,
    response_body_buf: super::body_capture::ResponseBodyCapture,
    sequence: ResponsesEventSequence,
}

#[derive(Default)]
struct StreamObservation {
    starts_client_output: bool,
    semantic_event: bool,
    terminal: bool,
    terminal_json: bool,
    saw_done: bool,
    terminal_error: Option<ResponsesStreamError>,
}

impl<S> LoggingStreamState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        let entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        self.response_body_buf.write_log(self.log.clone(), entry);
        self.logged = true;
    }

    fn write_terminal_log_once(&mut self) {
        let response_error = self.terminal_error.take();
        self.write_log_once(response_error);
    }
}

impl<S> Drop for LoggingStreamState<S> {
    fn drop(&mut self) {
        // 流被客户端提前取消时不会再进入 `None/Err` 分支，这里兜底保证日志至少落一行。
        if self.terminal_seen {
            self.write_terminal_log_once();
        } else {
            self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
        }
    }
}

impl<S, E> LoggingStreamState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(
        upstream: S,
        context: LogContext,
        log: Arc<LogWriter>,
        token_tracker: RequestTokenTracker,
        semantic_timeout: Option<Duration>,
    ) -> Self {
        let response_body_buf = super::body_capture::ResponseBodyCapture::new(&context);
        Self {
            upstream,
            collector: SseUsageCollector::new(),
            parser: SseEventParser::new(),
            log,
            context,
            _token_tracker: token_tracker,
            logged: false,
            terminal_seen: false,
            terminal_error: None,
            semantic_timeout,
            last_semantic_event_at: Instant::now(),
            response_body_buf,
            sequence: ResponsesEventSequence::default(),
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        if self.terminal_seen {
            self.write_terminal_log_once();
            return Ok(None);
        }

        match self.next_upstream_item().await? {
            Some(Ok(chunk)) => {
                self.context.mark_upstream_first_byte();
                let mut out_chunk = chunk;
                let semantics = self.openai_stream_semantics();
                let mut observation = StreamObservation::default();
                let mut events = Vec::new();
                self.parser
                    .push_chunk(&out_chunk, |data| events.push(data))
                    .map_err(|error| {
                        let message = format!("Failed to parse upstream SSE: {error}");
                        tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                        self.context.status = 502;
                        self.write_log_once(Some(message));
                        error
                    })?;
                let done_index = semantics
                    .done_sentinel
                    .then(|| events.iter().position(|data| data.trim() == "[DONE]"))
                    .flatten();
                for data in &events {
                    self.sequence.observe_data(&data);
                    observe_stream_data(semantics, &self.context.provider, &data, &mut observation);
                    if data.trim() == "[DONE]" {
                        break;
                    }
                }
                if observation.semantic_event {
                    self.last_semantic_event_at = Instant::now();
                }
                if observation.terminal {
                    self.terminal_seen = true;
                }
                apply_observed_error(&mut self.context, &mut self.terminal_error, &observation);
                if observation.should_synthesize_done() {
                    out_chunk = append_openai_done(out_chunk);
                }
                if let Some(done_index) = done_index {
                    // Preserve every complete SSE event through the terminal marker, then discard
                    // non-spec payloads that share its upstream transport chunk.
                    out_chunk = serialize_sse_data_events(&events[..=done_index]);
                    tracing::debug!(
                        provider = %self.context.provider,
                        upstream_id = %self.context.upstream_id,
                        "dropping OpenAI SSE data after [DONE]"
                    );
                }
                self.collector.push_chunk(&out_chunk).map_err(|error| {
                    let message = format!("Failed to parse upstream SSE: {error}");
                    tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                    self.context.status = 502;
                    self.write_log_once(Some(message));
                    error
                })?;
                self.response_body_buf.push(&out_chunk).await;
                if self.terminal_error.is_some() {
                    self.write_terminal_log_once();
                }
                if observation.starts_client_output {
                    self.context.mark_first_output();
                }

                self.context.mark_first_client_flush();
                Ok(Some((out_chunk, self)))
            }
            Some(Err(err)) => {
                let semantics = self.openai_stream_semantics();
                let mut observation = StreamObservation::default();
                let mut events = Vec::new();
                self.parser
                    .finish(|data| events.push(data))
                    .map_err(|error| {
                        let message = format!("Failed to parse upstream SSE: {error}");
                        tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                        self.context.status = 502;
                        self.write_log_once(Some(message));
                        error
                    })?;
                for data in events {
                    self.sequence.observe_data(&data);
                    observe_stream_data(semantics, &self.context.provider, &data, &mut observation);
                }
                if observation.semantic_event {
                    self.last_semantic_event_at = Instant::now();
                }
                if observation.starts_client_output {
                    self.context.mark_first_output();
                }
                if observation.terminal {
                    self.terminal_seen = true;
                    apply_observed_error(&mut self.context, &mut self.terminal_error, &observation);
                    self.write_terminal_log_once();
                    return Ok(None);
                }

                let message = format!("Failed to read upstream response: {err}");
                if is_chat_stream(&self.context.provider, &self.context.path) {
                    self.context.status = 502;
                    self.terminal_seen = true;
                    self.write_log_once(Some(message.clone()));
                    self.context.mark_first_client_flush();
                    return Ok(Some((chat_error_chunk(&message), self)));
                }
                if semantics.responses_events {
                    let sequence_number = self.sequence.take_next();
                    let out_chunk = openai_response_failed_done_chunk(
                        &message,
                        self.context.model.as_deref(),
                        sequence_number,
                    );
                    self.response_body_buf.push(&out_chunk).await;
                    self.terminal_seen = true;
                    self.terminal_error = Some(message);
                    self.context.status = 502;
                    self.context.mark_first_client_flush();
                    return Ok(Some((out_chunk, self)));
                }
                self.context.status = 502;
                self.write_log_once(Some(message));
                Err(std::io::Error::new(std::io::ErrorKind::Other, err))
            }
            None => {
                let semantics = self.openai_stream_semantics();
                let mut observation = StreamObservation::default();
                let mut events = Vec::new();
                self.parser
                    .finish(|data| events.push(data))
                    .map_err(|error| {
                        let message = format!("Failed to parse upstream SSE: {error}");
                        tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                        self.context.status = 502;
                        self.write_log_once(Some(message));
                        error
                    })?;
                for data in events {
                    self.sequence.observe_data(&data);
                    observe_stream_data(semantics, &self.context.provider, &data, &mut observation);
                }
                if observation.semantic_event {
                    self.last_semantic_event_at = Instant::now();
                }
                if observation.terminal {
                    self.terminal_seen = true;
                }
                apply_observed_error(&mut self.context, &mut self.terminal_error, &observation);
                if observation.starts_client_output {
                    self.context.mark_first_output();
                }

                // 无尾部空行的 terminal event 只会在 finish 时出现，不能按正常 EOF 丢掉错误。
                if self.terminal_seen {
                    self.write_terminal_log_once();
                } else {
                    self.write_log_once(None);
                }
                Ok(None)
            }
        }
    }

    async fn next_upstream_item(&mut self) -> Result<Option<Result<Bytes, E>>, std::io::Error> {
        let Some(timeout) = self.semantic_timeout else {
            return Ok(self.upstream.next().await);
        };
        let elapsed = self.last_semantic_event_at.elapsed();
        if elapsed >= timeout {
            return Ok(Some(Ok(self.semantic_timeout_chunk(timeout))));
        }
        let remaining = timeout.saturating_sub(elapsed);
        match tokio::time::timeout(remaining, self.upstream.next()).await {
            Ok(item) => Ok(item),
            Err(_) => Ok(Some(Ok(self.semantic_timeout_chunk(timeout)))),
        }
    }

    fn semantic_timeout_chunk(&mut self, timeout: Duration) -> Bytes {
        let message = format!(
            "OpenAI Responses stream semantic timeout after {}s.",
            timeout.as_secs_f64()
        );
        tracing::warn!(
            path = %self.context.path,
            provider = %self.context.provider,
            upstream_id = %self.context.upstream_id,
            timeout_secs = timeout.as_secs_f64(),
            "OpenAI Responses stream semantic timeout"
        );
        self.terminal_seen = true;
        self.terminal_error = Some(message.clone());
        self.context.status = 504;
        let sequence_number = self.sequence.take_next();
        openai_response_failed_done_chunk(&message, self.context.model.as_deref(), sequence_number)
    }

    fn openai_stream_semantics(&self) -> OpenAiStreamSemantics {
        openai_stream_semantics(&self.context.provider, &self.context.path)
    }
}

pub(super) fn stream_with_logging_and_model_override_semantic_timeout<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    model_override: String,
    token_tracker: RequestTokenTracker,
    semantic_timeout: Option<Duration>,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let upstream = super::upstream_stream::with_chat_finish_validation(
        upstream,
        is_chat_stream(&context.provider, &context.path),
    );
    let state = ModelOverrideStreamState::new(
        upstream,
        context,
        log,
        Some(model_override),
        token_tracker,
        semantic_timeout,
    );
    try_unfold(state, |state| async move { state.step().await })
}

struct ModelOverrideStreamState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    _token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    model_override: Option<String>,
    upstream_ended: bool,
    logged: bool,
    terminal_seen: bool,
    terminal_error: Option<String>,
    semantic_timeout: Option<Duration>,
    last_semantic_event_at: Instant,
    response_body_buf: super::body_capture::ResponseBodyCapture,
    sequence: ResponsesEventSequence,
    saw_sse_event: bool,
    diagnostic: Option<super::body_capture::StreamDiagnostic>,
    output_item_ids: BTreeMap<i64, String>,
}

impl<S> ModelOverrideStreamState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        let entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        self.response_body_buf.write_log(self.log.clone(), entry);
        self.logged = true;
    }

    fn write_terminal_log_once(&mut self) {
        let response_error = self.terminal_error.take();
        self.write_log_once(response_error);
    }
}

impl<S> Drop for ModelOverrideStreamState<S> {
    fn drop(&mut self) {
        // 和基础流一致：提前 drop 也必须落日志，避免“请求发生但无日志行”。
        if self.terminal_seen {
            self.write_terminal_log_once();
        } else {
            self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
        }
    }
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
        model_override: Option<String>,
        token_tracker: RequestTokenTracker,
        semantic_timeout: Option<Duration>,
    ) -> Self {
        let response_body_buf = super::body_capture::ResponseBodyCapture::new(&context);
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            context,
            _token_tracker: token_tracker,
            out: VecDeque::new(),
            model_override,
            upstream_ended: false,
            logged: false,
            terminal_seen: false,
            terminal_error: None,
            semantic_timeout,
            last_semantic_event_at: Instant::now(),
            response_body_buf,
            sequence: ResponsesEventSequence::default(),
            saw_sse_event: false,
            diagnostic: Some(super::body_capture::StreamDiagnostic::new()),
            output_item_ids: BTreeMap::new(),
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        loop {
            if let Some(next) = self.out.pop_front() {
                self.context.mark_first_client_flush();
                return Ok(Some((next, self)));
            }
            if self.terminal_seen {
                self.write_terminal_log_once();
                return Ok(None);
            }
            if self.upstream_ended {
                if self.openai_stream_semantics().responses_events {
                    // 正常 transport EOF 并不代表 Responses 请求完成，必须见到协议终态。
                    let message = self
                        .diagnostic
                        .as_ref()
                        .and_then(|value| value.message())
                        .unwrap_or_else(|| {
                            "Upstream Responses stream truncated before a terminal event."
                                .to_string()
                        });
                    tracing::warn!(provider = %self.context.provider, upstream_id = %self.context.upstream_id,
                        error = %message, "Responses stream ended without terminal event");
                    self.context.status = 502;
                    self.terminal_seen = true;
                    self.terminal_error = Some(message.clone());
                    let sequence_number = self.sequence.take_next();
                    let chunk = openai_response_failed_done_chunk(
                        &message,
                        self.failure_model(),
                        sequence_number,
                    );
                    self.response_body_buf.push(&chunk).await;
                    self.write_terminal_log_once();
                    self.context.mark_first_client_flush();
                    return Ok(Some((chunk, self)));
                }
                self.write_log_once(None);
                return Ok(None);
            }

            match self.next_upstream_item().await? {
                Some(Ok(chunk)) => {
                    self.context.mark_upstream_first_byte();
                    self.collector.push_chunk(&chunk).map_err(|error| {
                        let message = format!("Failed to parse upstream SSE: {error}");
                        tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                        self.context.status = 502;
                        self.write_log_once(Some(message));
                        error
                    })?;
                    self.response_body_buf.push(&chunk).await;
                    if let Some(diagnostic) = &mut self.diagnostic {
                        diagnostic.push(&chunk);
                    }
                    let mut events = Vec::new();
                    self.parser.push_chunk(&chunk, |data| events.push(data)).map_err(|error| {
                        let message = format!("Failed to parse upstream SSE: {error}");
                        tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                        self.context.status = 502;
                        self.write_log_once(Some(message));
                        error
                    })?;
                    self.saw_sse_event |= !events.is_empty();
                    if self.saw_sse_event {
                        self.diagnostic = None;
                    }
                    let mut observation = StreamObservation::default();
                    let saw_done = events.iter().any(|data| data.trim() == "[DONE]");
                    let semantics = self.openai_stream_semantics();
                    for data in events {
                        let mut event_observation = StreamObservation::default();
                        observe_stream_data(
                            semantics,
                            &self.context.provider,
                            &data,
                            &mut event_observation,
                        );
                        let should_synthesize_done = event_observation.terminal_json && !saw_done;
                        let event_saw_done = event_observation.saw_done;
                        observation.merge(event_observation);
                        self.push_event_output(&data);
                        if should_synthesize_done {
                            self.append_done_to_last_output();
                        }
                        if event_saw_done {
                            tracing::debug!(
                                provider = %self.context.provider,
                                upstream_id = %self.context.upstream_id,
                                "dropping Responses events after [DONE]"
                            );
                            break;
                        }
                    }
                    if observation.semantic_event {
                        self.last_semantic_event_at = Instant::now();
                    }
                    if observation.terminal {
                        self.terminal_seen = true;
                    }
                    apply_observed_error(&mut self.context, &mut self.terminal_error, &observation);
                    if self.terminal_error.is_some() {
                        self.write_terminal_log_once();
                    }
                    if observation.starts_client_output {
                        self.context.mark_first_output();
                    }
                }
                Some(Err(err)) => {
                    let semantics = self.openai_stream_semantics();
                    if is_chat_stream(&self.context.provider, &self.context.path) {
                        let message = format!("Failed to read upstream response: {err}");
                        self.context.status = 502;
                        self.terminal_seen = true;
                        self.write_log_once(Some(message.clone()));
                        self.context.mark_first_client_flush();
                        return Ok(Some((chat_error_chunk(&message), self)));
                    }
                    if semantics.responses_events {
                        let message = format!("Failed to read upstream response: {err}");
                        let sequence_number = self.sequence.take_next();
                        let out_chunk = openai_response_failed_done_chunk(
                            &message,
                            self.failure_model(),
                            sequence_number,
                        );
                        self.response_body_buf.push(&out_chunk).await;
                        self.terminal_seen = true;
                        self.terminal_error = Some(message);
                        self.context.status = 502;
                        self.context.mark_first_client_flush();
                        return Ok(Some((out_chunk, self)));
                    }
                    self.context.status = 502;
                    self.write_log_once(Some(format!("Failed to read upstream response: {err}")));
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
                }
                None => {
                    self.upstream_ended = true;
                    let mut events = Vec::new();
                    self.parser.finish(|data| events.push(data)).map_err(|error| {
                        let message = format!("Failed to parse upstream SSE: {error}");
                        tracing::warn!(upstream_id = %self.context.upstream_id, error = %message,
                            "upstream SSE parser rejected oversized data");
                        self.context.status = 502;
                        self.write_log_once(Some(message));
                        error
                    })?;
                    self.saw_sse_event |= !events.is_empty();
                    if self.saw_sse_event {
                        self.diagnostic = None;
                    }
                    let mut observation = StreamObservation::default();
                    let saw_done = events.iter().any(|data| data.trim() == "[DONE]");
                    let semantics = self.openai_stream_semantics();
                    for data in events {
                        let mut event_observation = StreamObservation::default();
                        observe_stream_data(
                            semantics,
                            &self.context.provider,
                            &data,
                            &mut event_observation,
                        );
                        let should_synthesize_done = event_observation.terminal_json && !saw_done;
                        let event_saw_done = event_observation.saw_done;
                        observation.merge(event_observation);
                        self.push_event_output(&data);
                        if should_synthesize_done {
                            self.append_done_to_last_output();
                        }
                        if event_saw_done {
                            tracing::debug!(
                                provider = %self.context.provider,
                                upstream_id = %self.context.upstream_id,
                                "dropping Responses events after [DONE] at EOF"
                            );
                            break;
                        }
                    }
                    if observation.semantic_event {
                        self.last_semantic_event_at = Instant::now();
                    }
                    if observation.terminal {
                        self.terminal_seen = true;
                    }
                    apply_observed_error(&mut self.context, &mut self.terminal_error, &observation);
                    if observation.starts_client_output {
                        self.context.mark_first_output();
                    }
                }
            }
        }
    }

    async fn next_upstream_item(&mut self) -> Result<Option<Result<Bytes, E>>, std::io::Error> {
        let Some(timeout) = self.semantic_timeout else {
            return Ok(self.upstream.next().await);
        };
        let elapsed = self.last_semantic_event_at.elapsed();
        if elapsed >= timeout {
            return Ok(Some(Ok(self.semantic_timeout_chunk(timeout))));
        }
        let remaining = timeout.saturating_sub(elapsed);
        match tokio::time::timeout(remaining, self.upstream.next()).await {
            Ok(item) => Ok(item),
            Err(_) => Ok(Some(Ok(self.semantic_timeout_chunk(timeout)))),
        }
    }

    fn semantic_timeout_chunk(&mut self, timeout: Duration) -> Bytes {
        let message = format!(
            "OpenAI Responses stream semantic timeout after {}s.",
            timeout.as_secs_f64()
        );
        tracing::warn!(
            path = %self.context.path,
            provider = %self.context.provider,
            upstream_id = %self.context.upstream_id,
            timeout_secs = timeout.as_secs_f64(),
            "OpenAI Responses stream semantic timeout"
        );
        self.terminal_seen = true;
        self.terminal_error = Some(message.clone());
        self.context.status = 504;
        let sequence_number = self.sequence.take_next();
        openai_response_failed_done_chunk(&message, self.failure_model(), sequence_number)
    }

    fn openai_stream_semantics(&self) -> OpenAiStreamSemantics {
        openai_stream_semantics(&self.context.provider, &self.context.path)
    }

    fn push_event_output(&mut self, data: &str) {
        let allow_codex_metadata = self.context.provider == PROVIDER_CODEX
            && is_openai_responses_stream_path(&self.context.path);
        if !should_forward_response_event(data, allow_codex_metadata) {
            tracing::debug!(
                provider = %self.context.provider,
                upstream_id = %self.context.upstream_id,
                allow_codex_metadata,
                "filtered private Responses SSE event"
            );
            return;
        }
        let output = self
            .model_override
            .as_deref()
            .map(|model| rewrite_sse_data(data, model))
            .unwrap_or_else(|| data.to_string());
        let mut output = self.normalize_failure_event(output);
        if let Ok(mut value) = serde_json::from_str::<Value>(&output) {
            super::remember_responses_output_item_id(&value, &mut self.output_item_ids);
            if value.get("type").and_then(Value::as_str) == Some("response.completed") {
                if let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) {
                    let hydrated =
                        super::hydrate_responses_output_item_ids(response, &self.output_item_ids);
                    if hydrated > 0 {
                        tracing::debug!(
                            provider = %self.context.provider,
                            upstream_id = %self.context.upstream_id,
                            hydrated,
                            "hydrated missing Responses output item IDs"
                        );
                        output = value.to_string();
                    }
                }
            }
        }
        if sse_data_type(&output).as_deref() == Some("response.failed") {
            self.out.push_back(Bytes::from(format!(
                "event: response.failed\ndata: {output}\n\n"
            )));
            return;
        }
        self.out
            .push_back(Bytes::from(format!("data: {output}\n\n")));
    }

    fn normalize_failure_event(&mut self, output: String) -> String {
        let Ok(mut value) = serde_json::from_str::<Value>(&output) else {
            return output;
        };
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let failure_model = self.failure_model().map(str::to_string);
        let normalization = responses_failure::normalize_stream_event(
            &mut value,
            &mut self.sequence,
            failure_model.as_deref(),
        );
        if normalization.changed() {
            tracing::warn!(
                provider = %self.context.provider,
                upstream_id = %self.context.upstream_id,
                event_type,
                sequence_number = ?normalization.sequence_number,
                added_created_at = normalization.added_created_at,
                added_model = normalization.added_model,
                repaired_response_shape = normalization.repaired_response_shape,
                "normalized incomplete Responses terminal failure event"
            );
            return value.to_string();
        }
        output
    }

    fn failure_model(&self) -> Option<&str> {
        self.model_override
            .as_deref()
            .or(self.context.model.as_deref())
    }

    fn append_done_to_last_output(&mut self) {
        let Some(last) = self.out.pop_back() else {
            self.out.push_back(Bytes::from("data: [DONE]\n\n"));
            return;
        };
        let mut combined = Vec::with_capacity(last.len() + b"data: [DONE]\n\n".len());
        combined.extend_from_slice(&last);
        combined.extend_from_slice(b"data: [DONE]\n\n");
        self.out.push_back(Bytes::from(combined));
    }
}

#[derive(Clone, Copy, Default)]
struct OpenAiStreamSemantics {
    done_sentinel: bool,
    responses_events: bool,
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

fn observe_stream_data(
    semantics: OpenAiStreamSemantics,
    provider: &str,
    data: &str,
    observation: &mut StreamObservation,
) {
    if data.trim() == "[DONE]" {
        if semantics.done_sentinel {
            observation.semantic_event = true;
            observation.terminal = true;
            observation.saw_done = true;
        }
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if semantics.responses_events {
        observation.semantic_event = true;
        observation.terminal_error = responses_stream_error(&value);
        if openai_stream_value_is_terminal(&value) {
            observation.terminal = true;
            observation.terminal_json = true;
        }
    } else if semantics.done_sentinel {
        if let Some(error) = super::upstream_stream::chat_stream_error(&value) {
            observation.terminal = true;
            observation.terminal_error = Some(error);
        }
    }
    if openai_responses_data_starts_client_output(provider, &value) {
        observation.starts_client_output = true;
    }
    if let Some(text) = extract_stream_text_from_value(provider, &value) {
        if !text.is_empty() {
            observation.starts_client_output = true;
        }
    }
}

impl StreamObservation {
    fn merge(&mut self, next: StreamObservation) {
        let StreamObservation {
            starts_client_output,
            semantic_event,
            terminal,
            terminal_json,
            saw_done,
            terminal_error,
        } = next;
        self.starts_client_output |= starts_client_output;
        self.semantic_event |= semantic_event;
        self.terminal |= terminal;
        self.terminal_json |= terminal_json;
        self.saw_done |= saw_done;
        if self.terminal_error.is_none() {
            self.terminal_error = terminal_error;
        }
    }

    fn should_synthesize_done(&self) -> bool {
        self.terminal_json && !self.saw_done
    }
}

fn apply_observed_error(
    context: &mut LogContext,
    terminal_error: &mut Option<String>,
    observation: &StreamObservation,
) {
    let Some(error) = observation.terminal_error.as_ref() else {
        return;
    };
    context.status = error.status.as_u16();
    *terminal_error = Some(error.message.clone());
}

fn append_openai_done(chunk: Bytes) -> Bytes {
    let mut bytes = Vec::with_capacity(chunk.len() + b"data: [DONE]\n\n".len());
    bytes.extend_from_slice(chunk.as_ref());
    bytes.extend_from_slice(b"data: [DONE]\n\n");
    Bytes::from(bytes)
}

fn serialize_sse_data_events(events: &[String]) -> Bytes {
    let mut output = String::new();
    for event in events {
        for line in event.lines() {
            output.push_str("data: ");
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }
    Bytes::from(output)
}

fn should_forward_response_event(data: &str, allow_codex_metadata: bool) -> bool {
    if data.trim() == "[DONE]" {
        return true;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return true;
    };
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return true;
    };
    if event_type.starts_with("responsesapi.") {
        return false;
    }
    if event_type.starts_with("codex.") {
        return allow_codex_metadata && event_type == "codex.response.metadata";
    }
    true
}

fn openai_response_failed_done_chunk(
    message: &str,
    model: Option<&str>,
    sequence_number: u64,
) -> Bytes {
    let payload = openai_response_failed_payload(message, model, sequence_number);
    Bytes::from(format!(
        "event: response.failed\ndata: {payload}\n\ndata: [DONE]\n\n"
    ))
}

fn openai_response_failed_payload(
    message: &str,
    model: Option<&str>,
    sequence_number: u64,
) -> Value {
    responses_failure::failed_event(message, model, sequence_number)
}

fn sse_data_type(data: &str) -> Option<String> {
    serde_json::from_str::<Value>(data).ok().and_then(|value| {
        value
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn openai_stream_semantics(provider: &str, path: &str) -> OpenAiStreamSemantics {
    let done_sentinel = matches!(
        provider,
        PROVIDER_OPENAI | PROVIDER_OPENAI_RESPONSES | PROVIDER_CODEX | PROVIDER_XAI
    );
    OpenAiStreamSemantics {
        done_sentinel,
        responses_events: done_sentinel && is_openai_responses_stream_path(path),
    }
}

fn is_chat_stream(provider: &str, path: &str) -> bool {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    openai_stream_semantics(provider, path).done_sentinel
        && matches!(path, "/v1/chat/completions" | "/chat/completions")
}

fn chat_error_chunk(message: &str) -> Bytes {
    tracing::warn!(
        error = message,
        "upstream Chat stream failed after response commitment"
    );
    super::responses_event_sse(serde_json::json!({"error": {
        "message": message,
        "type": "upstream_error",
        "code": "upstream_stream_error"
    }}))
}

fn is_openai_responses_stream_path(path: &str) -> bool {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    path == "/v1/responses" || path.starts_with("/v1/responses/")
}

fn openai_stream_value_is_terminal(value: &Value) -> bool {
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return false;
    };
    matches!(
        event_type.trim(),
        "response.completed"
            | "response.done"
            | "response.failed"
            | "response.error"
            | "response.incomplete"
            | "response.cancelled"
            | "response.canceled"
            | "error"
    )
}

fn extract_stream_text_from_value<'a>(provider: &str, value: &'a Value) -> Option<&'a str> {
    match provider {
        PROVIDER_OPENAI | PROVIDER_OPENAI_RESPONSES | PROVIDER_CODEX | PROVIDER_XAI => {
            extract_openai_stream_text(value)
                .or_else(|| extract_openai_responses_delta_text(value))
                .or_else(|| {
                    if value.get("type").is_none() {
                        extract_fallback_stream_text(value)
                    } else {
                        None
                    }
                })
        }
        PROVIDER_ANTHROPIC => extract_anthropic_stream_text(value),
        PROVIDER_GEMINI => extract_gemini_stream_text(value),
        _ => extract_fallback_stream_text(value),
    }
}

fn openai_responses_data_starts_client_output(provider: &str, value: &Value) -> bool {
    if !matches!(
        provider,
        PROVIDER_OPENAI_RESPONSES | PROVIDER_CODEX | PROVIDER_XAI
    ) {
        return false;
    }
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return false;
    };
    !matches!(
        event_type.trim(),
        "" | "response.created"
            | "response.in_progress"
            | "response.failed"
            | "response.error"
            | "error"
    )
}

fn extract_openai_stream_text(value: &Value) -> Option<&str> {
    let delta = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str);
    if let Some(delta) = delta {
        return Some(delta);
    }
    let event_type = value.get("type").and_then(Value::as_str)?;
    if event_type.ends_with("output_text.delta") {
        return value.get("delta").and_then(Value::as_str);
    }
    None
}

fn extract_openai_responses_delta_text(value: &Value) -> Option<&str> {
    // 首输出识别只借用增量文本，完整事件的业务语义由上方独立判断。
    let event_type = value.get("type").and_then(Value::as_str)?;
    if !is_openai_responses_realtime_output_delta(event_type) {
        return None;
    }
    value.get("delta").and_then(Value::as_str)
}

fn is_openai_responses_realtime_output_delta(event_type: &str) -> bool {
    event_type.ends_with(".delta")
        && matches!(
            event_type
                .strip_prefix("response.")
                .unwrap_or(event_type)
                .strip_suffix(".delta")
                .unwrap_or(event_type),
            "output_text"
                | "reasoning_text"
                | "reasoning_summary_text"
                | "refusal"
                | "function_call_arguments"
                | "mcp_call_arguments"
                | "custom_tool_call_input"
                | "code_interpreter_call_code"
        )
}

fn extract_anthropic_stream_text(value: &Value) -> Option<&str> {
    if let Some(delta) = value.get("delta") {
        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            return Some(text);
        }
        if let Some(text) = delta.as_str() {
            return Some(text);
        }
    }
    value
        .get("content_block")
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
}

fn extract_gemini_stream_text(value: &Value) -> Option<&str> {
    let candidates = value.get("candidates").and_then(Value::as_array)?;
    for candidate in candidates {
        if let Some(content) = candidate.get("content") {
            if let Some(parts) = content.get("parts").and_then(Value::as_array) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

fn extract_fallback_stream_text(value: &Value) -> Option<&str> {
    value
        .get("delta")
        .and_then(Value::as_str)
        .or_else(|| value.get("text").and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xai_uses_responses_stream_semantics_and_delta_extraction() {
        let semantics = openai_stream_semantics(PROVIDER_XAI, "/v1/responses");
        assert!(semantics.done_sentinel);
        assert!(semantics.responses_events);

        let delta = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "hello"
        });
        assert!(openai_responses_data_starts_client_output(
            PROVIDER_XAI,
            &delta
        ));
        assert_eq!(
            extract_stream_text_from_value(PROVIDER_XAI, &delta),
            Some("hello")
        );
    }

    #[test]
    fn filters_private_responses_events_by_client_contract() {
        assert!(!should_forward_response_event(
            r#"{"type":"responsesapi.telemetry"}"#,
            false
        ));
        assert!(!should_forward_response_event(
            r#"{"type":"codex.rate_limits"}"#,
            false
        ));
        assert!(!should_forward_response_event(
            r#"{"type":"codex.response.metadata"}"#,
            false
        ));
        assert!(should_forward_response_event(
            r#"{"type":"codex.response.metadata"}"#,
            true
        ));
        assert!(should_forward_response_event(
            r#"{"type":"response.completed"}"#,
            false
        ));
        assert!(should_forward_response_event("[DONE]", false));
    }
}
