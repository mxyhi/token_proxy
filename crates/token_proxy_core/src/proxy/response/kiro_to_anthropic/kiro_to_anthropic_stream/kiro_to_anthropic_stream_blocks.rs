use axum::body::Bytes;
use serde_json::{json, Map, Value};
use std::time::Instant;

use super::{ActiveBlock, KiroToAnthropicState, ToolUseState, USAGE_UPDATE_CHAR_THRESHOLD, USAGE_UPDATE_TIME_INTERVAL, USAGE_UPDATE_TOKEN_DELTA};
use crate::proxy::log::{build_log_entry, UsageSnapshot};
use crate::proxy::token_estimator;
use super::super::super::kiro_to_responses_helpers::{
    apply_usage_fallback,
    usage_from_kiro,
    usage_json_from_kiro,
};

impl<S, E> KiroToAnthropicState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    pub(super) async fn emit_text_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.ensure_message_start();
        let index = self.ensure_text_block();
        self.content.push_str(delta);
        self.token_tracker.add_output_text(delta).await;
        self.out.push_back(super::super::super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "text_delta", "text": delta }
            }),
        ));
    }

    pub(super) async fn emit_thinking_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.ensure_message_start();
        let index = self.ensure_thinking_block();
        self.reasoning.push_str(delta);
        self.token_tracker.add_output_text(delta).await;
        self.out.push_back(super::super::super::anthropic_event_sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "thinking_delta", "thinking": delta }
            }),
        ));
    }

    pub(super) fn ensure_message_start(&mut self) {
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
        self.out.push_back(super::super::super::anthropic_event_sse(
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
        self.out.push_back(super::super::super::anthropic_event_sse(
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
        self.out.push_back(super::super::super::anthropic_event_sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
        ));
        index
    }

    pub(super) fn ensure_tool_use_block(&mut self, tool_use_id: &str, name: &str) {
        self.ensure_message_start();
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
        self.out.push_back(super::super::super::anthropic_event_sse(
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

    pub(super) fn emit_tool_use_input(&mut self, tool_use_id: &str, value: &Value) {
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
        self.out.push_back(super::super::super::anthropic_event_sse(
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

    pub(super) fn stop_tool_use_block(&mut self, tool_use_id: &str) {
        let Some(state) = self.tool_uses.get_mut(tool_use_id) else {
            return;
        };
        if state.sent_stop {
            return;
        }
        state.sent_stop = true;
        let index = state.index;
        self.out.push_back(super::super::super::anthropic_event_sse(
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
                self.out.push_back(super::super::super::anthropic_event_sse(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": index }),
                ));
            }
            ActiveBlock::ToolUse { id } => {
                self.stop_tool_use_block(&id);
            }
        }
    }

    pub(super) fn finish_message_if_needed(&mut self) {
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

        self.out.push_back(super::super::super::anthropic_event_sse(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason, "stop_sequence": null },
                "usage": Value::Object(usage_obj)
            }),
        ));
        self.out.push_back(super::super::super::anthropic_event_sse(
            "message_stop",
            json!({ "type": "message_stop" }),
        ));
        self.sent_message_stop = true;
    }

    pub(super) fn log_usage_once(&mut self) {
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

    pub(super) fn maybe_emit_usage_ping(&mut self) {
        let len = self.raw_content.len();
        let should_send = len.saturating_sub(self.last_ping_len) >= USAGE_UPDATE_CHAR_THRESHOLD
            || (self.last_ping_time.elapsed() >= USAGE_UPDATE_TIME_INTERVAL && len > self.last_ping_len);
        if !should_send {
            return;
        }

        let output_tokens = token_estimator::estimate_text_tokens(Some(&self.model), &self.raw_content);
        if output_tokens > self.last_reported_output_tokens + USAGE_UPDATE_TOKEN_DELTA {
            self.ensure_message_start();
            let input_tokens = self.usage.input_tokens.unwrap_or(0);
            self.out.push_back(super::super::super::anthropic_event_sse(
                "ping",
                json!({
                    "type": "ping",
                    "usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "total_tokens": input_tokens.saturating_add(output_tokens),
                        "estimated": true
                    }
                }),
            ));
            self.last_reported_output_tokens = output_tokens;
        }

        self.last_ping_len = len;
        self.last_ping_time = Instant::now();
    }
}
