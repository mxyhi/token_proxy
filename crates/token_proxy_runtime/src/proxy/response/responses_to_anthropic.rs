use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use super::super::compat_reason;
use super::super::log::{attach_response_body, build_log_entry, LogContext, LogWriter};
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;
use super::responses_error::{responses_stream_error, ResponsesStreamError};
use super::streaming::STREAM_DROPPED_ERROR;
use crate::proxy::anthropic_compat::web_search;
use crate::proxy::claude_reasoning;

pub(super) fn stream_responses_to_anthropic<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
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
    Thinking { index: usize },
    ToolUse { item_id: String },
    WebSearch { item_id: String },
}

struct ToolUseState {
    index: usize,
    tool_use_id: String,
    name: String,
    sent_start: bool,
    sent_stop: bool,
    sent_input: bool,
}

struct ReasoningBlockState {
    index: usize,
    sent_start: bool,
    sent_stop: bool,
    sent_delta: bool,
    sent_signature: bool,
}

struct WebSearchState {
    index: usize,
    result_index: usize,
    tool_use_id: String,
    query: String,
    results: Option<Value>,
    sent_start: bool,
    sent_stop: bool,
    sent_result: bool,
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
    stream_failed: bool,
    logged: bool,
    upstream_ended: bool,
    active_block: Option<ActiveBlock>,
    next_block_index: usize,
    tool_uses: HashMap<String, ToolUseState>,
    reasoning_blocks: HashMap<String, ReasoningBlockState>,
    web_searches: HashMap<String, WebSearchState>,
    redacted_reasoning_emitted: HashSet<String>,
    saw_tool_use: bool,
    stop_reason_override: Option<&'static str>,
    saw_reasoning_delta: bool,
    response_body_buf: String,
}

impl<S> ResponsesToAnthropicState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        let mut entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        attach_response_body(&mut entry, &self.response_body_buf);
        self.log.clone().write_detached(entry);
        self.logged = true;
    }
}

impl<S> Drop for ResponsesToAnthropicState<S> {
    fn drop(&mut self) {
        // 若下游中途取消，Drop 仍会触发，保证请求日志不丢。
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
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
            stream_failed: false,
            logged: false,
            upstream_ended: false,
            active_block: None,
            next_block_index: 0,
            tool_uses: HashMap::new(),
            reasoning_blocks: HashMap::new(),
            web_searches: HashMap::new(),
            redacted_reasoning_emitted: HashSet::new(),
            saw_tool_use: false,
            stop_reason_override: None,
            saw_reasoning_delta: false,
            response_body_buf: String::new(),
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
                    self.response_body_buf
                        .push_str(&String::from_utf8_lossy(chunk.as_ref()));
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
        if self.sent_message_stop || self.stream_failed {
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
        if let Some(error) = responses_stream_error(&value) {
            self.fail_stream(error);
            return;
        }

        if event_type.ends_with("output_text.delta") {
            self.handle_output_text_delta(&value, token_texts);
            return;
        }
        if event_type.ends_with("reasoning_text.delta")
            || event_type.ends_with("reasoning_summary_text.delta")
        {
            self.handle_reasoning_text_delta(&value, token_texts);
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
        if event_type.ends_with("response.incomplete") {
            self.handle_response_incomplete(&value);
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

    fn handle_reasoning_text_delta(&mut self, value: &Value, token_texts: &mut Vec<String>) {
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        self.saw_reasoning_delta = true;
        token_texts.push(delta.to_string());
        self.ensure_message_start();
        let index = match value.get("item_id").and_then(Value::as_str) {
            Some(item_id) if !item_id.is_empty() => {
                let index = self.ensure_reasoning_block(item_id);
                if let Some(state) = self.reasoning_blocks.get_mut(item_id) {
                    state.sent_delta = true;
                }
                index
            }
            _ => self.ensure_thinking_block(),
        };
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "thinking_delta", "thinking": delta }
            }),
        ));
    }

    fn handle_output_item_added(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
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
            Some("reasoning") => {
                let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                    return;
                };
                self.ensure_message_start();
                self.ensure_reasoning_block(item_id);
            }
            Some("web_search_call") => {
                let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                if !item_id.is_empty() {
                    self.ensure_message_start();
                    self.ensure_web_search(item_id, item);
                }
            }
            _ => {}
        }
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
        if !self
            .tool_uses
            .get(item_id)
            .is_some_and(|state| state.sent_start)
        {
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
        // Claude Code 会把 input_json_delta 的 partial_json 逐段拼接成最终 JSON。
        // 若我们在 arguments.done 再发送一次完整 arguments，会导致拼接重复并变成非法 JSON（最终 tool input 变成 {}）。
        if let Some(state) = self.tool_uses.get_mut(item_id) {
            state.sent_input = true;
        }
    }

    fn handle_function_call_arguments_done(&mut self, value: &Value) {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let arguments = value.get("arguments").and_then(Value::as_str).unwrap_or("");
        self.ensure_message_start();
        self.ensure_tool_use_state(item_id);
        self.emit_tool_use_arguments(item_id, arguments);
        self.stop_tool_use_block(item_id);
    }

    fn handle_output_item_done(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                    return;
                };
                self.ensure_message_start();
                self.ensure_tool_use_state(item_id);
                self.stop_tool_use_block(item_id);
            }
            Some("reasoning") => {
                let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                    return;
                };
                self.ensure_message_start();
                if claude_reasoning::redacted_thinking_data(
                    item.get("encrypted_content")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                )
                .is_some()
                {
                    self.stop_reasoning_block(item_id);
                    self.emit_redacted_reasoning(item);
                } else {
                    if let Some(signature) = item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        self.emit_reasoning_signature(item_id, signature);
                    }
                    self.stop_reasoning_block(item_id);
                }
            }
            Some("web_search_call") => {
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    self.ensure_message_start();
                    self.ensure_web_search(item_id, item);
                    self.emit_web_search_result(item_id, item.get("results"));
                }
            }
            _ => {}
        }
    }

    fn handle_response_completed(&mut self, value: &Value) {
        let Some(response) = value.get("response").and_then(Value::as_object) else {
            return;
        };
        self.handle_response_output_items(response);
        self.stop_reason_override = Some(
            compat_reason::anthropic_stop_reason_from_chat_finish_reason(
                compat_reason::chat_finish_reason_from_response_object(response, self.saw_tool_use),
            ),
        );
    }

    fn handle_response_incomplete(&mut self, value: &Value) {
        let Some(response) = value.get("response").and_then(Value::as_object) else {
            return;
        };
        self.handle_response_output_items(response);
        self.stop_reason_override = Some(
            compat_reason::anthropic_stop_reason_from_chat_finish_reason(
                compat_reason::chat_finish_reason_from_response_object(response, self.saw_tool_use),
            ),
        );
    }

    fn handle_response_output_items(&mut self, response: &Map<String, Value>) {
        let Some(output) = response.get("output").and_then(Value::as_array) else {
            return;
        };
        let mut reasoning_snapshot = String::new();
        for item in output {
            let Some(item) = item.as_object() else {
                continue;
            };
            match item.get("type").and_then(Value::as_str) {
                Some("function_call") => {
                    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                        let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                        let tool_use_id = if !call_id.is_empty() {
                            call_id.to_string()
                        } else {
                            item_id.to_string()
                        };
                        self.ensure_tool_use_block(item_id, &tool_use_id, name);
                        self.emit_tool_use_arguments(item_id, arguments);
                        self.stop_tool_use_block(item_id);
                    }
                }
                Some("reasoning") => {
                    if self.emit_redacted_reasoning(item) {
                        continue;
                    }
                    let summary = extract_reasoning_text_from_item(item);
                    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                        let Some(signature) = item
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                        else {
                            continue;
                        };
                        let already_emitted = self
                            .reasoning_blocks
                            .get(item_id)
                            .is_some_and(|state| state.sent_delta);
                        if !already_emitted {
                            self.emit_reasoning_summary_for_item(item_id, &summary);
                        }
                        self.emit_reasoning_signature(item_id, signature);
                        self.stop_reasoning_block(item_id);
                    } else if reasoning_snapshot.is_empty() && !summary.is_empty() {
                        reasoning_snapshot = summary;
                    }
                }
                Some("message") => {
                    if item.get("role").and_then(Value::as_str) != Some("assistant") {
                        continue;
                    }
                    let Some(content) = item.get("content").and_then(Value::as_array) else {
                        continue;
                    };
                    if reasoning_snapshot.is_empty() {
                        reasoning_snapshot = extract_reasoning_text(content);
                    }
                }
                Some("web_search_call") => {
                    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                        self.ensure_message_start();
                        self.ensure_web_search(item_id, item);
                        self.emit_web_search_result(item_id, item.get("results"));
                    }
                }
                _ => {}
            }
        }
        self.emit_reasoning_snapshot(&reasoning_snapshot);
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

    fn ensure_reasoning_state(&mut self, item_id: &str) -> &mut ReasoningBlockState {
        self.reasoning_blocks
            .entry(item_id.to_string())
            .or_insert_with(|| {
                let index = self.next_block_index;
                self.next_block_index += 1;
                ReasoningBlockState {
                    index,
                    sent_start: false,
                    sent_stop: false,
                    sent_delta: false,
                    sent_signature: false,
                }
            })
    }

    fn ensure_reasoning_block(&mut self, item_id: &str) -> usize {
        let index = self.ensure_reasoning_state(item_id).index;
        let sent_start = self
            .reasoning_blocks
            .get(item_id)
            .is_some_and(|state| state.sent_start);
        if !sent_start {
            self.start_reasoning_block(item_id);
            return index;
        }
        if !matches!(
            self.active_block,
            Some(ActiveBlock::Thinking { index: active }) if active == index
        ) {
            self.stop_active_block();
            self.active_block = Some(ActiveBlock::Thinking { index });
        }
        index
    }

    fn start_reasoning_block(&mut self, item_id: &str) {
        let index = self.ensure_reasoning_state(item_id).index;
        let sent_start = self
            .reasoning_blocks
            .get(item_id)
            .is_some_and(|state| state.sent_start);
        if sent_start {
            return;
        }

        self.stop_active_block();
        if let Some(state) = self.reasoning_blocks.get_mut(item_id) {
            state.sent_start = true;
        }
        self.active_block = Some(ActiveBlock::Thinking { index });
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
        ));
    }

    fn emit_reasoning_summary_for_item(&mut self, item_id: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        self.saw_reasoning_delta = true;
        self.ensure_message_start();
        let index = self.ensure_reasoning_block(item_id);
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "thinking_delta", "thinking": text }
            }),
        ));
        if let Some(state) = self.reasoning_blocks.get_mut(item_id) {
            state.sent_delta = true;
        }
    }

    fn emit_reasoning_signature(&mut self, item_id: &str, signature: &str) {
        if signature.is_empty()
            || self
                .reasoning_blocks
                .get(item_id)
                .is_some_and(|state| state.sent_signature || state.sent_stop)
        {
            return;
        }
        self.ensure_message_start();
        let index = self.ensure_reasoning_block(item_id);
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "signature_delta", "signature": signature }
            }),
        ));
        if let Some(state) = self.reasoning_blocks.get_mut(item_id) {
            state.sent_signature = true;
        }
    }

    fn emit_redacted_reasoning(&mut self, item: &Map<String, Value>) -> bool {
        let Some(encrypted_content) = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return false;
        };
        let Some(data) = claude_reasoning::redacted_thinking_data(encrypted_content) else {
            return false;
        };

        let dedupe_key = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(encrypted_content)
            .to_string();
        if !self.redacted_reasoning_emitted.insert(dedupe_key) {
            return true;
        }

        self.ensure_message_start();
        self.stop_active_block();
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "redacted_thinking",
                    "data": data
                }
            }),
        ));
        self.out.push_back(super::anthropic_event_sse(
            "content_block_stop",
            json!({
                "type": "content_block_stop",
                "index": index
            }),
        ));
        true
    }

    fn stop_reasoning_block(&mut self, item_id: &str) {
        let Some(state) = self.reasoning_blocks.get_mut(item_id) else {
            return;
        };
        if state.sent_stop || !state.sent_start {
            return;
        }
        state.sent_stop = true;
        if matches!(
            &self.active_block,
            Some(ActiveBlock::Thinking { index }) if *index == state.index
        ) {
            self.active_block = None;
        }
        self.out.push_back(super::anthropic_event_sse(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": state.index }),
        ));
    }

    fn emit_reasoning_snapshot(&mut self, text: &str) {
        if self.saw_reasoning_delta || text.trim().is_empty() {
            return;
        }
        self.saw_reasoning_delta = true;
        self.ensure_message_start();
        let index = self.ensure_thinking_block();
        self.out.push_back(super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "thinking_delta", "thinking": text }
            }),
        ));
        self.stop_active_block();
    }

    fn ensure_tool_use_block(&mut self, item_id: &str, tool_use_id: &str, name: &str) {
        if !self.tool_uses.contains_key(item_id) {
            let index = self.next_block_index;
            self.next_block_index += 1;
            self.tool_uses.insert(
                item_id.to_string(),
                ToolUseState {
                    index,
                    tool_use_id: tool_use_id.to_string(),
                    name: name.to_string(),
                    sent_start: false,
                    sent_stop: false,
                    sent_input: false,
                },
            );
        }

        if let Some(state) = self.tool_uses.get_mut(item_id) {
            if state.tool_use_id.is_empty() {
                state.tool_use_id = tool_use_id.to_string();
            }
            if state.name.is_empty() {
                state.name = name.to_string();
            }
        }

        if !self
            .tool_uses
            .get(item_id)
            .is_some_and(|state| state.sent_start)
        {
            self.start_tool_use_block(item_id);
        }
    }

    fn ensure_tool_use_state(&mut self, item_id: &str) -> &mut ToolUseState {
        self.tool_uses
            .entry(item_id.to_string())
            .or_insert_with(|| {
                let index = self.next_block_index;
                self.next_block_index += 1;
                ToolUseState {
                    index,
                    tool_use_id: item_id.to_string(),
                    name: String::new(),
                    sent_start: false,
                    sent_stop: false,
                    sent_input: false,
                }
            })
    }

    fn start_tool_use_block(&mut self, item_id: &str) {
        let Some((index, tool_use_id, name, sent_start)) =
            self.tool_uses.get(item_id).map(|state| {
                (
                    state.index,
                    state.tool_use_id.clone(),
                    state.name.clone(),
                    state.sent_start,
                )
            })
        else {
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

    fn emit_tool_use_arguments(&mut self, item_id: &str, arguments: &str) {
        if arguments.trim().is_empty() {
            return;
        }
        let state = self.ensure_tool_use_state(item_id);
        if state.sent_input {
            return;
        }
        if !state.sent_start {
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
                "delta": { "type": "input_json_delta", "partial_json": arguments }
            }),
        ));
        if let Some(state) = self.tool_uses.get_mut(item_id) {
            state.sent_input = true;
        }
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

    fn ensure_web_search(&mut self, item_id: &str, item: &Map<String, Value>) {
        let query = web_search::responses_search_query(item);
        if !self.web_searches.contains_key(item_id) {
            let index = self.next_block_index;
            self.next_block_index += 1;
            let result_index = self.next_block_index;
            self.next_block_index += 1;
            self.web_searches.insert(
                item_id.to_string(),
                WebSearchState {
                    index,
                    result_index,
                    tool_use_id: web_search::claude_search_id(item_id),
                    query,
                    results: None,
                    sent_start: false,
                    sent_stop: false,
                    sent_result: false,
                },
            );
        } else if let Some(state) = self.web_searches.get_mut(item_id) {
            if state.query.is_empty() {
                state.query = query;
            }
        }

        let Some((index, tool_use_id, query, sent_start)) =
            self.web_searches.get(item_id).map(|state| {
                (
                    state.index,
                    state.tool_use_id.clone(),
                    state.query.clone(),
                    state.sent_start,
                )
            })
        else {
            return;
        };
        if sent_start {
            return;
        }
        self.stop_active_block();
        if let Some(state) = self.web_searches.get_mut(item_id) {
            state.sent_start = true;
        }
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "server_tool_use",
                    "id": tool_use_id,
                    "name": web_search::CLAUDE_WEB_SEARCH_NAME,
                    "input": { "query": query }
                }
            }),
        ));
        self.active_block = Some(ActiveBlock::WebSearch {
            item_id: item_id.to_string(),
        });
    }

    fn emit_web_search_result(&mut self, item_id: &str, results: Option<&Value>) {
        let Some((result_index, tool_use_id, content)) =
            self.web_searches.get_mut(item_id).and_then(|state| {
                if let Some(results) = results {
                    state.results = Some(results.clone());
                }
                if state.sent_result {
                    return None;
                }
                state.sent_result = true;
                Some((
                    state.result_index,
                    state.tool_use_id.clone(),
                    web_search::claude_search_results_from_responses(state.results.as_ref()),
                ))
            })
        else {
            return;
        };
        self.stop_active_block();
        self.stop_web_search_block(item_id);
        self.out.push_back(super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": result_index,
                "content_block": {
                    "type": "web_search_tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content
                }
            }),
        ));
        self.out.push_back(super::anthropic_event_sse(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": result_index }),
        ));
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
            ActiveBlock::Thinking { index } => {
                self.out.push_back(super::anthropic_event_sse(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": index }),
                ));
            }
            ActiveBlock::ToolUse { item_id } => {
                self.stop_tool_use_block(&item_id);
            }
            ActiveBlock::WebSearch { item_id } => {
                self.stop_web_search_block(&item_id);
            }
        }
    }

    fn stop_web_search_block(&mut self, item_id: &str) {
        let Some(index) = self.web_searches.get(item_id).map(|state| state.index) else {
            return;
        };
        let Some(state) = self.web_searches.get_mut(item_id) else {
            return;
        };
        if state.sent_stop {
            return;
        }
        state.sent_stop = true;
        if matches!(
            &self.active_block,
            Some(ActiveBlock::WebSearch { item_id: active }) if active == item_id
        ) {
            self.active_block = None;
        }
        self.out.push_back(super::anthropic_event_sse(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": index }),
        ));
    }

    fn finish_message_if_needed(&mut self) {
        if self.sent_message_stop || self.stream_failed {
            return;
        }
        self.ensure_message_start();
        self.stop_active_block();

        let stop_reason = self.stop_reason_override.unwrap_or_else(|| {
            if self.saw_tool_use {
                "tool_use"
            } else {
                "end_turn"
            }
        });
        let usage = self.collector.finish();
        let (input_tokens, output_tokens) = usage
            .usage
            .as_ref()
            .map(|u| (u.input_tokens.unwrap_or(0), u.output_tokens.unwrap_or(0)))
            .unwrap_or((0, 0));
        let mut usage_obj = Map::new();
        usage_obj.insert("input_tokens".to_string(), json!(input_tokens));
        usage_obj.insert("output_tokens".to_string(), json!(output_tokens));
        let billable = &usage.billable_usage;
        if billable.cache_read_tokens > 0 {
            usage_obj.insert(
                "cache_read_input_tokens".to_string(),
                json!(billable.cache_read_tokens),
            );
        }
        let cache_write = billable
            .cache_write_tokens
            .saturating_add(billable.cache_write_5m_tokens)
            .saturating_add(billable.cache_write_1h_tokens);
        if cache_write > 0 {
            usage_obj.insert(
                "cache_creation_input_tokens".to_string(),
                json!(cache_write),
            );
            if billable.cache_write_5m_tokens > 0 || billable.cache_write_1h_tokens > 0 {
                usage_obj.insert(
                    "cache_creation".to_string(),
                    json!({
                        "ephemeral_5m_input_tokens": billable.cache_write_5m_tokens,
                        "ephemeral_1h_input_tokens": billable.cache_write_1h_tokens,
                    }),
                );
            }
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

    fn fail_stream(&mut self, error: ResponsesStreamError) {
        let message = error.message.clone();
        self.context.status = error.status.as_u16();
        self.stream_failed = true;
        self.upstream_ended = true;
        self.out.push_back(anthropic_error_sse(&error));
        self.write_log_once(Some(message));
    }

    fn log_usage_once(&mut self) {
        self.write_log_once(None);
    }
}

pub(super) fn anthropic_error_sse(error: &ResponsesStreamError) -> Bytes {
    super::anthropic_event_sse(
        "error",
        json!({
            "type": "error",
            "error": Value::Object(error.openai_error_object())
        }),
    )
}

fn extract_reasoning_text(parts: &[Value]) -> String {
    let mut reasoning = String::new();
    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("reasoning_text") {
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            reasoning.push_str(text);
        }
    }
    reasoning
}

fn extract_reasoning_summary(item: &Map<String, Value>) -> String {
    let Some(summary) = item.get("summary").and_then(Value::as_array) else {
        return String::new();
    };
    let mut combined = String::new();
    for part in summary {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("summary_text") {
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            combined.push_str(text);
        }
    }
    combined
}

fn extract_reasoning_text_from_item(item: &Map<String, Value>) -> String {
    let summary = extract_reasoning_summary(item);
    if !summary.is_empty() {
        return summary;
    }
    item.get("content")
        .and_then(Value::as_array)
        .map(|parts| extract_reasoning_text(parts))
        .unwrap_or_default()
}
