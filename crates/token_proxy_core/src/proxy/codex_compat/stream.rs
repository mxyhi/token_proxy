use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::extract_tool_name_map_from_request_body;
use super::super::log::{build_log_entry, LogContext, LogWriter};
use super::super::response::STREAM_DROPPED_ERROR;
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;

pub(crate) fn stream_codex_to_chat<E>(
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
    let state = CodexToChatState::new(upstream, context, log, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

struct CodexToChatState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    response_id: String,
    created: i64,
    model: String,
    function_call_index: i64,
    finish_reason: Option<&'static str>,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
    tool_name_map: HashMap<String, String>,
}

impl<S> CodexToChatState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        self.logged = true;
        let entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        self.log.clone().write_detached(entry);
    }
}

impl<S> Drop for CodexToChatState<S> {
    fn drop(&mut self) {
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S, E> CodexToChatState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(
        upstream: S,
        mut context: LogContext,
        log: Arc<LogWriter>,
        token_tracker: RequestTokenTracker,
    ) -> Self {
        let now_ms = now_unix_seconds();
        let response_id = format!("chatcmpl_proxy_{now_ms}");
        let model = context
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let tool_name_map = extract_tool_name_map_from_request_body(context.request_body.as_deref());
        context.request_body = None;

        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            token_tracker,
            context,
            out: VecDeque::new(),
            response_id,
            created: now_ms,
            model,
            function_call_index: -1,
            finish_reason: None,
            sent_done: false,
            logged: false,
            upstream_ended: false,
            tool_name_map,
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

        match event_type {
            "response.created" => {
                self.update_from_created(&value);
            }
            "response.output_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    token_texts.push(delta.to_string());
                    self.push_chunk(json!({ "role": "assistant", "content": delta }));
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    token_texts.push(delta.to_string());
                    self.push_chunk(json!({ "role": "assistant", "reasoning_content": delta }));
                }
            }
            "response.reasoning_summary_text.done" => {
                self.push_chunk(json!({ "role": "assistant", "reasoning_content": "\n\n" }));
            }
            "response.output_item.done" => {
                self.handle_function_call_item(&value);
            }
            "response.completed" => {
                self.finish_reason = Some(self.resolve_finish_reason());
            }
            _ => {}
        }
    }

    fn update_from_created(&mut self, value: &Value) {
        if let Some(response) = value.get("response").and_then(Value::as_object) {
            if let Some(id) = response.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    self.response_id = id.to_string();
                }
            }
            if let Some(created) = response.get("created_at").and_then(Value::as_i64) {
                self.created = created;
            }
            if let Some(model) = response.get("model").and_then(Value::as_str) {
                if !model.is_empty() {
                    self.model = model.to_string();
                }
            }
        }
    }

    fn handle_function_call_item(&mut self, value: &Value) {
        let Some(item) = value.get("item").and_then(Value::as_object) else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
        let restored = self
            .tool_name_map
            .get(name)
            .map(String::as_str)
            .unwrap_or(name);
        let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
        let id = item
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("id").and_then(Value::as_str))
            .unwrap_or("call_proxy");
        self.function_call_index += 1;
        let tool_call = json!({
            "index": self.function_call_index,
            "id": id,
            "type": "function",
            "function": { "name": restored, "arguments": arguments }
        });
        self.push_chunk(json!({ "role": "assistant", "tool_calls": [tool_call] }));
    }

    fn push_chunk(&mut self, delta: Value) {
        let chunk = chat_chunk_sse(&self.response_id, self.created, &self.model, delta, None);
        self.out.push_back(chunk);
    }

    fn push_done(&mut self) {
        if self.sent_done {
            return;
        }
        let finish = self.finish_reason.unwrap_or_else(|| self.resolve_finish_reason());
        let done = chat_chunk_sse(
            &self.response_id,
            self.created,
            &self.model,
            json!({}),
            Some(finish),
        );
        self.out.push_back(done);
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
        self.sent_done = true;
    }

    fn resolve_finish_reason(&self) -> &'static str {
        if self.function_call_index >= 0 {
            "tool_calls"
        } else {
            "stop"
        }
    }

    fn log_usage_once(&mut self) {
        self.write_log_once(None);
    }
}

pub(crate) fn stream_codex_to_responses<E>(
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
    let state = CodexToResponsesState::new(upstream, context, log, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

struct CodexToResponsesState<S> {
    upstream: S,
    parser: SseEventParser,
    collector: SseUsageCollector,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
    tool_name_map: HashMap<String, String>,
}

impl<S> CodexToResponsesState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        self.logged = true;
        let entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        self.log.clone().write_detached(entry);
    }
}

impl<S> Drop for CodexToResponsesState<S> {
    fn drop(&mut self) {
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S, E> CodexToResponsesState<S>
where
    S: futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    fn new(
        upstream: S,
        mut context: LogContext,
        log: Arc<LogWriter>,
        token_tracker: RequestTokenTracker,
    ) -> Self {
        let tool_name_map = extract_tool_name_map_from_request_body(context.request_body.as_deref());
        context.request_body = None;
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            token_tracker,
            context,
            out: VecDeque::new(),
            sent_done: false,
            logged: false,
            upstream_ended: false,
            tool_name_map,
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
                        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
                        self.sent_done = true;
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
            self.out.push_back(Bytes::from("data: [DONE]\n\n"));
            self.sent_done = true;
            return;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        restore_tool_names_in_event(&mut value, &self.tool_name_map);
        if let Some(delta) = extract_output_text_delta(&value) {
            token_texts.push(delta.to_string());
        }
        self.out
            .push_back(Bytes::from(format!("data: {}\n\n", value.to_string())));
    }

    fn log_usage_once(&mut self) {
        self.write_log_once(None);
    }
}

fn restore_tool_names_in_event(value: &mut Value, tool_name_map: &HashMap<String, String>) {
    if tool_name_map.is_empty() {
        return;
    }
    if let Some(item) = value.get_mut("item").and_then(Value::as_object_mut) {
        restore_tool_names_in_item(item, tool_name_map);
    }
    if let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) {
        restore_tool_names_in_response(response, tool_name_map);
    }
    if let Some(response) = value.as_object_mut() {
        restore_tool_names_in_response(response, tool_name_map);
    }
}

fn restore_tool_names_in_response(
    response: &mut Map<String, Value>,
    tool_name_map: &HashMap<String, String>,
) {
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return;
    };
    for item in output {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        restore_tool_names_in_item(item, tool_name_map);
    }
}

fn restore_tool_names_in_item(item: &mut Map<String, Value>, tool_name_map: &HashMap<String, String>) {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return;
    }
    let Some(name) = item.get("name").and_then(Value::as_str) else {
        return;
    };
    let Some(restored) = tool_name_map.get(name) else {
        return;
    };
    item.insert("name".to_string(), Value::String(restored.clone()));
}

fn extract_output_text_delta(value: &Value) -> Option<&str> {
    if value.get("type").and_then(Value::as_str) != Some("response.output_text.delta") {
        return None;
    }
    value.get("delta").and_then(Value::as_str)
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

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
