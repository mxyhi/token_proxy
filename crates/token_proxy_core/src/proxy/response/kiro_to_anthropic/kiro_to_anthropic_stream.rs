use axum::body::Bytes;
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::proxy::kiro::{EventStreamDecoder, KiroUsage};
use crate::proxy::log::{LogContext, LogWriter};
use crate::proxy::response::STREAM_DROPPED_ERROR;
use crate::proxy::token_rate::RequestTokenTracker;

pub(super) const USAGE_UPDATE_CHAR_THRESHOLD: usize = 5000;
pub(super) const USAGE_UPDATE_TIME_INTERVAL: Duration = Duration::from_secs(15);
pub(super) const USAGE_UPDATE_TOKEN_DELTA: u64 = 10;

pub(crate) fn stream_kiro_to_anthropic<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>>
        + Unpin
        + Send
        + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = KiroToAnthropicState::new(
        upstream,
        context,
        log,
        token_tracker,
        estimated_input_tokens,
    );
    futures_util::stream::try_unfold(state, |state| async move { state.step().await })
}

enum ActiveBlock {
    Text { index: usize },
    Thinking { index: usize },
    ToolUse { id: String },
}

struct ToolUseState {
    index: usize,
    name: String,
    sent_start: bool,
    sent_stop: bool,
    sent_input: bool,
}

struct ThinkingStreamState {
    in_thinking: bool,
    pending: String,
}

struct KiroToAnthropicState<S> {
    upstream: S,
    decoder: EventStreamDecoder,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
    out: VecDeque<Bytes>,
    message_id: String,
    model: String,
    sent_message_start: bool,
    sent_message_stop: bool,
    active_block: Option<ActiveBlock>,
    next_block_index: usize,
    tool_uses: HashMap<String, ToolUseState>,
    processed_tool_keys: HashSet<String>,
    tool_state: Option<crate::proxy::kiro::tool_parser::ToolUseState>,
    usage: KiroUsage,
    stop_reason: Option<String>,
    thinking_state: ThinkingStreamState,
    raw_content: String,
    content: String,
    reasoning: String,
    saw_tool_use: bool,
    logged: bool,
    upstream_ended: bool,
    last_ping_len: usize,
    last_ping_time: Instant,
    last_reported_output_tokens: u64,
}

impl<S> Drop for KiroToAnthropicState<S> {
    fn drop(&mut self) {
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S, E> KiroToAnthropicState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(
        upstream: S,
        context: LogContext,
        log: Arc<LogWriter>,
        token_tracker: RequestTokenTracker,
        estimated_input_tokens: Option<u64>,
    ) -> Self {
        let now_ms = super::super::now_ms();
        let model = context
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        Self {
            upstream,
            decoder: EventStreamDecoder::new(),
            log,
            context,
            token_tracker,
            estimated_input_tokens,
            out: VecDeque::new(),
            message_id: format!("msg_proxy_{now_ms}"),
            model,
            sent_message_start: false,
            sent_message_stop: false,
            active_block: None,
            next_block_index: 0,
            tool_uses: HashMap::new(),
            processed_tool_keys: HashSet::new(),
            tool_state: None,
            usage: KiroUsage::default(),
            stop_reason: None,
            thinking_state: ThinkingStreamState {
                in_thinking: false,
                pending: String::new(),
            },
            raw_content: String::new(),
            content: String::new(),
            reasoning: String::new(),
            saw_tool_use: false,
            logged: false,
            upstream_ended: false,
            last_ping_len: 0,
            last_ping_time: Instant::now(),
            last_reported_output_tokens: 0,
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        loop {
            if let Some(next) = self.out.pop_front() {
                return Ok(Some((next, self)));
            }

            if self.upstream_ended {
                return Ok(None);
            }

            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    self.handle_chunk(&chunk).await?;
                }
                Some(Err(err)) => {
                    self.log_usage_once();
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
                }
                None => {
                    self.upstream_ended = true;
                    self.finish_stream().await?;
                    if self.out.is_empty() {
                        return Ok(None);
                    }
                }
            }
        }
    }

    async fn handle_chunk(&mut self, chunk: &Bytes) -> Result<(), std::io::Error> {
        let messages = self
            .decoder
            .push(chunk)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.message))?;
        for message in messages {
            self.handle_message(&message.payload, &message.event_type)
                .await;
        }
        Ok(())
    }

    async fn finish_stream(&mut self) -> Result<(), std::io::Error> {
        let messages = self
            .decoder
            .finish()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.message))?;
        for message in messages {
            self.handle_message(&message.payload, &message.event_type)
                .await;
        }
        self.flush_thinking_pending().await;
        self.finish_message_if_needed();
        self.log_usage_once();
        Ok(())
    }
}

mod kiro_to_anthropic_stream_blocks;
mod kiro_to_anthropic_stream_handlers;
