use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use super::super::log::{build_log_entry, LogContext, LogWriter};
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;

pub(super) fn stream_responses_to_anthropic<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>>
        + Unpin
        + Send
        + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = ResponsesToAnthropicState::new(upstream, context, log, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

enum ActiveBlock {
    Text { index: usize },
    ToolUse { item_id: String },
}

struct ToolUseState {
    index: usize,
    tool_use_id: String,
    name: String,
    sent_start: bool,
    sent_stop: bool,
}

struct ResponsesToAnthropicState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    message_id: String,
    model: String,
    sent_message_start: bool,
    sent_message_stop: bool,
    logged: bool,
    upstream_ended: bool,
    active_block: Option<ActiveBlock>,
    next_block_index: usize,
    tool_uses: HashMap<String, ToolUseState>,
    saw_tool_use: bool,
}

impl<S, E> ResponsesToAnthropicState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(
        upstream: S,
        context: LogContext,
        log: Arc<LogWriter>,
        token_tracker: RequestTokenTracker,
    ) -> Self {
        let now_ms = super::now_ms();
        let model = context
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            context,
            token_tracker,
            out: VecDeque::new(),
            message_id: format!("msg_proxy_{now_ms}"),
            model,
            sent_message_start: false,
            sent_message_stop: false,
            logged: false,
            upstream_ended: false,
            active_block: None,
            next_block_index: 0,
            tool_uses: HashMap::new(),
            saw_tool_use: false,
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
                    self.collector.push_chunk(&chunk);
                    let mut events = Vec::new();
                    self.parser.push_chunk(&chunk, |data| events.push(data));
                    let mut texts = Vec::new();
                    for data in events {
                        self.handle_event(&data, &mut texts);
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
                        self.handle_event(&data, &mut texts);
                    }
                    for text in texts {
                        self.token_tracker.add_output_text(&text).await;
                    }
                    self.finish_message_if_needed();
                    if self.out.is_empty() {
                        self.log_usage_once();
                        return Ok(None);
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, data: &str, token_texts: &mut Vec<String>) {
        if self.sent_message_stop {
            return;
        }
        if data == "[DONE]" {
            self.finish_message_if_needed();
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let Some(event_type) = value.get("type").and_then(Value::as_str) else {
            return;
        };

        if event_type.ends_with("output_text.delta") {
            self.handle_output_text_delta(&value, token_texts);
            return;
        }
        if event_type.ends_with("output_item.added") {
            self.handle_output_item_added(&value);
            return;
        }
        if event_type.ends_with("function_call_arguments.delta") {
            self.handle_function_call_arguments_delta(&value);
            return;
        }
        if event_type.ends_with("function_call_arguments.done") {
            self.handle_function_call_arguments_done(&value);
            return;
        }
        if event_type.ends_with("output_item.done") {
            self.handle_output_item_done(&value);
            return;
        }
        if event_type.ends_with("response.completed") {
            self.handle_response_completed(&value);
            return;
        }
    }

    fn handle_output_text_delta(&mut self, value: &Value, token_texts: &mut Vec<String>) {
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        token_texts.push(delta.to_string());
        self.ensure_message_start();
        let index = self.ensure_text_block();
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "text_delta", "text": delta }
            }),
        ));
    }

    fn handle_output_item_added(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");

        let tool_use_id = if !call_id.is_empty() {
            call_id.to_string()
        } else if !item_id.is_empty() {
            item_id.to_string()
        } else {
            "tool_use_proxy".to_string()
        };

        self.ensure_message_start();
        self.ensure_tool_use_block(item_id, &tool_use_id, name);
    }

    fn handle_function_call_arguments_delta(&mut self, value: &Value) {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        self.ensure_message_start();
        self.ensure_tool_use_state(item_id);
        if !self.tool_uses.get(item_id).is_some_and(|state| state.sent_start) {
            self.start_tool_use_block(item_id);
        }
        self.set_active_tool_use(item_id);
        let Some(index) = self.tool_uses.get(item_id).map(|state| state.index) else {
            return;
        };
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "input_json_delta", "partial_json": delta }
            }),
        ));
    }

    fn handle_function_call_arguments_done(&mut self, value: &Value) {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return;
        };
        self.ensure_message_start();
        self.ensure_tool_use_state(item_id);
        self.stop_tool_use_block(item_id);
    }

    fn handle_output_item_done(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let Some(item_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        self.ensure_message_start();
        self.ensure_tool_use_state(item_id);
        self.stop_tool_use_block(item_id);
    }

    fn handle_response_completed(&mut self, value: &Value) {
        let Some(response) = value.get("response").and_then(Value::as_object) else {
            return;
        };
        let Some(output) = response.get("output").and_then(Value::as_array) else {
            return;
        };
        for item in output {
            let Some(item) = item.as_object() else {
                continue;
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                continue;
            }
            if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let tool_use_id = if !call_id.is_empty() {
                    call_id.to_string()
                } else {
                    item_id.to_string()
                };
                self.ensure_tool_use_block(item_id, &tool_use_id, name);
                self.stop_tool_use_block(item_id);
            }
        }
    }

    fn ensure_message_start(&mut self) {
        if self.sent_message_start {
            return;
        }
        self.sent_message_start = true;

        // Usage is best-effort: OpenAI responses stream may not expose input tokens early.
        let message = json!({
            "id": self.message_id.as_str(),
            "type": "message",
            "role": "assistant",
            "model": self.model.as_str(),
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": { "input_tokens": 0, "output_tokens": 0 }
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

    fn ensure_tool_use_block(&mut self, item_id: &str, tool_use_id: &str, name: &str) {
        if !self.tool_uses.contains_key(item_id) {
            let index = self.next_block_index;
            self.next_block_index += 1;
            self.tool_uses.insert(item_id.to_string(), ToolUseState {
                index,
                tool_use_id: tool_use_id.to_string(),
                name: name.to_string(),
                sent_start: false,
                sent_stop: false,
            });
        }

        if let Some(state) = self.tool_uses.get_mut(item_id) {
            if state.tool_use_id.is_empty() {
                state.tool_use_id = tool_use_id.to_string();
            }
            if state.name.is_empty() {
                state.name = name.to_string();
            }
        }

        if !self.tool_uses.get(item_id).is_some_and(|state| state.sent_start) {
            self.start_tool_use_block(item_id);
        }
    }

    fn ensure_tool_use_state(&mut self, item_id: &str) -> &mut ToolUseState {
        self.tool_uses.entry(item_id.to_string()).or_insert_with(|| {
            let index = self.next_block_index;
            self.next_block_index += 1;
            ToolUseState {
                index,
                tool_use_id: item_id.to_string(),
                name: String::new(),
                sent_start: false,
                sent_stop: false,
            }
        })
    }

    fn start_tool_use_block(&mut self, item_id: &str) {
        let Some((index, tool_use_id, name, sent_start)) = self.tool_uses.get(item_id).map(|state| {
            (
                state.index,
                state.tool_use_id.clone(),
                state.name.clone(),
                state.sent_start,
            )
        }) else {
            return;
        };
        if sent_start {
            return;
        }

        self.stop_active_block();
        if let Some(state) = self.tool_uses.get_mut(item_id) {
            state.sent_start = true;
        }
        self.saw_tool_use = true;
        self.active_block = Some(ActiveBlock::ToolUse {
            item_id: item_id.to_string(),
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

    fn set_active_tool_use(&mut self, item_id: &str) {
        if !self.tool_uses.contains_key(item_id) {
            return;
        };
        match &self.active_block {
            Some(ActiveBlock::ToolUse { item_id: active }) if active == item_id => {}
            _ => {
                self.stop_active_block();
                self.active_block = Some(ActiveBlock::ToolUse {
                    item_id: item_id.to_string(),
                });
            }
        }
    }

    fn stop_tool_use_block(&mut self, item_id: &str) {
        let Some(state) = self.tool_uses.get_mut(item_id) else {
            return;
        };
        if state.sent_stop {
            return;
        }
        state.sent_stop = true;
        if matches!(
            &self.active_block,
            Some(ActiveBlock::ToolUse { item_id: active }) if active == item_id
        ) {
            self.active_block = None;
        }
        self.out.push_back(super::anthropic_event_sse(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": state.index }),
        ));
    }

    fn stop_active_block(&mut self) {
        let Some(active) = self.active_block.take() else {
            return;
        };
        match active {
            ActiveBlock::Text { index } => {
                self.out.push_back(super::anthropic_event_sse(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": index }),
                ));
            }
            ActiveBlock::ToolUse { item_id } => {
                self.stop_tool_use_block(&item_id);
            }
        }
    }

    fn finish_message_if_needed(&mut self) {
        if self.sent_message_stop {
            return;
        }
        self.ensure_message_start();
        self.stop_active_block();

        let stop_reason = if self.saw_tool_use {
            "tool_use"
        } else {
            "end_turn"
        };
        let usage = self.collector.finish();
        let (input_tokens, output_tokens) = usage
            .usage
            .as_ref()
            .map(|u| (u.input_tokens.unwrap_or(0), u.output_tokens.unwrap_or(0)))
            .unwrap_or((0, 0));
        let mut usage_obj = Map::new();
        usage_obj.insert("input_tokens".to_string(), json!(input_tokens));
        usage_obj.insert("output_tokens".to_string(), json!(output_tokens));
        if let Some(cached) = usage.cached_tokens {
            // Best-effort mapping: treat cached tokens as "cache_read_input_tokens".
            usage_obj.insert("cache_read_input_tokens".to_string(), json!(cached));
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
        let entry = build_log_entry(&self.context, self.collector.finish(), None);
        self.log.clone().write_detached(entry);
        self.logged = true;
    }
}
