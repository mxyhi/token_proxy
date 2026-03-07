use serde_json::{json, Map, Value};

use super::super::super::kiro_to_responses_helpers::{
    detect_event_type, extract_error, update_stop_reason, update_usage,
};
use super::super::kiro_to_anthropic_helpers::split_partial_tag;
use super::KiroToAnthropicState;
use crate::proxy::kiro::tool_parser::process_tool_use_event;

impl<S, E> KiroToAnthropicState<S>
where
    S: futures_util::stream::Stream<Item = Result<axum::body::Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    pub(super) async fn handle_message(&mut self, payload: &[u8], event_type: &str) {
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
        if !self.sent_message_start {
            self.ensure_message_start();
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
            return;
        }

        if let Some(text) = event.get("text").and_then(Value::as_str) {
            self.emit_thinking_delta(text).await;
        }
    }

    async fn handle_text_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.raw_content.push_str(delta);

        let mut combined = String::new();
        if !self.thinking_state.pending.is_empty() {
            combined.push_str(&self.thinking_state.pending);
            self.thinking_state.pending.clear();
        }
        combined.push_str(delta);
        self.process_thinking_delta(&combined).await;
        self.maybe_emit_usage_ping();
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

    pub(super) async fn flush_thinking_pending(&mut self) {
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
}

fn tool_use_id(source: &Map<String, Value>) -> Option<&str> {
    source
        .get("toolUseId")
        .or_else(|| source.get("tool_use_id"))
        .and_then(Value::as_str)
}
