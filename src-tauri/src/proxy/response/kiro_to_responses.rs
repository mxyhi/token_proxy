use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use std::{collections::{HashMap, HashSet, VecDeque}, sync::Arc};

use super::super::compat_reason;
use super::super::kiro::{EventStreamDecoder, KiroUsage, KiroToolUse};
use super::super::log::{build_log_entry, LogContext, LogWriter, TokenUsage, UsageSnapshot};
use super::super::token_rate::RequestTokenTracker;

pub(super) fn stream_kiro_to_responses<E>(
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
    let state = KiroToResponsesState::new(upstream, context, log, token_tracker);
    futures_util::stream::try_unfold(state, |state| async move { state.step().await })
}

pub(super) fn convert_kiro_response(bytes: &Bytes, model: Option<&str>) -> Result<Bytes, String> {
    let parsed = crate::proxy::kiro::parse_event_stream(bytes)
        .map_err(|message| format!("Failed to parse Kiro response: {message}"))?;
    let now_ms = super::now_ms();
    let response_id = format!("resp_{now_ms}");
    let created_at = (now_ms / 1000) as i64;
    let response = build_response_object(
        parsed.content,
        parsed.tool_uses,
        parsed.usage,
        parsed.stop_reason.as_deref(),
        model,
        response_id,
        created_at,
    );
    serde_json::to_vec(&response)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

pub(super) fn extract_kiro_usage_snapshot(bytes: &Bytes) -> Option<UsageSnapshot> {
    let parsed = crate::proxy::kiro::parse_event_stream(bytes).ok()?;
    let usage_snapshot = UsageSnapshot {
        usage: usage_from_kiro(&parsed.usage),
        cached_tokens: None,
        usage_json: usage_json_from_kiro(&parsed.usage),
    };
    if usage_snapshot.usage.is_none()
        && usage_snapshot.usage_json.is_none()
        && usage_snapshot.cached_tokens.is_none()
    {
        return None;
    }
    Some(usage_snapshot)
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

struct KiroToResponsesState<S> {
    upstream: S,
    decoder: EventStreamDecoder,
    log: Arc<LogWriter>,
    context: LogContext,
    token_tracker: RequestTokenTracker,
    out: VecDeque<Bytes>,
    response_id: String,
    created_at: i64,
    model: String,
    next_output_index: u64,
    message: Option<MessageOutput>,
    function_calls: Vec<Option<FunctionCallOutput>>,
    tool_call_by_id: HashMap<String, usize>,
    processed_tool_ids: HashSet<String>,
    tool_state: Option<ToolUseState>,
    usage: KiroUsage,
    stop_reason: Option<String>,
    sequence: u64,
    sent_done: bool,
    logged: bool,
    upstream_ended: bool,
}

struct ToolUseState {
    id: String,
    name: String,
    input_buffer: String,
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
            out: VecDeque::new(),
            response_id: format!("resp_{now_ms}"),
            created_at,
            model,
            next_output_index: 0,
            message: None,
            function_calls: Vec::new(),
            tool_call_by_id: HashMap::new(),
            processed_tool_ids: HashSet::new(),
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
            self.push_error(error);
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
            "assistantResponseEvent" => self.handle_assistant_response(event_obj).await,
            "toolUseEvent" => self.handle_tool_use_event(event_obj).await,
            "reasoningContentEvent" => self.handle_reasoning_content(event_obj).await,
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
                self.handle_text_delta(text).await;
            }
        }
    }

    async fn handle_text_delta(&mut self, delta: &str) {
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

    async fn handle_tool_use_event(&mut self, event: &Map<String, Value>) {
        let (completed, next_state) =
            process_tool_use_event(event, self.tool_state.take(), &mut self.processed_tool_ids);
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
            if tool_use_id.is_empty() || self.processed_tool_ids.contains(tool_use_id) {
                continue;
            }
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            let input = tool
                .get("input")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            self.processed_tool_ids.insert(tool_use_id.to_string());
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
        let response = build_response_object(
            self.message
                .as_ref()
                .map(|message| message.text.clone())
                .unwrap_or_default(),
            collect_tool_uses(&self.function_calls),
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
        let usage_snapshot = UsageSnapshot {
            usage: usage_from_kiro(&self.usage),
            cached_tokens: None,
            usage_json: usage_json_from_kiro(&self.usage),
        };
        let entry = build_log_entry(&self.context, usage_snapshot, None);
        self.log.clone().write_detached(entry);
    }
}

fn usage_from_kiro(usage: &KiroUsage) -> Option<TokenUsage> {
    if usage.input_tokens.is_none()
        && usage.output_tokens.is_none()
        && usage.total_tokens.is_none()
    {
        return None;
    }
    Some(TokenUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    })
}

fn usage_json_from_kiro(usage: &KiroUsage) -> Option<Value> {
    let input_tokens = usage.input_tokens?;
    let output_tokens = usage.output_tokens.unwrap_or(0);
    let total_tokens = usage
        .total_tokens
        .or_else(|| input_tokens.checked_add(output_tokens))
        .unwrap_or(input_tokens);
    Some(json!({
        "input_tokens": input_tokens,
        "input_tokens_details": { "cached_tokens": 0 },
        "output_tokens": output_tokens,
        "output_tokens_details": { "reasoning_tokens": 0 },
        "total_tokens": total_tokens
    }))
}

fn build_response_object(
    content: String,
    tool_uses: Vec<KiroToolUse>,
    usage: KiroUsage,
    stop_reason: Option<&str>,
    model: Option<&str>,
    response_id: String,
    created_at: i64,
) -> Value {
    let (status, incomplete_reason) =
        compat_reason::responses_status_from_chat_finish_reason(map_stop_reason(stop_reason));
    let status = status.unwrap_or("completed");
    let incomplete_details = incomplete_reason
        .map(|reason| json!({ "reason": reason }))
        .unwrap_or(Value::Null);

    let usage_value = usage_json_from_kiro(&usage);
    let usage_json = usage_value.unwrap_or(Value::Null);
    let parallel_tool_calls = tool_uses.len() > 1;

    let mut output = Vec::new();
    if !content.trim().is_empty() || tool_uses.is_empty() {
        output.push(json!({
            "type": "message",
            "id": "msg_0",
            "status": "completed",
            "role": "assistant",
            "content": [
                { "type": "output_text", "text": content, "annotations": [] }
            ]
        }));
    }
    for (index, tool_use) in tool_uses.iter().enumerate() {
        let arguments = serde_json::to_string(&tool_use.input).unwrap_or_default();
        output.push(json!({
            "id": format!("fc_{index}"),
            "type": "function_call",
            "status": "completed",
            "arguments": arguments,
            "call_id": tool_use.tool_use_id,
            "name": tool_use.name
        }));
    }

    json!({
        "id": response_id,
        "object": "response",
        "created_at": created_at,
        "status": status,
        "error": null,
        "incomplete_details": incomplete_details,
        "model": model.unwrap_or("unknown"),
        "parallel_tool_calls": parallel_tool_calls,
        "output": output,
        "usage": usage_json
    })
}

fn map_stop_reason(stop_reason: Option<&str>) -> Option<&'static str> {
    match stop_reason {
        Some("max_tokens") => Some("length"),
        Some("content_filtered") => Some("content_filter"),
        Some("tool_use") => Some("tool_calls"),
        Some("stop_sequence") | Some("end_turn") => Some("stop"),
        Some(other) if other.is_empty() => None,
        Some(_) => Some("stop"),
        None => None,
    }
}

fn collect_tool_uses(function_calls: &[Option<FunctionCallOutput>]) -> Vec<KiroToolUse> {
    let mut output = Vec::new();
    for call in function_calls {
        let Some(call) = call else {
            continue;
        };
        let input =
            serde_json::from_str::<Map<String, Value>>(&call.arguments).unwrap_or_default();
        output.push(KiroToolUse {
            tool_use_id: call.call_id.clone(),
            name: call.name.clone(),
            input,
        });
    }
    output
}

fn detect_event_type(event: &Map<String, Value>) -> &str {
    for key in [
        "assistantResponseEvent",
        "toolUseEvent",
        "reasoningContentEvent",
        "messageStopEvent",
        "messageMetadataEvent",
        "metadataEvent",
        "usageEvent",
        "usage",
        "supplementaryWebLinksEvent",
    ] {
        if event.contains_key(key) {
            return key;
        }
    }
    ""
}

fn extract_error(event: &Map<String, Value>) -> Option<String> {
    if let Some(Value::String(err_type)) = event.get("_type") {
        let message = event.get("message").and_then(Value::as_str).unwrap_or("");
        return Some(format!("Kiro error: {err_type} {message}"));
    }
    if let Some(Value::String(kind)) = event.get("type") {
        if matches!(kind.as_str(), "error" | "exception" | "internalServerException") {
            let message = event.get("message").and_then(Value::as_str).unwrap_or("");
            if message.is_empty() {
                if let Some(Value::Object(err_obj)) = event.get("error") {
                    if let Some(text) = err_obj.get("message").and_then(Value::as_str) {
                        return Some(format!("Kiro error: {text}"));
                    }
                }
            }
            return Some(format!("Kiro error: {message}"));
        }
    }
    None
}

fn update_stop_reason(event: &Map<String, Value>, stop_reason: &mut Option<String>) {
    if let Some(reason) = event.get("stop_reason").and_then(Value::as_str) {
        *stop_reason = Some(reason.to_string());
    }
    if let Some(reason) = event.get("stopReason").and_then(Value::as_str) {
        *stop_reason = Some(reason.to_string());
    }
}

fn update_usage(event: &Map<String, Value>, usage: &mut KiroUsage) {
    if let Some(tokens) = event.get("inputTokens").and_then(Value::as_u64) {
        usage.input_tokens = Some(tokens);
    }
    if let Some(tokens) = event.get("outputTokens").and_then(Value::as_u64) {
        usage.output_tokens = Some(tokens);
    }
    if let Some(tokens) = event.get("totalTokens").and_then(Value::as_u64) {
        usage.total_tokens = Some(tokens);
    }

    if let Some(metadata) = event.get("messageMetadataEvent").and_then(Value::as_object) {
        update_usage_from_metadata(metadata, usage);
    } else if let Some(metadata) = event.get("metadataEvent").and_then(Value::as_object) {
        update_usage_from_metadata(metadata, usage);
    }

    if let Some(usage_obj) = event.get("usage").and_then(Value::as_object) {
        update_usage_from_usage_obj(usage_obj, usage);
    }
    if let Some(usage_obj) = event.get("usageEvent").and_then(Value::as_object) {
        update_usage_from_usage_obj(usage_obj, usage);
    }

    if let Some(links) = event
        .get("supplementaryWebLinksEvent")
        .and_then(Value::as_object)
    {
        if let Some(tokens) = links.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = links.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }
}

fn update_usage_from_metadata(metadata: &Map<String, Value>, usage: &mut KiroUsage) {
    if let Some(token_usage) = metadata.get("tokenUsage").and_then(Value::as_object) {
        if let Some(tokens) = token_usage.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("uncachedInputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
        if let Some(tokens) = token_usage.get("cacheReadInputTokens").and_then(Value::as_u64) {
            let current = usage.input_tokens.unwrap_or(0);
            usage.input_tokens = Some(current + tokens);
        }
    }

    if usage.input_tokens.is_none() {
        if let Some(tokens) = metadata.get("inputTokens").and_then(Value::as_u64) {
            usage.input_tokens = Some(tokens);
        }
    }
    if usage.output_tokens.is_none() {
        if let Some(tokens) = metadata.get("outputTokens").and_then(Value::as_u64) {
            usage.output_tokens = Some(tokens);
        }
    }
    if usage.total_tokens.is_none() {
        if let Some(tokens) = metadata.get("totalTokens").and_then(Value::as_u64) {
            usage.total_tokens = Some(tokens);
        }
    }
}

fn update_usage_from_usage_obj(usage_obj: &Map<String, Value>, usage: &mut KiroUsage) {
    let input_tokens = usage_obj
        .get("input_tokens")
        .or_else(|| usage_obj.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage_obj
        .get("output_tokens")
        .or_else(|| usage_obj.get("completion_tokens"))
        .and_then(Value::as_u64);
    let total_tokens = usage_obj.get("total_tokens").and_then(Value::as_u64);

    if input_tokens.is_some() {
        usage.input_tokens = input_tokens;
    }
    if output_tokens.is_some() {
        usage.output_tokens = output_tokens;
    }
    if total_tokens.is_some() {
        usage.total_tokens = total_tokens;
    }
}

fn process_tool_use_event(
    event: &Map<String, Value>,
    current: Option<ToolUseState>,
    processed: &mut HashSet<String>,
) -> (Vec<KiroToolUse>, Option<ToolUseState>) {
    let mut tool_uses = Vec::new();
    let mut state = current;

    let source = event
        .get("toolUseEvent")
        .and_then(Value::as_object)
        .unwrap_or(event);

    let tool_use_id = source
        .get("toolUseId")
        .or_else(|| source.get("tool_use_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = source.get("name").and_then(Value::as_str).unwrap_or("");
    let stop = source.get("stop").and_then(Value::as_bool).unwrap_or(false);

    if !tool_use_id.is_empty() && !name.is_empty() {
        if let Some(current_state) = &state {
            if current_state.id != tool_use_id {
                state = None;
            }
        }

        if state.is_none() && !processed.contains(tool_use_id) {
            state = Some(ToolUseState {
                id: tool_use_id.to_string(),
                name: name.to_string(),
                input_buffer: String::new(),
            });
        }
    }

    if let Some(current_state) = &mut state {
        if let Some(Value::String(fragment)) = source.get("input") {
            current_state.input_buffer.push_str(fragment);
        } else if let Some(Value::Object(input)) = source.get("input") {
            let serialized = serde_json::to_string(input).unwrap_or_default();
            current_state.input_buffer = serialized;
        }
    }

    if stop {
        if let Some(current_state) = state.take() {
            let input = parse_tool_input(&current_state.input_buffer);
            processed.insert(current_state.id.clone());
            tool_uses.push(KiroToolUse {
                tool_use_id: current_state.id,
                name: current_state.name,
                input,
            });
        }
    }

    (tool_uses, state)
}

fn parse_tool_input(raw: &str) -> Map<String, Value> {
    if raw.trim().is_empty() {
        return Map::new();
    }
    let repaired = repair_json(raw);
    serde_json::from_str::<Map<String, Value>>(&repaired).unwrap_or_default()
}

fn repair_json(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ',' {
            if let Some(next) = chars.peek() {
                if *next == '}' || *next == ']' {
                    continue;
                }
            }
        }
        output.push(ch);
    }
    output
}
