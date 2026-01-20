use axum::body::Bytes;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use super::super::kiro::{EventStreamDecoder, KiroToolUse, KiroUsage};
use super::super::kiro::tool_parser::process_tool_use_event;
use super::super::log::{build_log_entry, LogContext, LogWriter, UsageSnapshot};
use super::super::token_rate::RequestTokenTracker;
use super::kiro_to_responses_helpers::{
    apply_usage_fallback,
    detect_event_type,
    extract_error,
    update_stop_reason,
    update_usage,
    usage_from_kiro,
    usage_json_from_kiro,
};

pub(super) fn convert_kiro_response(
    bytes: &Bytes,
    model: Option<&str>,
    estimated_input_tokens: Option<u64>,
) -> Result<Bytes, String> {
    let parsed = crate::proxy::kiro::parse_event_stream(bytes)
        .map_err(|message| format!("Failed to parse Kiro response: {message}"))?;
    let mut usage = parsed.usage.clone();
    apply_usage_fallback(
        &mut usage,
        model,
        estimated_input_tokens,
        &parsed.content,
        &parsed.reasoning,
    );
    let response = build_claude_response(
        parsed.content,
        parsed.reasoning,
        parsed.tool_uses,
        usage,
        parsed.stop_reason.as_deref(),
        model.unwrap_or("unknown"),
    );
    serde_json::to_vec(&response)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

pub(super) fn stream_kiro_to_anthropic<E>(
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
    tool_state: Option<super::super::kiro::tool_parser::ToolUseState>,
    usage: KiroUsage,
    stop_reason: Option<String>,
    thinking_state: ThinkingStreamState,
    content: String,
    reasoning: String,
    saw_tool_use: bool,
    logged: bool,
    upstream_ended: bool,
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
        let now_ms = super::now_ms();
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
            content: String::new(),
            reasoning: String::new(),
            saw_tool_use: false,
            logged: false,
            upstream_ended: false,
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

    async fn handle_message(&mut self, payload: &[u8], event_type: &str) {
        if self.sent_message_stop || payload.is_empty() {
            return;
        }
        let Ok(event) = serde_json::from_slice::<Value>(payload) else {
            return;
        };
        let Some(event_obj) = event.as_object() else {
            return;
        };
        if let Some(error) = extract_error(event_obj) {
            if error != "invalidStateEvent" {
                self.finish_message_if_needed();
            }
            return;
        }

        update_stop_reason(event_obj, &mut self.stop_reason);
        update_usage(event_obj, &mut self.usage);

        let event_type = if !event_type.is_empty() {
            event_type
        } else {
            detect_event_type(event_obj)
        };

        match event_type {
            "assistantResponseEvent" => self.handle_assistant_response(event_obj).await,
            "toolUseEvent" => self.handle_tool_use_event(event_obj).await,
            "reasoningContentEvent" => self.handle_reasoning_content(event_obj).await,
            "messageStopEvent" | "message_stop" => {
                update_stop_reason(event_obj, &mut self.stop_reason);
            }
            _ => {}
        }
    }

    async fn handle_assistant_response(&mut self, event: &Map<String, Value>) {
        if let Some(Value::Object(assistant)) = event.get("assistantResponseEvent") {
            if let Some(text) = assistant.get("content").and_then(Value::as_str) {
                self.handle_text_delta(text).await;
            }
            if let Some(items) = assistant.get("toolUses").and_then(Value::as_array) {
                self.handle_tool_uses(items);
            }
            update_stop_reason(assistant, &mut self.stop_reason);
        }
        if let Some(text) = event.get("content").and_then(Value::as_str) {
            self.handle_text_delta(text).await;
        }
        if let Some(items) = event.get("toolUses").and_then(Value::as_array) {
            self.handle_tool_uses(items);
        }
    }

    async fn handle_reasoning_content(&mut self, event: &Map<String, Value>) {
        if let Some(Value::Object(reasoning)) = event.get("reasoningContentEvent") {
            if let Some(text) = reasoning.get("thinkingText").and_then(Value::as_str) {
                self.emit_thinking_delta(text).await;
            }
            if let Some(text) = reasoning.get("text").and_then(Value::as_str) {
                self.emit_thinking_delta(text).await;
            }
        }
    }

    async fn handle_text_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let mut combined = String::new();
        if !self.thinking_state.pending.is_empty() {
            combined.push_str(&self.thinking_state.pending);
            self.thinking_state.pending.clear();
        }
        combined.push_str(delta);
        self.process_thinking_delta(&combined).await;
    }

    async fn emit_text_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.ensure_message_start();
        let index = self.ensure_text_block();
        self.content.push_str(delta);
        self.token_tracker.add_output_text(delta).await;
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "text_delta", "text": delta }
            }),
        ));
    }

    async fn emit_thinking_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.ensure_message_start();
        let index = self.ensure_thinking_block();
        self.reasoning.push_str(delta);
        self.token_tracker.add_output_text(delta).await;
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "thinking_delta", "thinking": delta }
            }),
        ));
    }

    async fn process_thinking_delta(&mut self, input: &str) {
        const START: &str = "<thinking>";
        const END: &str = "</thinking>";

        let mut cursor = 0;
        while cursor < input.len() {
            if self.thinking_state.in_thinking {
                if let Some(pos) = input[cursor..].find(END) {
                    let end = cursor + pos;
                    if end > cursor {
                        self.emit_thinking_delta(&input[cursor..end]).await;
                    }
                    cursor = end + END.len();
                    self.thinking_state.in_thinking = false;
                    continue;
                }
                let (emit, pending) = split_partial_tag(&input[cursor..], END);
                if !emit.is_empty() {
                    self.emit_thinking_delta(&emit).await;
                }
                self.thinking_state.pending = pending;
                break;
            }

            if let Some(pos) = input[cursor..].find(START) {
                let end = cursor + pos;
                if end > cursor {
                    self.emit_text_delta(&input[cursor..end]).await;
                }
                cursor = end + START.len();
                self.thinking_state.in_thinking = true;
                continue;
            }
            let (emit, pending) = split_partial_tag(&input[cursor..], START);
            if !emit.is_empty() {
                self.emit_text_delta(&emit).await;
            }
            self.thinking_state.pending = pending;
            break;
        }
    }

    async fn flush_thinking_pending(&mut self) {
        if self.thinking_state.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.thinking_state.pending);
        if self.thinking_state.in_thinking {
            self.emit_thinking_delta(&pending).await;
        } else {
            self.emit_text_delta(&pending).await;
        }
    }

    async fn handle_tool_use_event(&mut self, event: &Map<String, Value>) {
        let (completed, next_state) =
            process_tool_use_event(event, self.tool_state.take(), &mut self.processed_tool_keys);
        self.tool_state = next_state;

        let source = event
            .get("toolUseEvent")
            .and_then(Value::as_object)
            .unwrap_or(event);
        let tool_use_id = tool_use_id(source);
        let name = source.get("name").and_then(Value::as_str).unwrap_or("");
        let stop = source.get("stop").and_then(Value::as_bool).unwrap_or(false);
        let input_value = source.get("input");

        if let Some(tool_use_id) = tool_use_id {
            if !name.is_empty() {
                self.ensure_tool_use_block(tool_use_id, name);
            }
            if let Some(input_value) = input_value {
                self.emit_tool_use_input(tool_use_id, input_value);
            }
            if stop {
                self.stop_tool_use_block(tool_use_id);
            }
        }

        for tool_use in completed {
            self.ensure_tool_use_block(&tool_use.tool_use_id, &tool_use.name);
            self.emit_tool_use_input(
                &tool_use.tool_use_id,
                &Value::Object(tool_use.input.clone()),
            );
            self.stop_tool_use_block(&tool_use.tool_use_id);
        }
    }

    fn handle_tool_uses(&mut self, items: &[Value]) {
        for item in items {
            let Some(tool) = item.as_object() else {
                continue;
            };
            let tool_use_id = tool_use_id(tool);
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            let input = tool.get("input").cloned().unwrap_or_else(|| json!({}));
            let Some(tool_use_id) = tool_use_id else {
                continue;
            };
            let dedupe_key = format!("id:{tool_use_id}");
            if self.processed_tool_keys.contains(&dedupe_key) {
                continue;
            }
            self.processed_tool_keys.insert(dedupe_key);
            if !name.is_empty() {
                self.ensure_tool_use_block(tool_use_id, name);
            }
            self.emit_tool_use_input(tool_use_id, &input);
            self.stop_tool_use_block(tool_use_id);
        }
    }

    fn ensure_message_start(&mut self) {
        if self.sent_message_start {
            return;
        }
        self.sent_message_start = true;
        let usage = usage_json_from_kiro(&self.usage).unwrap_or_else(|| json!({
            "input_tokens": 0,
            "output_tokens": 0
        }));
        let message = json!({
            "id": self.message_id.as_str(),
            "type": "message",
            "role": "assistant",
            "model": self.model.as_str(),
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": usage
        });
        self.out.push_back(super::anthropic_event_sse(
            "message_start",
            json!({ "type": "message_start", "message": message }),
        ));
    }

    fn ensure_text_block(&mut self) -> usize {
        if let Some(ActiveBlock::Text { index }) = self.active_block {
            return index;
        }
        self.stop_active_block();
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.active_block = Some(ActiveBlock::Text { index });
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "text", "text": "" }
            }),
        ));
        index
    }

    fn ensure_thinking_block(&mut self) -> usize {
        if let Some(ActiveBlock::Thinking { index }) = self.active_block {
            return index;
        }
        self.stop_active_block();
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.active_block = Some(ActiveBlock::Thinking { index });
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
        ));
        index
    }

    fn ensure_tool_use_block(&mut self, tool_use_id: &str, name: &str) {
        if !self.tool_uses.contains_key(tool_use_id) {
            let index = self.next_block_index;
            self.next_block_index += 1;
            self.tool_uses.insert(tool_use_id.to_string(), ToolUseState {
                index,
                name: name.to_string(),
                sent_start: false,
                sent_stop: false,
                sent_input: false,
            });
        }
        if let Some(state) = self.tool_uses.get_mut(tool_use_id) {
            if state.name.is_empty() {
                state.name = name.to_string();
            }
        }
        if !self.tool_uses.get(tool_use_id).is_some_and(|state| state.sent_start) {
            self.start_tool_use_block(tool_use_id);
        }
    }

    fn start_tool_use_block(&mut self, tool_use_id: &str) {
        let Some((index, name, sent_start)) = self.tool_uses.get(tool_use_id).map(|state| {
            (state.index, state.name.clone(), state.sent_start)
        }) else {
            return;
        };
        if sent_start {
            return;
        }
        self.stop_active_block();
        if let Some(state) = self.tool_uses.get_mut(tool_use_id) {
            state.sent_start = true;
        }
        self.saw_tool_use = true;
        self.active_block = Some(ActiveBlock::ToolUse {
            id: tool_use_id.to_string(),
        });
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "tool_use",
                    "id": tool_use_id,
                    "name": name,
                    "input": {}
                }
            }),
        ));
    }

    fn emit_tool_use_input(&mut self, tool_use_id: &str, value: &Value) {
        let Some((index, sent_input)) = self
            .tool_uses
            .get(tool_use_id)
            .map(|state| (state.index, state.sent_input))
        else {
            return;
        };
        let input = match value {
            Value::String(text) => text.clone(),
            Value::Object(obj) => serde_json::to_string(obj).unwrap_or_default(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        if input.trim().is_empty() {
            return;
        }
        if sent_input && !value.is_string() {
            return;
        }
        self.set_active_tool_use(tool_use_id);
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "input_json_delta", "partial_json": input }
            }),
        ));
        if let Some(state) = self.tool_uses.get_mut(tool_use_id) {
            if !value.is_string() {
                state.sent_input = true;
            }
        }
    }

    fn set_active_tool_use(&mut self, tool_use_id: &str) {
        if !self.tool_uses.contains_key(tool_use_id) {
            return;
        }
        match &self.active_block {
            Some(ActiveBlock::ToolUse { id }) if id == tool_use_id => {}
            _ => {
                self.stop_active_block();
                self.active_block = Some(ActiveBlock::ToolUse {
                    id: tool_use_id.to_string(),
                });
            }
        }
    }

    fn stop_tool_use_block(&mut self, tool_use_id: &str) {
        let Some(state) = self.tool_uses.get_mut(tool_use_id) else {
            return;
        };
        if state.sent_stop {
            return;
        }
        state.sent_stop = true;
        let index = state.index;
        self.out.push_back(super::anthropic_event_sse(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": index }),
        ));
        if matches!(&self.active_block, Some(ActiveBlock::ToolUse { id }) if id == tool_use_id) {
            self.active_block = None;
        }
    }

    fn stop_active_block(&mut self) {
        let Some(active) = self.active_block.take() else {
            return;
        };
        match active {
            ActiveBlock::Text { index } | ActiveBlock::Thinking { index } => {
                self.out.push_back(super::anthropic_event_sse(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": index }),
                ));
            }
            ActiveBlock::ToolUse { id } => {
                self.stop_tool_use_block(&id);
            }
        }
    }

    fn finish_message_if_needed(&mut self) {
        if self.sent_message_stop {
            return;
        }
        self.ensure_message_start();
        self.stop_active_block();

        let stop_reason = self.stop_reason.clone().unwrap_or_else(|| {
            if self.saw_tool_use {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            }
        });
        apply_usage_fallback(
            &mut self.usage,
            Some(&self.model),
            self.estimated_input_tokens,
            &self.content,
            &self.reasoning,
        );
        let input_tokens = self.usage.input_tokens.unwrap_or(0);
        let output_tokens = self.usage.output_tokens.unwrap_or(0);
        let mut usage_obj = Map::new();
        usage_obj.insert("input_tokens".to_string(), json!(input_tokens));
        usage_obj.insert("output_tokens".to_string(), json!(output_tokens));
        if let Some(cached) = usage_json_from_kiro(&self.usage)
            .and_then(|value| value.get("cache_read_input_tokens").cloned())
        {
            usage_obj.insert("cache_read_input_tokens".to_string(), cached);
        }

        self.out.push_back(super::anthropic_event_sse(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason, "stop_sequence": null },
                "usage": Value::Object(usage_obj)
            }),
        ));
        self.out.push_back(super::anthropic_event_sse(
            "message_stop",
            json!({ "type": "message_stop" }),
        ));
        self.sent_message_stop = true;
    }

    fn log_usage_once(&mut self) {
        if self.logged {
            return;
        }
        self.logged = true;
        apply_usage_fallback(
            &mut self.usage,
            Some(&self.model),
            self.estimated_input_tokens,
            &self.content,
            &self.reasoning,
        );
        let usage_snapshot = UsageSnapshot {
            usage: usage_from_kiro(&self.usage),
            cached_tokens: None,
            usage_json: usage_json_from_kiro(&self.usage),
        };
        let entry = build_log_entry(&self.context, usage_snapshot, None);
        self.log.clone().write_detached(entry);
    }
}

fn tool_use_id(source: &Map<String, Value>) -> Option<&str> {
    source
        .get("toolUseId")
        .or_else(|| source.get("tool_use_id"))
        .and_then(Value::as_str)
}

fn build_claude_response(
    content: String,
    reasoning: String,
    tool_uses: Vec<KiroToolUse>,
    usage: KiroUsage,
    stop_reason: Option<&str>,
    model: &str,
) -> Value {
    let mut blocks = Vec::new();
    if !reasoning.trim().is_empty() {
        blocks.push(json!({
            "type": "thinking",
            "thinking": reasoning,
            "signature": thinking_signature(&reasoning)
        }));
    }
    if !content.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": content }));
    }
    for tool_use in tool_uses.iter() {
        blocks.push(json!({
            "type": "tool_use",
            "id": tool_use.tool_use_id,
            "name": tool_use.name,
            "input": tool_use.input
        }));
    }
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "" }));
    }
    let stop_reason = stop_reason.unwrap_or_else(|| {
        if tool_uses.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        }
    });
    let usage_value = usage_json_from_kiro(&usage).unwrap_or_else(|| json!({
        "input_tokens": 0,
        "output_tokens": 0
    }));
    json!({
        "id": format!("msg_{}", super::super::kiro::utils::random_uuid()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": blocks,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage_value
    })
}

fn thinking_signature(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    STANDARD.encode(hasher.finalize())
}

fn split_partial_tag(segment: &str, tag: &str) -> (String, String) {
    if tag.len() <= 1 || segment.is_empty() {
        return (segment.to_string(), String::new());
    }
    let max_len = std::cmp::min(segment.len(), tag.len() - 1);
    for len in (1..=max_len).rev() {
        if segment.ends_with(&tag[..len]) {
            let emit_end = segment.len() - len;
            return (segment[..emit_end].to_string(), segment[emit_end..].to_string());
        }
    }
    (segment.to_string(), String::new())
}
