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

pub(super) fn stream_responses_to_chat<E>(
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
    let state = ResponsesToChatState::new(upstream, context, log, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

struct ResponsesToChatState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    chat_id: String,
    created: i64,
    model: String,
    sent_role: bool,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
    tool_calls: Vec<ToolCallState>,
    tool_calls_by_item_id: HashMap<String, usize>,
    // 非文本输出只透传一次，避免重复注入 content_parts。
    content_parts_sent: bool,
}

struct ToolCallState {
    index: usize,
    call_id: String,
    name: String,
    arguments: String,
    sent_initial: bool,
    sent_arguments: bool,
}

impl<S, E> ResponsesToChatState<S>
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
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            token_tracker,
            model: context
                .model
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            context,
            out: VecDeque::new(),
            chat_id: format!("chatcmpl_proxy_{now_ms}"),
            created: (now_ms / 1000) as i64,
            sent_role: false,
            sent_done: false,
            logged: false,
            upstream_ended: false,
            tool_calls: Vec::new(),
            tool_calls_by_item_id: HashMap::new(),
            content_parts_sent: false,
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
        let Some(event_type) = value.get("type").and_then(Value::as_str) else {
            return;
        };
        if event_type.ends_with("output_text.delta") {
            self.handle_output_text_delta(&value, token_texts);
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
        if event_type.ends_with("output_item.added") {
            self.handle_output_item_added(&value);
            return;
        }
        if event_type.ends_with("output_item.done") {
            self.handle_output_item_done(&value);
            return;
        }
        if event_type.ends_with("response.completed") {
            self.handle_response_completed(&value);
        }
    }

    fn handle_output_text_delta(&mut self, value: &Value, token_texts: &mut Vec<String>) {
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        token_texts.push(delta.to_string());
        self.ensure_role_sent();
        self.out.push_back(chat_chunk_sse(
            &self.chat_id,
            self.created,
            &self.model,
            json!({ "content": delta }),
            None,
        ));
    }

    fn handle_output_item_added(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return;
        };
        if item_type == "function_call" {
            self.handle_function_call_item_added(item);
        }
    }

    fn handle_function_call_item_added(&mut self, item: &Map<String, Value>) {
        let Some(item_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let call_id = item.get("call_id").and_then(Value::as_str);
        let name = item.get("name").and_then(Value::as_str);

        let (index, call_id, name, should_emit) = {
            let state = self.ensure_tool_call_state(item_id, call_id, name);
            let should_emit = !state.sent_initial;
            state.sent_initial = true;
            (
                state.index,
                state.call_id.clone(),
                state.name.clone(),
                should_emit,
            )
        };
        if should_emit {
            let id = tool_call_id(&call_id, item_id);
            self.push_tool_call_delta(index, &id, &name, "");
        }
    }

    fn handle_function_call_arguments_delta(&mut self, value: &Value) {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        let (index, call_id, name) = {
            let state = self.ensure_tool_call_state(item_id, None, None);
            state.arguments.push_str(delta);
            state.sent_initial = true;
            state.sent_arguments = true;
            (state.index, state.call_id.clone(), state.name.clone())
        };
        let id = tool_call_id(&call_id, item_id);
        self.push_tool_call_delta(index, &id, &name, delta);
    }

    fn handle_function_call_arguments_done(&mut self, value: &Value) {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let arguments = value.get("arguments").and_then(Value::as_str).unwrap_or("");
        let name = value.get("name").and_then(Value::as_str);

        let (index, call_id, name, should_emit) = {
            let state = self.ensure_tool_call_state(item_id, None, name);
            if !arguments.is_empty() {
                state.arguments = arguments.to_string();
            }
            let should_emit = !arguments.is_empty() && !state.sent_arguments;
            state.sent_initial = true;
            if should_emit {
                state.sent_arguments = true;
            }
            (state.index, state.call_id.clone(), state.name.clone(), should_emit)
        };
        if should_emit {
            let id = tool_call_id(&call_id, item_id);
            self.push_tool_call_delta(index, &id, &name, arguments);
        }
    }

    fn handle_output_item_done(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return;
        };
        match item_type {
            "function_call" => self.handle_function_call_item_snapshot(item),
            "message" => self.handle_message_item_snapshot(item),
            _ => {}
        }
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
            match item.get("type").and_then(Value::as_str) {
                Some("function_call") => self.handle_function_call_item_snapshot(item),
                Some("message") => self.handle_message_item_snapshot(item),
                _ => {}
            }
        }
    }

    fn handle_function_call_item_snapshot(&mut self, item: &Map<String, Value>) {
        let Some(item_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let call_id = item.get("call_id").and_then(Value::as_str);
        let name = item.get("name").and_then(Value::as_str);
        let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");

        let (index, call_id, name, should_emit) = {
            let state = self.ensure_tool_call_state(item_id, call_id, name);
            if !arguments.is_empty() {
                state.arguments = arguments.to_string();
            }
            let should_emit = !arguments.is_empty() && !state.sent_arguments;
            state.sent_initial = true;
            if should_emit {
                state.sent_arguments = true;
            }
            (state.index, state.call_id.clone(), state.name.clone(), should_emit)
        };
        if should_emit {
            let id = tool_call_id(&call_id, item_id);
            self.push_tool_call_delta(index, &id, &name, arguments);
        }
    }

    fn handle_message_item_snapshot(&mut self, item: &Map<String, Value>) {
        if item.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            return;
        };
        self.maybe_emit_content_parts(content);
    }

    fn ensure_tool_call_state(
        &mut self,
        item_id: &str,
        call_id: Option<&str>,
        name: Option<&str>,
    ) -> &mut ToolCallState {
        let index = if let Some(index) = self.tool_calls_by_item_id.get(item_id) {
            *index
        } else {
            let index = self.tool_calls.len();
            self.tool_calls_by_item_id
                .insert(item_id.to_string(), index);
            self.tool_calls.push(ToolCallState {
                index,
                call_id: String::new(),
                name: String::new(),
                arguments: String::new(),
                sent_initial: false,
                sent_arguments: false,
            });
            index
        };

        let state = self.tool_calls.get_mut(index).expect("tool call state");
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
        state
    }

    fn maybe_emit_content_parts(&mut self, parts: &[Value]) {
        if self.content_parts_sent {
            return;
        }
        // Chat 标准没有 Responses 的非文本输出，这里用扩展字段保留原始内容。
        let has_non_text = parts.iter().any(|part| {
            part.get("type")
                .and_then(Value::as_str)
                .is_some_and(|part_type| part_type != "output_text")
        });
        if !has_non_text {
            return;
        }
        self.ensure_role_sent();
        self.out.push_back(chat_chunk_sse(
            &self.chat_id,
            self.created,
            &self.model,
            json!({ "content_parts": parts }),
            None,
        ));
        self.content_parts_sent = true;
    }

    fn ensure_role_sent(&mut self) {
        if self.sent_role {
            return;
        }
        self.sent_role = true;
        self.out.push_back(chat_chunk_sse(
            &self.chat_id,
            self.created,
            &self.model,
            json!({ "role": "assistant", "content": "" }),
            None,
        ));
    }

    fn push_tool_call_delta(&mut self, index: usize, id: &str, name: &str, arguments: &str) {
        self.ensure_role_sent();
        let mut function = Map::new();
        if !name.is_empty() {
            function.insert("name".to_string(), Value::String(name.to_string()));
        }
        function.insert(
            "arguments".to_string(),
            Value::String(arguments.to_string()),
        );
        let tool_call = json!({
            "index": index,
            "id": id,
            "type": "function",
            "function": Value::Object(function)
        });
        self.out.push_back(chat_chunk_sse(
            &self.chat_id,
            self.created,
            &self.model,
            json!({ "tool_calls": [tool_call] }),
            None,
        ));
    }

    fn push_done(&mut self) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;
        self.out.push_back(chat_chunk_sse(
            &self.chat_id,
            self.created,
            &self.model,
            json!({}),
            Some(self.finish_reason()),
        ));
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
    }

    fn log_usage_once(&mut self) {
        if self.logged {
            return;
        }
        self.logged = true;
        let entry = build_log_entry(&self.context, self.collector.finish(), None);
        self.log.clone().write_detached(entry);
    }

    fn finish_reason(&self) -> &'static str {
        if self.tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        }
    }
}

fn chat_chunk_sse(
    id: &str,
    created: i64,
    model: &str,
    delta: Value,
    finish_reason: Option<&str>,
) -> Bytes {
    let chunk = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [
            {
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason
            }
        ]
    });
    Bytes::from(format!("data: {}\n\n", chunk.to_string()))
}

fn tool_call_id(call_id: &str, item_id: &str) -> String {
    if !call_id.is_empty() {
        call_id.to_string()
    } else if !item_id.is_empty() {
        item_id.to_string()
    } else {
        "call_proxy".to_string()
    }
}
