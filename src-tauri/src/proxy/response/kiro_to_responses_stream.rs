use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use std::{collections::{HashMap, HashSet, VecDeque}, sync::Arc};

use super::super::kiro::{EventStreamDecoder, KiroUsage, KiroToolUse};
use super::super::kiro::tool_parser::{process_tool_use_event, ToolUseState};
use super::super::log::{build_log_entry, LogContext, LogWriter, UsageSnapshot};
use super::super::token_rate::RequestTokenTracker;
use super::kiro_to_responses_helpers::{
    apply_usage_fallback,
    build_response_object,
    collect_tool_uses,
    detect_event_type,
    extract_error,
    update_stop_reason,
    update_usage,
    usage_from_kiro,
    usage_json_from_kiro,
    FunctionCallOutput,
};

pub(super) fn stream_kiro_to_responses<E>(
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
    let state = KiroToResponsesState::new(
        upstream,
        context,
        log,
        token_tracker,
        estimated_input_tokens,
    );
    futures_util::stream::try_unfold(state, |state| async move { state.step().await })
}

struct MessageOutput {
    id: String,
    output_index: u64,
    text: String,
}

struct ThinkingStreamState {
    in_thinking: bool,
    pending: String,
}

struct KiroToResponsesState<S> {
    upstream: S,
    decoder: EventStreamDecoder,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
    out: VecDeque<Bytes>,
    response_id: String,
    created_at: i64,
    model: String,
    next_output_index: u64,
    message: Option<MessageOutput>,
    reasoning: String,
    thinking_state: ThinkingStreamState,
    function_calls: Vec<Option<FunctionCallOutput>>,
    tool_call_by_id: HashMap<String, usize>,
    processed_tool_keys: HashSet<String>,
    tool_state: Option<ToolUseState>,
    usage: KiroUsage,
    stop_reason: Option<String>,
    sequence: u64,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
}

impl<S, E> KiroToResponsesState<S>
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
        let created_at = (now_ms / 1000) as i64;
        let model = context
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let mut state = Self {
            upstream,
            decoder: EventStreamDecoder::new(),
            log,
            context,
            token_tracker,
            estimated_input_tokens,
            out: VecDeque::new(),
            response_id: format!("resp_{now_ms}"),
            created_at,
            model,
            next_output_index: 0,
            message: None,
            reasoning: String::new(),
            thinking_state: ThinkingStreamState {
                in_thinking: false,
                pending: String::new(),
            },
            function_calls: Vec::new(),
            tool_call_by_id: HashMap::new(),
            processed_tool_keys: HashSet::new(),
            tool_state: None,
            usage: KiroUsage::default(),
            stop_reason: None,
            sequence: 0,
            sent_done: false,
            logged: false,
            upstream_ended: false,
        };
        state.push_response_created();
        state
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
        if !self.sent_done {
            self.push_done();
        }
        self.log_usage_once();
        Ok(())
    }

    async fn handle_message(&mut self, payload: &[u8], event_type: &str) {
        if self.sent_done || payload.is_empty() {
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
                self.push_error(error);
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
            "followupPromptEvent" => {}
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
                self.emit_reasoning_delta(text).await;
            }
            if let Some(text) = reasoning.get("text").and_then(Value::as_str) {
                self.emit_reasoning_delta(text).await;
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
        self.ensure_message_output();
        if let Some(message) = self.message.as_mut() {
            message.text.push_str(delta);
        }
        self.token_tracker.add_output_text(delta).await;
        let item_id = self.message.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let output_index = self.message.as_ref().map(|m| m.output_index).unwrap_or(0);
        self.push_event(json!({
            "type": "response.output_text.delta",
            "item_id": item_id,
            "output_index": output_index,
            "content_index": 0,
            "delta": delta
        }));
    }

    async fn emit_reasoning_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.ensure_message_output();
        self.reasoning.push_str(delta);
        self.token_tracker.add_output_text(delta).await;
        let item_id = self.message.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let output_index = self.message.as_ref().map(|m| m.output_index).unwrap_or(0);
        self.push_event(json!({
            "type": "response.reasoning_text.delta",
            "item_id": item_id,
            "output_index": output_index,
            "content_index": 0,
            "delta": delta
        }));
    }

    async fn process_thinking_delta(&mut self, input: &str) {
        const START: &str = "<thinking>";
        const END: &str = "</thinking>";

        let mut cursor = 0;
        // Parse <thinking> tags incrementally so reasoning never leaks into output_text.
        while cursor < input.len() {
            if self.thinking_state.in_thinking {
                if let Some(pos) = input[cursor..].find(END) {
                    let end = cursor + pos;
                    if end > cursor {
                        self.emit_reasoning_delta(&input[cursor..end]).await;
                    }
                    cursor = end + END.len();
                    self.thinking_state.in_thinking = false;
                    continue;
                }
                let (emit, pending) = split_partial_tag(&input[cursor..], END);
                if !emit.is_empty() {
                    self.emit_reasoning_delta(&emit).await;
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
            self.emit_reasoning_delta(&pending).await;
        } else {
            self.emit_text_delta(&pending).await;
        }
    }

    async fn handle_tool_use_event(&mut self, event: &Map<String, Value>) {
        let (completed, next_state) =
            process_tool_use_event(event, self.tool_state.take(), &mut self.processed_tool_keys);
        self.tool_state = next_state;
        for tool_use in completed {
            self.ensure_function_call_output(&tool_use);
            self.finalize_function_call(&tool_use);
        }
    }

    fn handle_tool_uses(&mut self, items: &[Value]) {
        for item in items {
            let Some(tool) = item.as_object() else {
                continue;
            };
            let tool_use_id = tool
                .get("toolUseId")
                .or_else(|| tool.get("tool_use_id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let dedupe_key = format!("id:{tool_use_id}");
            if tool_use_id.is_empty() || self.processed_tool_keys.contains(&dedupe_key) {
                continue;
            }
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            let input = tool
                .get("input")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            self.processed_tool_keys.insert(dedupe_key);
            let tool_use = KiroToolUse {
                tool_use_id: tool_use_id.to_string(),
                name: name.to_string(),
                input,
            };
            self.ensure_function_call_output(&tool_use);
            self.finalize_function_call(&tool_use);
        }
    }

    fn ensure_message_output(&mut self) {
        if self.message.is_some() {
            return;
        }
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        let message_id = format!("msg_{}", self.response_id);
        self.push_event(json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {
                "id": message_id,
                "type": "message",
                "status": "in_progress",
                "role": "assistant",
                "content": []
            }
        }));
        self.message = Some(MessageOutput {
            id: message_id,
            output_index,
            text: String::new(),
        });
    }

    fn ensure_function_call_output(&mut self, tool_use: &KiroToolUse) {
        let index = if let Some(index) = self.tool_call_by_id.get(&tool_use.tool_use_id) {
            *index
        } else {
            let index = self.function_calls.len();
            self.tool_call_by_id
                .insert(tool_use.tool_use_id.clone(), index);
            index
        };
        if self.function_calls.len() <= index {
            self.function_calls.resize_with(index + 1, || None);
        }

        if self.function_calls[index].is_none() {
            let output_index = self.next_output_index;
            self.next_output_index += 1;
            let item_id = format!("fc_{}", tool_use.tool_use_id);
            let call_id = tool_use.tool_use_id.clone();
            let name = tool_use.name.clone();
            self.push_event(json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "id": item_id,
                    "type": "function_call",
                    "status": "in_progress",
                    "call_id": call_id,
                    "name": name,
                    "arguments": ""
                }
            }));
            self.function_calls[index] = Some(FunctionCallOutput {
                id: format!("fc_{}", tool_use.tool_use_id),
                output_index,
                call_id: tool_use.tool_use_id.clone(),
                name: tool_use.name.clone(),
                arguments: String::new(),
            });
        }
    }

    fn finalize_function_call(&mut self, tool_use: &KiroToolUse) {
        let Some(index) = self.tool_call_by_id.get(&tool_use.tool_use_id).copied() else {
            return;
        };
        let Some(state) = self.function_calls.get_mut(index).and_then(Option::as_mut) else {
            return;
        };
        if state.arguments.is_empty() {
            state.arguments = serde_json::to_string(&tool_use.input).unwrap_or_default();
        }
        let item_id = state.id.clone();
        let output_index = state.output_index;
        let name = state.name.clone();
        let call_id = state.call_id.clone();
        let arguments = state.arguments.clone();
        self.push_event(json!({
            "type": "response.function_call_arguments.done",
            "item_id": item_id,
            "output_index": output_index,
            "name": name,
            "arguments": arguments
        }));
        self.push_event(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments
            }
        }));
    }

    fn push_response_created(&mut self) {
        self.push_event(json!({
            "type": "response.created",
            "response": {
                "id": self.response_id,
                "object": "response",
                "created_at": self.created_at,
                "status": "in_progress",
                "model": self.model
            }
        }));
    }

    fn push_done(&mut self) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;
        self.finalize_usage();
        if self.stop_reason.is_none() {
            self.stop_reason = Some(if self.function_calls.iter().any(|call| call.is_some()) {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            });
        }
        let tool_uses = collect_tool_uses(&self.function_calls);
        let response = build_response_object(
            self.message
                .as_ref()
                .map(|message| message.text.clone())
                .unwrap_or_default(),
            self.reasoning.clone(),
            tool_uses,
            self.usage.clone(),
            self.stop_reason.as_deref(),
            Some(&self.model),
            self.response_id.clone(),
            self.created_at,
        );
        self.push_event(json!({
            "type": "response.completed",
            "response": response
        }));
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
    }

    fn push_event(&mut self, mut event: Value) {
        if let Some(obj) = event.as_object_mut() {
            let sequence_number = self.next_sequence();
            obj.insert("sequence_number".to_string(), Value::Number(sequence_number.into()));
        }
        self.out.push_back(super::responses_event_sse(event));
    }

    fn push_error(&mut self, message: String) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.failed",
            "error": { "message": message }
        })));
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    fn log_usage_once(&mut self) {
        if self.logged {
            return;
        }
        self.logged = true;
        self.finalize_usage();
        let usage_snapshot = UsageSnapshot {
            usage: usage_from_kiro(&self.usage),
            cached_tokens: None,
            usage_json: usage_json_from_kiro(&self.usage),
        };
        let entry = build_log_entry(&self.context, usage_snapshot, None);
        self.log.clone().write_detached(entry);
    }

    fn finalize_usage(&mut self) {
        let content = self
            .message
            .as_ref()
            .map(|message| message.text.as_str())
            .unwrap_or("");
        apply_usage_fallback(
            &mut self.usage,
            Some(&self.model),
            self.estimated_input_tokens,
            content,
            &self.reasoning,
        );
    }
}

fn split_partial_tag(segment: &str, tag: &str) -> (String, String) {
    if tag.len() <= 1 || segment.len() < 1 {
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
