use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc};

use super::super::log::{build_log_entry, LogContext, LogWriter};
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;
use super::streaming::STREAM_DROPPED_ERROR;
use format::{snapshot_to_output_item, usage_to_value, OutputItemSnapshot};
use state_types::{FunctionCallOutput, MessageOutput, ReasoningOutput};

mod format;
mod state_types;

pub(super) fn stream_chat_to_responses<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = ChatToResponsesState::new(upstream, context, log, token_tracker);
    futures_util::stream::try_unfold(state, |state| async move { state.step().await })
}

struct ChatToResponsesState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    id_seed: u64,
    response_id: String,
    created_at: i64,
    model: String,
    next_output_index: u64,
    reasoning: Option<ReasoningOutput>,
    message: Option<MessageOutput>,
    function_calls: Vec<Option<FunctionCallOutput>>,
    sequence: u64,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
}

impl<S> ChatToResponsesState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        self.logged = true;
        let entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        self.log.clone().write_detached(entry);
    }
}

impl<S> Drop for ChatToResponsesState<S> {
    fn drop(&mut self) {
        // 对齐 new-api 的兜底语义：流提前释放也应保留请求日志。
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S, E> ChatToResponsesState<S>
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
        let created_at = (now_ms / 1000) as i64;
        let model = context
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let mut state = Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            context,
            token_tracker,
            out: VecDeque::new(),
            id_seed: now_ms,
            response_id: format!("resp_{now_ms}"),
            created_at,
            model,
            next_output_index: 0,
            reasoning: None,
            message: None,
            function_calls: Vec::new(),
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
                    if self.context.ttfb_ms.is_none() {
                        self.context.ttfb_ms = Some(self.context.start.elapsed().as_millis());
                    }
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
                    if !self.sent_done {
                        self.push_done();
                    }
                    self.log_usage_once();
                    if self.out.is_empty() {
                        return Ok(None);
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, data: &str, token_texts: &mut Vec<String>) {
        if self.sent_done {
            return;
        }
        if data == "[DONE]" {
            self.push_done();
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };

        let Some(delta) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"))
        else {
            return;
        };

        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            self.handle_text_delta(content, token_texts);
        }
        if let Some(reasoning_content) = delta.get("reasoning_content").and_then(Value::as_str) {
            self.handle_reasoning_delta(reasoning_content, token_texts);
        }
        if let Some(thinking_blocks) = delta.get("thinking_blocks").and_then(Value::as_array) {
            self.handle_thinking_blocks_delta(thinking_blocks, token_texts);
        }
        if let Some(audio) = delta.get("audio") {
            self.handle_audio_delta(audio);
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                self.handle_tool_call_delta(tool_call);
            }
        }
        if let Some(function_call) = delta.get("function_call") {
            self.handle_legacy_function_call_delta(function_call);
        }
    }

    fn handle_text_delta(&mut self, delta: &str, token_texts: &mut Vec<String>) {
        self.ensure_message_output();
        self.ensure_message_text_part();
        let (item_id, output_index) = {
            let message = self.message.as_mut().expect("message output must exist");
            message.text.push_str(delta);
            (message.id.clone(), message.output_index)
        };
        token_texts.push(delta.to_string());

        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_text.delta",
            "item_id": item_id.as_str(),
            "output_index": output_index,
            "content_index": 0,
            "delta": delta,
            "sequence_number": sequence_number
        })));
    }

    fn handle_reasoning_delta(&mut self, delta: &str, token_texts: &mut Vec<String>) {
        let (item_id, output_index) = {
            let reasoning = self.ensure_reasoning_output();
            reasoning.text.push_str(delta);
            (reasoning.id.clone(), reasoning.output_index)
        };
        token_texts.push(delta.to_string());

        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": item_id,
            "output_index": output_index,
            "delta": delta,
            "sequence_number": sequence_number
        })));
    }

    fn handle_thinking_blocks_delta(
        &mut self,
        thinking_blocks: &[Value],
        token_texts: &mut Vec<String>,
    ) {
        for block in thinking_blocks {
            let Some(block) = block.as_object() else {
                continue;
            };
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    if let Some(text) = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        self.handle_reasoning_delta(text, token_texts);
                    }
                }
                Some("redacted_thinking") => {
                    if let Some(data) = block
                        .get("data")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        let reasoning = self.ensure_reasoning_output();
                        reasoning.encrypted_content = Some(data.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_audio_delta(&mut self, delta: &Value) {
        self.ensure_message_output();
        let message = self.message.as_mut().expect("message output must exist");
        match &mut message.audio {
            Some(existing) => merge_chat_audio_delta(existing, delta),
            None => {
                message.audio = Some(delta.clone());
            }
        }
    }

    fn handle_tool_call_delta(&mut self, tool_call: &Value) {
        let Some(tool_call) = tool_call.as_object() else {
            return;
        };
        let call_index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;

        let call_id = tool_call.get("id").and_then(Value::as_str);
        let function = tool_call.get("function").and_then(Value::as_object);
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str);
        let arguments_delta = function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str);

        if let Some(arguments_delta) = arguments_delta {
            let (item_id, output_index) = {
                let state = self.ensure_function_call_output(call_index, call_id, name);
                state.arguments.push_str(arguments_delta);
                (state.id.clone(), state.output_index)
            };
            self.push_function_call_arguments_delta(&item_id, output_index, arguments_delta);
        } else {
            self.ensure_function_call_output(call_index, call_id, name);
        }
    }

    fn handle_legacy_function_call_delta(&mut self, function_call: &Value) {
        let Some(function_call) = function_call.as_object() else {
            return;
        };
        let name = function_call.get("name").and_then(Value::as_str);
        let arguments_delta = function_call.get("arguments").and_then(Value::as_str);

        if let Some(arguments_delta) = arguments_delta {
            let (item_id, output_index) = {
                let state = self.ensure_function_call_output(0, None, name);
                state.arguments.push_str(arguments_delta);
                (state.id.clone(), state.output_index)
            };
            self.push_function_call_arguments_delta(&item_id, output_index, arguments_delta);
        } else {
            self.ensure_function_call_output(0, None, name);
        }
    }

    fn ensure_reasoning_output(&mut self) -> &mut ReasoningOutput {
        if self.reasoning.is_none() {
            let output_index = self.next_output_index;
            self.next_output_index += 1;
            let reasoning_id = format!("rs_{}", self.id_seed);
            self.push_reasoning_item_added(&reasoning_id, output_index);
            self.reasoning = Some(ReasoningOutput {
                id: reasoning_id,
                output_index,
                text: String::new(),
                encrypted_content: None,
            });
        }
        self.reasoning
            .as_mut()
            .expect("reasoning output must exist")
    }

    fn ensure_message_output(&mut self) {
        if self.message.is_none() {
            let output_index = self.next_output_index;
            self.next_output_index += 1;
            let message_id = format!("msg_{}", self.id_seed);
            self.push_message_item_added(&message_id, output_index);
            self.message = Some(MessageOutput {
                id: message_id,
                output_index,
                text: String::new(),
                text_part_started: false,
                audio: None,
            });
        }
    }

    fn ensure_message_text_part(&mut self) {
        self.ensure_message_output();
        let (item_id, output_index, should_emit) = {
            let message = self.message.as_mut().expect("message output must exist");
            let should_emit = !message.text_part_started;
            if should_emit {
                message.text_part_started = true;
            }
            (message.id.clone(), message.output_index, should_emit)
        };
        if should_emit {
            self.push_message_content_part_added(&item_id, output_index);
        }
    }

    fn ensure_function_call_output(
        &mut self,
        call_index: usize,
        call_id: Option<&str>,
        name: Option<&str>,
    ) -> &mut FunctionCallOutput {
        if self.function_calls.len() <= call_index {
            self.function_calls.resize_with(call_index + 1, || None);
        }

        if self.function_calls[call_index].is_none() {
            let output_index = self.next_output_index;
            self.next_output_index += 1;
            let item_id = format!("fc_{}_{}", self.id_seed, call_index);
            let call_id = call_id
                .map(|value| value.to_string())
                .unwrap_or_else(|| format!("call_{}_{}", self.id_seed, call_index));
            let name = name.unwrap_or("").to_string();

            self.push_function_call_item_added(&item_id, output_index, &call_id, &name);
            self.function_calls[call_index] = Some(FunctionCallOutput {
                id: item_id,
                output_index,
                call_id,
                name,
                arguments: String::new(),
            });
        } else {
            let state = self.function_calls[call_index]
                .as_mut()
                .expect("call output must exist");
            if let Some(call_id) = call_id {
                if state.call_id.is_empty() {
                    state.call_id = call_id.to_string();
                }
            }
            if let Some(name) = name {
                if state.name.is_empty() {
                    state.name = name.to_string();
                }
            }
        }

        self.function_calls[call_index]
            .as_mut()
            .expect("call output must exist")
    }

    fn push_response_created(&mut self) {
        let response = self.build_response_object("in_progress", Vec::new(), None, None);
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.created",
            "response": response,
            "sequence_number": sequence_number
        })));
    }

    fn push_message_item_added(&mut self, item_id: &str, output_index: u64) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "message",
                "status": "in_progress",
                "role": "assistant",
                "content": []
            },
            "sequence_number": sequence_number
        })));
    }

    fn push_reasoning_item_added(&mut self, item_id: &str, output_index: u64) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "reasoning",
                "status": "in_progress",
                "summary": []
            },
            "sequence_number": sequence_number
        })));
    }

    fn push_message_content_part_added(&mut self, item_id: &str, output_index: u64) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.content_part.added",
            "item_id": item_id,
            "output_index": output_index,
            "content_index": 0,
            "part": {
                "type": "output_text",
                "text": "",
                "annotations": []
            },
            "sequence_number": sequence_number
        })));
    }

    fn push_function_call_item_added(
        &mut self,
        item_id: &str,
        output_index: u64,
        call_id: &str,
        name: &str,
    ) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "function_call",
                "status": "in_progress",
                "arguments": "",
                "call_id": call_id,
                "name": name
            },
            "sequence_number": sequence_number
        })));
    }

    fn push_function_call_arguments_delta(
        &mut self,
        item_id: &str,
        output_index: u64,
        delta: &str,
    ) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.function_call_arguments.delta",
            "item_id": item_id,
            "output_index": output_index,
            "delta": delta,
            "sequence_number": sequence_number
        })));
    }

    fn push_done(&mut self) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;

        let completed_at = (super::now_ms() / 1000) as i64;
        let usage_snapshot = self.collector.finish();
        // Prefer upstream `usage` JSON to preserve breakdown fields (e.g. reasoning tokens).
        // Fallback to the normalized TokenUsage counters when upstream did not provide usage.
        let usage = usage_snapshot
            .usage_json
            .as_ref()
            .and_then(super::super::openai_compat::map_usage_chat_to_responses)
            .or_else(|| usage_snapshot.usage.map(usage_to_value));

        let mut snapshots = Vec::new();
        if let Some(reasoning) = &self.reasoning {
            snapshots.push(OutputItemSnapshot::Reasoning {
                id: reasoning.id.clone(),
                output_index: reasoning.output_index,
                text: reasoning.text.clone(),
                encrypted_content: reasoning.encrypted_content.clone(),
            });
        }
        if let Some(message) = &self.message {
            snapshots.push(OutputItemSnapshot::Message {
                id: message.id.clone(),
                output_index: message.output_index,
                text: message.text.clone(),
                audio: message.audio.clone(),
            });
        }
        for call in &self.function_calls {
            let Some(call) = call else {
                continue;
            };
            snapshots.push(OutputItemSnapshot::FunctionCall {
                id: call.id.clone(),
                output_index: call.output_index,
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }
        snapshots.sort_by_key(|item| match item {
            OutputItemSnapshot::Reasoning { output_index, .. } => *output_index,
            OutputItemSnapshot::Message { output_index, .. } => *output_index,
            OutputItemSnapshot::FunctionCall { output_index, .. } => *output_index,
        });

        let output = snapshots
            .iter()
            .map(snapshot_to_output_item)
            .collect::<Vec<_>>();
        for snapshot in &snapshots {
            self.push_item_done_events(snapshot);
        }

        let response = self.build_response_object("completed", output, usage, Some(completed_at));
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.completed",
            "response": response,
            "sequence_number": sequence_number
        })));
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
    }

    fn push_item_done_events(&mut self, snapshot: &OutputItemSnapshot) {
        match snapshot {
            OutputItemSnapshot::Reasoning {
                id,
                output_index,
                text,
                encrypted_content,
            } => self.push_reasoning_done_events(
                id,
                *output_index,
                text,
                encrypted_content.as_deref(),
            ),
            OutputItemSnapshot::Message {
                id,
                output_index,
                text,
                audio,
            } => self.push_message_done_events(id, *output_index, text, audio.as_ref()),
            OutputItemSnapshot::FunctionCall {
                id,
                output_index,
                call_id,
                name,
                arguments,
            } => self.push_function_call_done_events(id, *output_index, call_id, name, arguments),
        }
    }

    fn push_reasoning_done_events(
        &mut self,
        item_id: &str,
        output_index: u64,
        text: &str,
        encrypted_content: Option<&str>,
    ) {
        let mut item = json!({
            "id": item_id,
            "type": "reasoning",
            "status": "completed",
            "summary": [
                {
                    "type": "summary_text",
                    "text": text
                }
            ]
        });
        if let Some(item) = item.as_object_mut() {
            if let Some(encrypted_content) = encrypted_content {
                item.insert(
                    "encrypted_content".to_string(),
                    Value::String(encrypted_content.to_string()),
                );
            }
        }
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": item,
            "sequence_number": sequence_number
        })));
    }

    fn push_message_done_events(
        &mut self,
        item_id: &str,
        output_index: u64,
        text: &str,
        audio: Option<&Value>,
    ) {
        if !text.is_empty() {
            let sequence_number = self.next_sequence_number();
            self.out.push_back(super::responses_event_sse(json!({
                "type": "response.output_text.done",
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "text": text,
                "sequence_number": sequence_number
            })));

            let sequence_number = self.next_sequence_number();
            self.out.push_back(super::responses_event_sse(json!({
                "type": "response.content_part.done",
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": {
                    "type": "output_text",
                    "text": text,
                    "annotations": []
                },
                "sequence_number": sequence_number
            })));
        }

        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(json!({
                "type": "output_text",
                "text": text,
                "annotations": []
            }));
        }
        if let Some(audio) = audio {
            content.push(json!({
                "type": "output_audio",
                "audio": audio
            }));
        }

        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": content
            },
            "sequence_number": sequence_number
        })));
    }

    fn push_function_call_done_events(
        &mut self,
        item_id: &str,
        output_index: u64,
        call_id: &str,
        name: &str,
        arguments: &str,
    ) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.function_call_arguments.done",
            "item_id": item_id,
            "output_index": output_index,
            "name": name,
            "arguments": arguments,
            "sequence_number": sequence_number
        })));

        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": name,
                "arguments": arguments
            },
            "sequence_number": sequence_number
        })));
    }

    fn build_response_object(
        &self,
        status: &str,
        output: Vec<Value>,
        usage: Option<Value>,
        completed_at: Option<i64>,
    ) -> Value {
        json!({
            "id": self.response_id.as_str(),
            "object": "response",
            "created_at": self.created_at,
            "model": self.model.as_str(),
            "status": status,
            "output": output,
            "parallel_tool_calls": self.parallel_tool_calls(),
            "completed_at": completed_at,
            "usage": usage,
            "error": null,
            "metadata": {}
        })
    }

    fn parallel_tool_calls(&self) -> bool {
        self.function_calls
            .iter()
            .filter(|call| call.is_some())
            .count()
            > 1
    }

    fn log_usage_once(&mut self) {
        self.write_log_once(None);
    }

    fn next_sequence_number(&mut self) -> u64 {
        let current = self.sequence;
        self.sequence += 1;
        current
    }
}

fn merge_chat_audio_delta(existing: &mut Value, delta: &Value) {
    if !existing.is_object() || !delta.is_object() {
        *existing = delta.clone();
        return;
    }
    let existing_object = existing.as_object_mut().expect("audio delta object");
    let delta_object = delta.as_object().expect("audio delta object");

    for (key, value) in delta_object {
        match key.as_str() {
            // Streaming chat chunks may split both base64 audio and transcript text.
            "data" | "transcript" => {
                let merged = existing_object
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
                    + value.as_str().unwrap_or("");
                existing_object.insert(key.clone(), Value::String(merged));
            }
            _ => {
                existing_object.insert(key.clone(), value.clone());
            }
        }
    }
}
