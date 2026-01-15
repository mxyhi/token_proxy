use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{collections::HashMap, collections::VecDeque, sync::Arc};

use super::super::log::{build_log_entry, LogContext, LogWriter};
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;
use format::{snapshot_to_output_item, usage_to_value, OutputItemSnapshot};

mod format;

pub(super) fn stream_anthropic_to_responses(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, reqwest::Error>>
        + Unpin
        + Send
        + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let state = AnthropicToResponsesState::new(upstream, context, log, token_tracker);
    futures_util::stream::try_unfold(state, |state| async move { state.step().await })
}

struct MessageOutput {
    id: String,
    output_index: u64,
    text: String,
}

struct FunctionCallOutput {
    id: String,
    output_index: u64,
    call_id: String,
    name: String,
    arguments: String,
}

struct AnthropicToResponsesState<S> {
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
    message: Option<MessageOutput>,
    function_calls: Vec<Option<FunctionCallOutput>>,
    // Claude stream uses block index; map it to our function_call slot.
    tool_call_by_block_index: HashMap<usize, usize>,
    sequence: u64,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
}

impl<S> AnthropicToResponsesState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin + Send + 'static,
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
            message: None,
            function_calls: Vec::new(),
            tool_call_by_block_index: HashMap::new(),
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
        // Claude stream may include event: lines; parser only yields data: payload.
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let Some(event_type) = value.get("type").and_then(Value::as_str) else {
            return;
        };

        match event_type {
            "message_start" => {
                // Preserve the original requested model alias if present (consistent with other
                // format conversions); only fall back to upstream model when we have no hint.
                if self.model == "unknown" {
                    if let Some(model) = value
                        .get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(Value::as_str)
                    {
                        if !model.is_empty() {
                            self.model = model.to_string();
                        }
                    }
                }
            }
            "content_block_start" => self.handle_content_block_start(&value),
            "content_block_delta" => self.handle_content_block_delta(&value, token_texts),
            "message_stop" => {
                self.push_done();
            }
            _ => {}
        }
    }

    fn handle_content_block_start(&mut self, value: &Value) {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(block) = value.get("content_block").and_then(Value::as_object) else {
            return;
        };
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                self.ensure_message_output();
            }
            "tool_use" => {
                let call_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let tool_index = self.ensure_function_call_output(index, Some(call_id), Some(name));
                self.tool_call_by_block_index.insert(index, tool_index);
            }
            _ => {}
        }
    }

    fn handle_content_block_delta(&mut self, value: &Value, token_texts: &mut Vec<String>) {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(delta) = value.get("delta").and_then(Value::as_object) else {
            return;
        };
        let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
        match delta_type {
            "text_delta" => {
                let Some(text) = delta.get("text").and_then(Value::as_str) else {
                    return;
                };
                self.ensure_message_output();
                let (item_id, output_index) = {
                    let message = self.message.as_mut().expect("message output exists");
                    message.text.push_str(text);
                    (message.id.clone(), message.output_index)
                };
                token_texts.push(text.to_string());
                let sequence_number = self.next_sequence_number();
                self.out.push_back(super::responses_event_sse(json!({
                    "type": "response.output_text.delta",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": text,
                    "sequence_number": sequence_number
                })));
            }
            "input_json_delta" => {
                let Some(partial_json) = delta.get("partial_json").and_then(Value::as_str) else {
                    return;
                };
                let call_index = match self.tool_call_by_block_index.get(&index) {
                    Some(idx) => *idx,
                    None => {
                        let tool_index = self.ensure_function_call_output(index, None, None);
                        self.tool_call_by_block_index.insert(index, tool_index);
                        tool_index
                    }
                };
                let (item_id, output_index) = {
                    let state = self
                        .function_calls
                        .get_mut(call_index)
                        .and_then(Option::as_mut)
                        .expect("call output exists");
                    state.arguments.push_str(partial_json);
                    (state.id.clone(), state.output_index)
                };
                let sequence_number = self.next_sequence_number();
                self.out.push_back(super::responses_event_sse(json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": item_id,
                    "output_index": output_index,
                    "delta": partial_json,
                    "sequence_number": sequence_number
                })));
            }
            _ => {}
        }
    }

    fn ensure_message_output(&mut self) {
        if self.message.is_some() {
            return;
        }
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        let message_id = format!("msg_{}", self.id_seed);
        self.push_message_item_added(&message_id, output_index);
        self.push_message_content_part_added(&message_id, output_index);
        self.message = Some(MessageOutput {
            id: message_id,
            output_index,
            text: String::new(),
        });
    }

    fn ensure_function_call_output(&mut self, block_index: usize, call_id: Option<&str>, name: Option<&str>) -> usize {
        // Allocate one function_call per Claude content block index.
        let call_index = self.function_calls.len();
        let output_index = self.next_output_index;
        self.next_output_index += 1;

        let item_id = format!("fc_{}_{}", self.id_seed, block_index);
        let call_id = call_id
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("call_{}_{}", self.id_seed, block_index));
        let name = name.unwrap_or("").to_string();

        self.push_function_call_item_added(&item_id, output_index, &call_id, &name);
        self.function_calls.push(Some(FunctionCallOutput {
            id: item_id,
            output_index,
            call_id,
            name,
            arguments: String::new(),
        }));
        call_index
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

    fn push_done(&mut self) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;

        let completed_at = (super::now_ms() / 1000) as i64;
        let usage_snapshot = self.collector.finish();
        let usage = usage_snapshot
            .usage
            .clone()
            .map(|usage| usage_to_value(usage, usage_snapshot.cached_tokens));

        let mut snapshots = Vec::new();
        if let Some(message) = &self.message {
            snapshots.push(OutputItemSnapshot::Message {
                id: message.id.clone(),
                output_index: message.output_index,
                text: message.text.clone(),
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
            OutputItemSnapshot::Message {
                id,
                output_index,
                text,
            } => self.push_message_done_events(id, *output_index, text),
            OutputItemSnapshot::FunctionCall {
                id,
                output_index,
                call_id,
                name,
                arguments,
            } => self.push_function_call_done_events(id, *output_index, call_id, name, arguments),
        }
    }

    fn push_message_done_events(&mut self, item_id: &str, output_index: u64, text: &str) {
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

        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": text, "annotations": [] }
                ]
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
            "arguments": arguments,
            "sequence_number": sequence_number,
            "name": name
        })));

        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "function_call",
                "status": "completed",
                "arguments": arguments,
                "call_id": call_id,
                "name": name
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
        self.function_calls.iter().filter(|call| call.is_some()).count() > 1
    }

    fn log_usage_once(&mut self) {
        if self.logged {
            return;
        }
        self.logged = true;
        let entry = build_log_entry(&self.context, self.collector.finish(), None);
        self.log.clone().write_detached(entry);
    }

    fn next_sequence_number(&mut self) -> u64 {
        let current = self.sequence;
        self.sequence += 1;
        current
    }
}
