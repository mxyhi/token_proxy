use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{collections::HashMap, collections::VecDeque, sync::Arc};

use super::super::log::TokenUsage;
use super::super::log::{attach_response_body, build_log_entry, LogContext, LogWriter};
use super::super::sse::SseEventParser;
use super::super::token_rate::RequestTokenTracker;
use super::super::usage::SseUsageCollector;
use super::streaming::STREAM_DROPPED_ERROR;
use crate::proxy::anthropic_compat::web_search;
use crate::proxy::{claude_reasoning, compat_reason};
use format::{snapshot_to_output_item, usage_to_value, AnthropicCacheUsage, OutputItemSnapshot};

mod format;

pub(super) fn stream_anthropic_to_responses<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = AnthropicToResponsesState::new(upstream, context, log, token_tracker);
    futures_util::stream::try_unfold(state, |state| async move { state.step().await })
}

struct MessageOutput {
    id: String,
    output_index: u64,
    text: String,
}

struct ReasoningOutput {
    id: String,
    output_index: u64,
    text: String,
    encrypted_content: Option<String>,
}

struct FunctionCallOutput {
    id: String,
    output_index: u64,
    call_id: String,
    name: String,
    arguments: String,
}

struct WebSearchOutput {
    id: String,
    output_index: u64,
    query: String,
    input_json: String,
    results: Option<Value>,
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
    messages: Vec<Option<MessageOutput>>,
    message_by_block_index: HashMap<usize, usize>,
    reasonings: Vec<Option<ReasoningOutput>>,
    // Claude stream uses block index; map it to our reasoning slot.
    reasoning_by_block_index: HashMap<usize, usize>,
    function_calls: Vec<Option<FunctionCallOutput>>,
    // Claude stream uses block index; map it to our function_call slot.
    tool_call_by_block_index: HashMap<usize, usize>,
    web_searches: Vec<Option<WebSearchOutput>>,
    web_search_by_block_index: HashMap<usize, usize>,
    web_search_by_tool_use_id: HashMap<String, usize>,
    response_status: Option<&'static str>,
    incomplete_reason: Option<&'static str>,
    sequence: u64,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
    response_body_buf: String,
    input_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
}

impl<S> AnthropicToResponsesState<S> {
    fn write_log_once(&mut self, response_error: Option<String>) {
        if self.logged {
            return;
        }
        self.logged = true;
        let mut entry = build_log_entry(&self.context, self.collector.finish(), response_error);
        attach_response_body(&mut entry, &self.response_body_buf);
        self.log.clone().write_detached(entry);
    }
}

impl<S> Drop for AnthropicToResponsesState<S> {
    fn drop(&mut self) {
        // 客户端提前取消时 try_unfold 状态会直接 drop，这里做日志兜底。
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S, E> AnthropicToResponsesState<S>
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
            messages: Vec::new(),
            message_by_block_index: HashMap::new(),
            reasonings: Vec::new(),
            reasoning_by_block_index: HashMap::new(),
            function_calls: Vec::new(),
            tool_call_by_block_index: HashMap::new(),
            web_searches: Vec::new(),
            web_search_by_block_index: HashMap::new(),
            web_search_by_tool_use_id: HashMap::new(),
            response_status: None,
            incomplete_reason: None,
            sequence: 0,
            sent_done: false,
            logged: false,
            upstream_ended: false,
            response_body_buf: String::new(),
            input_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
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
                self.capture_anthropic_usage(
                    value
                        .get("message")
                        .and_then(|message| message.get("usage")),
                );
            }
            "content_block_start" => self.handle_content_block_start(&value),
            "content_block_delta" => self.handle_content_block_delta(&value, token_texts),
            "content_block_stop" => self.handle_content_block_stop(&value),
            "message_delta" => self.handle_message_delta(&value),
            "message_stop" => {
                self.finish_web_search_queries();
                self.push_done();
            }
            _ => {}
        }
    }

    fn handle_message_delta(&mut self, value: &Value) {
        self.capture_anthropic_usage(value.get("usage"));
        let stop_reason = value
            .get("delta")
            .and_then(Value::as_object)
            .and_then(|delta| delta.get("stop_reason"))
            .and_then(Value::as_str);
        let (status, incomplete_reason) =
            compat_reason::responses_status_from_anthropic_stop_reason(stop_reason);
        self.response_status = status;
        self.incomplete_reason = incomplete_reason;
    }

    fn capture_anthropic_usage(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage.and_then(Value::as_object) else {
            return;
        };
        if let Some(tokens) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
            self.cache_read_input_tokens = tokens;
        }
        if let Some(tokens) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.input_tokens = tokens;
        }
        if let Some(tokens) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.cache_creation_input_tokens = tokens;
        }
    }

    fn handle_content_block_start(&mut self, value: &Value) {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(block) = value.get("content_block").and_then(Value::as_object) else {
            return;
        };
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "thinking" => {
                let reasoning_index = self.ensure_reasoning_output(index);
                if let Some(signature) = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    self.set_reasoning_carrier(reasoning_index, signature.to_string());
                }
                self.reasoning_by_block_index.insert(index, reasoning_index);
            }
            "redacted_thinking" => {
                let reasoning_index = self.ensure_reasoning_output(index);
                if let Some(data) = block
                    .get("data")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    if let Some(carrier) = claude_reasoning::redacted_thinking_carrier(data) {
                        self.set_reasoning_carrier(reasoning_index, carrier);
                    }
                }
                self.reasoning_by_block_index.insert(index, reasoning_index);
            }
            "text" => {
                self.ensure_message_output(index);
            }
            "tool_use" => {
                let call_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let tool_index = self.ensure_function_call_output(index, Some(call_id), Some(name));
                self.tool_call_by_block_index.insert(index, tool_index);
            }
            "server_tool_use"
                if block.get("name").and_then(Value::as_str)
                    == Some(web_search::CLAUDE_WEB_SEARCH_NAME) =>
            {
                let tool_use_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                if tool_use_id.is_empty() {
                    tracing::debug!("丢弃缺少 ID 的 Claude 搜索调用块");
                    return;
                }
                let query = web_search::claude_search_query(block.get("input"));
                let search_index = self.ensure_web_search_output(index, tool_use_id, &query);
                self.web_search_by_block_index.insert(index, search_index);
                self.web_search_by_tool_use_id
                    .insert(tool_use_id.to_string(), search_index);
            }
            "web_search_tool_result" => {
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let Some(search_index) = self.web_search_by_tool_use_id.get(tool_use_id).copied()
                else {
                    return;
                };
                if let Some(search) = self
                    .web_searches
                    .get_mut(search_index)
                    .and_then(Option::as_mut)
                {
                    search.results = Some(web_search::responses_search_results_from_claude(
                        block.get("content"),
                    ));
                }
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
            "thinking_delta" => {
                let Some(text) = delta.get("thinking").and_then(Value::as_str) else {
                    return;
                };
                let reasoning_index = match self.reasoning_by_block_index.get(&index) {
                    Some(idx) => *idx,
                    None => {
                        let reasoning_index = self.ensure_reasoning_output(index);
                        self.reasoning_by_block_index.insert(index, reasoning_index);
                        reasoning_index
                    }
                };
                let (item_id, output_index) = {
                    let state = self
                        .reasonings
                        .get_mut(reasoning_index)
                        .and_then(Option::as_mut)
                        .expect("reasoning output exists");
                    state.text.push_str(text);
                    (state.id.clone(), state.output_index)
                };
                token_texts.push(text.to_string());
                let sequence_number = self.next_sequence_number();
                self.out.push_back(super::responses_event_sse(json!({
                    "type": "response.reasoning_summary_text.delta",
                    "item_id": item_id,
                    "output_index": output_index,
                    "delta": text,
                    "sequence_number": sequence_number
                })));
            }
            "signature_delta" => {
                let Some(signature) = delta
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                else {
                    return;
                };
                let reasoning_index = match self.reasoning_by_block_index.get(&index) {
                    Some(idx) => *idx,
                    None => {
                        let reasoning_index = self.ensure_reasoning_output(index);
                        self.reasoning_by_block_index.insert(index, reasoning_index);
                        reasoning_index
                    }
                };
                let reasoning = self
                    .reasonings
                    .get_mut(reasoning_index)
                    .and_then(Option::as_mut)
                    .expect("reasoning output exists");
                if reasoning
                    .encrypted_content
                    .as_deref()
                    .and_then(claude_reasoning::redacted_thinking_data)
                    .is_some()
                {
                    return;
                }
                // Anthropic may split the opaque signature across multiple deltas.
                reasoning
                    .encrypted_content
                    .get_or_insert_with(String::new)
                    .push_str(signature);
            }
            "text_delta" => {
                let Some(text) = delta.get("text").and_then(Value::as_str) else {
                    return;
                };
                let message_index = self.ensure_message_output(index);
                let (item_id, output_index) = {
                    let message = self
                        .messages
                        .get_mut(message_index)
                        .and_then(Option::as_mut)
                        .expect("message output exists");
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
                if let Some(search_index) = self.web_search_by_block_index.get(&index).copied() {
                    if let Some(search) = self
                        .web_searches
                        .get_mut(search_index)
                        .and_then(Option::as_mut)
                    {
                        search.input_json.push_str(partial_json);
                    }
                    return;
                }
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

    fn handle_content_block_stop(&mut self, value: &Value) {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        self.finish_web_search_query(index);
    }

    fn finish_web_search_queries(&mut self) {
        let indexes = self
            .web_search_by_block_index
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for index in indexes {
            self.finish_web_search_query(index);
        }
    }

    fn finish_web_search_query(&mut self, block_index: usize) {
        let Some(search_index) = self.web_search_by_block_index.get(&block_index).copied() else {
            return;
        };
        let Some(search) = self
            .web_searches
            .get_mut(search_index)
            .and_then(Option::as_mut)
        else {
            return;
        };
        if search.input_json.trim().is_empty() {
            return;
        }
        match web_search::claude_search_query_json(&search.input_json) {
            Some(query) if !query.is_empty() => search.query = query,
            Some(_) => {}
            None => tracing::debug!(
                block_index,
                "dropping incomplete Claude web search input JSON while retaining start query"
            ),
        }
    }

    fn ensure_reasoning_output(&mut self, block_index: usize) -> usize {
        let reasoning_index = self.reasonings.len();
        let output_index = self.next_output_index;
        self.next_output_index += 1;

        let item_id = format!("rs_{}_{}", self.id_seed, block_index);
        self.push_reasoning_item_added(&item_id, output_index);
        self.reasonings.push(Some(ReasoningOutput {
            id: item_id,
            output_index,
            text: String::new(),
            encrypted_content: None,
        }));
        reasoning_index
    }

    fn set_reasoning_carrier(&mut self, reasoning_index: usize, carrier: String) {
        if let Some(reasoning) = self
            .reasonings
            .get_mut(reasoning_index)
            .and_then(Option::as_mut)
        {
            reasoning.encrypted_content = Some(carrier);
        }
    }

    fn ensure_message_output(&mut self, block_index: usize) -> usize {
        if let Some(message_index) = self.message_by_block_index.get(&block_index) {
            return *message_index;
        }
        let message_index = self.messages.len();
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        let message_id = format!("msg_{}_{}", self.id_seed, block_index);
        self.push_message_item_added(&message_id, output_index);
        self.push_message_content_part_added(&message_id, output_index);
        self.messages.push(Some(MessageOutput {
            id: message_id,
            output_index,
            text: String::new(),
        }));
        self.message_by_block_index
            .insert(block_index, message_index);
        message_index
    }

    fn ensure_function_call_output(
        &mut self,
        block_index: usize,
        call_id: Option<&str>,
        name: Option<&str>,
    ) -> usize {
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

    fn ensure_web_search_output(
        &mut self,
        block_index: usize,
        tool_use_id: &str,
        query: &str,
    ) -> usize {
        if let Some(search_index) = self.web_search_by_block_index.get(&block_index) {
            return *search_index;
        }
        let search_index = self.web_searches.len();
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        let id = web_search::responses_search_id(tool_use_id);
        self.push_web_search_item_added(&id, output_index, query);
        self.web_searches.push(Some(WebSearchOutput {
            id,
            output_index,
            query: query.to_string(),
            input_json: String::new(),
            results: None,
        }));
        self.web_search_by_block_index
            .insert(block_index, search_index);
        search_index
    }

    fn push_response_created(&mut self) {
        let response = self.build_response_object("in_progress", Vec::new(), None, None, None);
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.created",
            "response": response,
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

    fn push_web_search_item_added(&mut self, item_id: &str, output_index: u64, query: &str) {
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {
                "id": item_id,
                "type": "web_search_call",
                "status": "in_progress",
                "action": { "type": "search", "query": query }
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
        let usage = usage_snapshot.usage.clone().map(|mut usage| {
            self.fill_missing_anthropic_stream_usage(&mut usage);
            usage_to_value(
                usage,
                Some(AnthropicCacheUsage {
                    read_tokens: self.cache_read_input_tokens,
                    creation_tokens: self.cache_creation_input_tokens,
                }),
            )
        });

        let mut snapshots = Vec::new();
        for reasoning in &self.reasonings {
            let Some(reasoning) = reasoning else {
                continue;
            };
            snapshots.push(OutputItemSnapshot::Reasoning {
                id: reasoning.id.clone(),
                output_index: reasoning.output_index,
                text: reasoning.text.clone(),
                encrypted_content: reasoning.encrypted_content.clone(),
            });
        }
        for message in &self.messages {
            let Some(message) = message else {
                continue;
            };
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
                arguments: if call.arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    call.arguments.clone()
                },
            });
        }
        for search in &self.web_searches {
            let Some(search) = search else {
                continue;
            };
            snapshots.push(OutputItemSnapshot::WebSearch {
                id: search.id.clone(),
                output_index: search.output_index,
                query: search.query.clone(),
                results: search.results.clone(),
            });
        }
        snapshots.sort_by_key(|item| match item {
            OutputItemSnapshot::Reasoning { output_index, .. } => *output_index,
            OutputItemSnapshot::Message { output_index, .. } => *output_index,
            OutputItemSnapshot::FunctionCall { output_index, .. } => *output_index,
            OutputItemSnapshot::WebSearch { output_index, .. } => *output_index,
        });

        let status = self.response_status.unwrap_or("completed");
        let output = snapshots
            .iter()
            .map(|snapshot| snapshot_to_output_item(snapshot, status))
            .collect::<Vec<_>>();
        for snapshot in &snapshots {
            self.push_item_done_events(snapshot, status);
        }

        let incomplete_details = self
            .incomplete_reason
            .map(|reason| json!({ "reason": reason }));
        let response = self.build_response_object(
            status,
            output,
            usage,
            Some(completed_at),
            incomplete_details,
        );
        let event_type = if status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        };
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": event_type,
            "response": response,
            "sequence_number": sequence_number
        })));
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
    }

    fn push_item_done_events(&mut self, snapshot: &OutputItemSnapshot, response_status: &str) {
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
                response_status,
            ),
            OutputItemSnapshot::Message {
                id,
                output_index,
                text,
            } => self.push_message_done_events(id, *output_index, text, response_status),
            OutputItemSnapshot::FunctionCall {
                id,
                output_index,
                call_id,
                name,
                arguments,
            } => self.push_function_call_done_events(id, *output_index, call_id, name, arguments),
            OutputItemSnapshot::WebSearch {
                id,
                output_index,
                query,
                results,
            } => self.push_web_search_done_events(id, *output_index, query, results.as_ref()),
        }
    }

    fn push_reasoning_done_events(
        &mut self,
        item_id: &str,
        output_index: u64,
        text: &str,
        encrypted_content: Option<&str>,
        response_status: &str,
    ) {
        let mut item = json!({
            "id": item_id,
            "type": "reasoning",
            "status": response_status,
            "summary": []
        });
        if let Some(item) = item.as_object_mut() {
            if !text.is_empty() {
                item.insert(
                    "summary".to_string(),
                    json!([{ "type": "summary_text", "text": text }]),
                );
            }
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
        response_status: &str,
    ) {
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
                "status": response_status,
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

    fn push_web_search_done_events(
        &mut self,
        item_id: &str,
        output_index: u64,
        query: &str,
        results: Option<&Value>,
    ) {
        let mut item = json!({
            "id": item_id,
            "type": "web_search_call",
            "status": "completed",
            "action": { "type": "search", "query": query }
        });
        if let Some(results) = results {
            item.as_object_mut()
                .expect("web search item is object")
                .insert("results".to_string(), results.clone());
        }
        let sequence_number = self.next_sequence_number();
        self.out.push_back(super::responses_event_sse(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": item,
            "sequence_number": sequence_number
        })));
    }

    fn build_response_object(
        &self,
        status: &str,
        output: Vec<Value>,
        usage: Option<Value>,
        completed_at: Option<i64>,
        incomplete_details: Option<Value>,
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
            "incomplete_details": incomplete_details.unwrap_or(Value::Null),
            "metadata": {}
        })
    }

    fn fill_missing_anthropic_stream_usage(&self, usage: &mut TokenUsage) {
        if usage.input_tokens.is_none() && self.input_tokens > 0 {
            usage.input_tokens = Some(self.input_tokens);
        }
    }

    fn parallel_tool_calls(&self) -> bool {
        self.function_calls
            .iter()
            .filter(|call| call.is_some())
            .count()
            + self
                .web_searches
                .iter()
                .filter(|search| search.is_some())
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
