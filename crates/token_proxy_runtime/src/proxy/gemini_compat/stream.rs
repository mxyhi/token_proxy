//! Gemini 流式响应 → OpenAI Chat 流式响应转换

use axum::{body::Bytes, http::StatusCode};
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc};

use crate::proxy::log::{attach_response_body, build_log_entry, LogContext, LogWriter};
use crate::proxy::response::STREAM_DROPPED_ERROR;
use crate::proxy::sse::SseEventParser;
use crate::proxy::token_rate::RequestTokenTracker;
use crate::proxy::usage::SseUsageCollector;

use super::tools::gemini_function_call_to_chat_tool_call;

/// 将 Gemini 流式响应转换为 OpenAI Chat 流式响应
pub(crate) fn stream_gemini_to_chat<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = GeminiToChatState::new(upstream, context, log, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

/// 将 OpenAI Chat 流式响应转换为 Gemini 流式响应
pub(crate) fn stream_chat_to_gemini<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    token_tracker: RequestTokenTracker,
) -> impl futures_util::stream::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    let state = ChatToGeminiState::new(upstream, context, log, token_tracker);
    try_unfold(state, |state| async move { state.step().await })
}

struct GeminiToChatState<S> {
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
    tool_call_index: usize,
    finish_reason: Option<&'static str>,
    final_usage: Value,
    response_body_buf: String,
}

struct ToolCallState {
    name: String,
    arguments: String,
}

struct ChatToGeminiState<S> {
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
    tool_calls: Vec<Option<ToolCallState>>,
    response_body_buf: String,
}

impl<S> GeminiToChatState<S> {
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

impl<S> Drop for GeminiToChatState<S> {
    fn drop(&mut self) {
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S> ChatToGeminiState<S> {
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

impl<S> Drop for ChatToGeminiState<S> {
    fn drop(&mut self) {
        self.write_log_once(Some(STREAM_DROPPED_ERROR.to_string()));
    }
}

impl<S, E> GeminiToChatState<S>
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
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            model: context
                .model
                .clone()
                .unwrap_or_else(|| "gemini".to_string()),
            context,
            token_tracker,
            out: VecDeque::new(),
            chat_id: format!("chatcmpl_gemini_{now_ms}"),
            created: (now_ms / 1000) as i64,
            sent_role: false,
            sent_done: false,
            logged: false,
            upstream_ended: false,
            tool_call_index: 0,
            finish_reason: None,
            final_usage: Value::Null,
            response_body_buf: String::new(),
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        loop {
            if let Some(next) = self.out.pop_front() {
                self.context.mark_first_client_flush();
                return Ok(Some((next, self)));
            }

            if self.upstream_ended {
                return Ok(None);
            }

            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    self.context.mark_upstream_first_byte();
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
                        if !text.is_empty() {
                            self.context.mark_first_output();
                        }
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
                        self.push_done("stop");
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
            self.push_done("stop");
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };

        // 用量可在 finishReason 之后单独到达，直到 EOF/[DONE] 才发送下游终止帧。
        if let Some(usage) = value.get("usageMetadata").filter(|usage| usage.is_object()) {
            token_proxy_protocol::gemini_usage::merge_snapshot(&mut self.final_usage, usage);
        }
        // 处理 Gemini 响应格式
        let Some(candidates) = value.get("candidates").and_then(Value::as_array) else {
            return;
        };

        for candidate in candidates {
            self.handle_candidate(candidate, token_texts);
        }
    }

    fn handle_candidate(&mut self, candidate: &Value, token_texts: &mut Vec<String>) {
        let Some(candidate) = candidate.as_object() else {
            return;
        };

        // 检查 finishReason
        let finish_reason = candidate.get("finishReason").and_then(Value::as_str);

        let Some(content) = candidate.get("content").and_then(Value::as_object) else {
            // 如果有 finishReason 但没有 content，发送完成信号
            if finish_reason.is_some() {
                self.finish_reason = Some(gemini_finish_reason_to_chat(
                    finish_reason,
                    self.tool_call_index > 0,
                ));
            }
            return;
        };

        let Some(parts) = content.get("parts").and_then(Value::as_array) else {
            return;
        };

        let mut has_tool_calls = false;

        for part in parts {
            let Some(part) = part.as_object() else {
                continue;
            };

            // 思考内容走 reasoning_content，不能混进正文（与非流式转换一致）。
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        token_texts.push(text.to_string());
                        self.ensure_role_sent();
                        self.out.push_back(chat_chunk_sse(
                            &self.chat_id,
                            self.created,
                            &self.model,
                            json!({ "reasoning_content": text }),
                            None,
                        ));
                    }
                }
                continue;
            }

            // 文本内容
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    token_texts.push(text.to_string());
                    self.ensure_role_sent();
                    self.out.push_back(chat_chunk_sse(
                        &self.chat_id,
                        self.created,
                        &self.model,
                        json!({ "content": text }),
                        None,
                    ));
                }
            }

            // 函数调用
            if let Some(function_call) = part.get("functionCall").and_then(Value::as_object) {
                has_tool_calls = true;
                self.ensure_role_sent();
                let mut tool_call =
                    gemini_function_call_to_chat_tool_call(function_call, self.tool_call_index);
                // Gemini 3 下一轮会校验当前轮首个 functionCall 的签名；流式也必须随调用透传。
                if let Some(signature) = part.get("thoughtSignature").and_then(Value::as_str) {
                    tool_call["provider_specific_fields"] = json!({
                        "provider": "gemini",
                        "thought_signature": signature
                    });
                    tracing::debug!("forwarded Gemini stream function call thought signature");
                }
                self.tool_call_index += 1;

                // 发送工具调用 delta
                self.out.push_back(chat_chunk_sse(
                    &self.chat_id,
                    self.created,
                    &self.model,
                    json!({ "tool_calls": [tool_call] }),
                    None,
                ));
            }
        }

        // 处理完成原因
        if let Some(reason) = finish_reason {
            let chat_reason = gemini_finish_reason_to_chat(
                Some(reason),
                has_tool_calls || self.tool_call_index > 0,
            );
            self.finish_reason = Some(chat_reason);
        }
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

    fn push_done(&mut self, finish_reason: &str) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;
        self.out.push_back(chat_chunk_sse(
            &self.chat_id,
            self.created,
            &self.model,
            json!({}),
            Some(self.finish_reason.unwrap_or(finish_reason)),
        ));
        if self.final_usage.is_object() {
            let usage = token_proxy_protocol::gemini_usage::to_chat(&self.final_usage);
            let chunk = json!({"id":self.chat_id,"object":"chat.completion.chunk",
                "created":self.created,"model":self.model,"choices":[],"usage":usage});
            self.out
                .push_back(Bytes::from(format!("data: {chunk}\n\n")));
        }
        self.out.push_back(Bytes::from("data: [DONE]\n\n"));
    }

    fn log_usage_once(&mut self) {
        self.write_log_once(None);
    }
}

impl<S, E> ChatToGeminiState<S>
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
        Self {
            upstream,
            parser: SseEventParser::new(),
            collector: SseUsageCollector::new(),
            log,
            context,
            token_tracker,
            out: VecDeque::new(),
            sent_done: false,
            logged: false,
            upstream_ended: false,
            tool_calls: Vec::new(),
            response_body_buf: String::new(),
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        loop {
            if let Some(next) = self.out.pop_front() {
                self.context.mark_first_client_flush();
                return Ok(Some((next, self)));
            }

            if self.upstream_ended {
                return Ok(None);
            }

            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    self.context.mark_upstream_first_byte();
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
                        if !text.is_empty() {
                            self.context.mark_first_output();
                        }
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
                        self.push_finish_reason("STOP", None);
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
            self.sent_done = true;
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
            self.fail_stream(error);
            return;
        }

        let usage = value.get("usage").and_then(chat_usage_to_gemini_usage);
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        let delta = choice.get("delta").and_then(Value::as_object).cloned();
        let finish_reason = choice.get("finish_reason").and_then(Value::as_str);

        if let Some(delta) = delta.as_ref() {
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                token_texts.push(content.to_string());
                self.push_text_delta(content, usage.clone());
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for tool_call in tool_calls {
                    if let Some(part) = self.update_tool_call(tool_call) {
                        self.push_candidate_part(part, usage.clone());
                    }
                }
            }
        }

        if let Some(reason) = finish_reason {
            let mapped = chat_finish_reason_to_gemini(reason);
            self.push_finish_reason(mapped, usage);
        }
    }

    fn push_text_delta(&mut self, text: &str, usage: Option<Value>) {
        let candidate = json!({
            "index": 0,
            "content": { "role": "model", "parts": [{ "text": text }] }
        });
        self.out.push_back(gemini_chunk_sse(candidate, usage));
    }

    fn push_candidate_part(&mut self, part: Value, usage: Option<Value>) {
        let candidate = json!({
            "index": 0,
            "content": { "role": "model", "parts": [part] }
        });
        self.out.push_back(gemini_chunk_sse(candidate, usage));
    }

    fn push_finish_reason(&mut self, reason: &str, usage: Option<Value>) {
        if self.sent_done {
            return;
        }
        self.sent_done = true;
        let candidate = json!({
            "index": 0,
            "content": { "role": "model", "parts": [] },
            "finishReason": reason
        });
        self.out.push_back(gemini_chunk_sse(candidate, usage));
    }

    fn fail_stream(&mut self, error: &Value) {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("Upstream Chat stream failed")
            .to_string();
        let status = crate::proxy::response::responses_error::openai_error_status(error);
        tracing::warn!(
            provider = %self.context.provider,
            upstream = %self.context.upstream_id,
            status = status.as_u16(),
            error = %message,
            "converted Chat stream error to Gemini error"
        );
        self.context.status = status.as_u16();
        self.sent_done = true;
        self.out.push_back(gemini_error_sse(status, &message));
        self.write_log_once(Some(message));
    }

    fn update_tool_call(&mut self, tool_call: &Value) -> Option<Value> {
        let Some(tool_call) = tool_call.as_object() else {
            return None;
        };
        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let function = tool_call.get("function").and_then(Value::as_object)?;
        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
        let delta = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("");

        if self.tool_calls.len() <= index {
            self.tool_calls.resize_with(index + 1, || None);
        }
        let state = self.tool_calls[index].get_or_insert(ToolCallState {
            name: String::new(),
            arguments: String::new(),
        });
        if !name.is_empty() {
            state.name = name.to_string();
        }
        if !delta.is_empty() {
            state.arguments.push_str(delta);
        }
        if state.name.is_empty() {
            return None;
        }
        let args = if state.arguments.is_empty() {
            json!({})
        } else {
            match serde_json::from_str::<Value>(&state.arguments) {
                Ok(args) => args,
                Err(_) => return None,
            }
        };
        let name = state.name.clone();
        self.tool_calls[index] = None;
        Some(json!({
            "functionCall": { "name": name, "args": args }
        }))
    }

    fn log_usage_once(&mut self) {
        self.write_log_once(None);
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
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish_reason
        }]
    });
    Bytes::from(format!("data: {}\n\n", chunk))
}

fn gemini_finish_reason_to_chat(reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        return "tool_calls";
    }
    match reason {
        Some("STOP") => "stop",
        Some("MAX_TOKENS") => "length",
        Some("SAFETY") => "content_filter",
        Some("RECITATION") => "content_filter",
        Some("OTHER") => "stop",
        Some("BLOCKLIST") => "content_filter",
        Some("PROHIBITED_CONTENT") => "content_filter",
        Some("SPII") => "content_filter",
        _ => "stop",
    }
}

fn gemini_chunk_sse(candidate: Value, usage: Option<Value>) -> Bytes {
    let mut payload = json!({ "candidates": [candidate] });
    if let Some(usage) = usage {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("usageMetadata".to_string(), usage);
        }
    }
    Bytes::from(format!("data: {}\n\n", payload))
}

pub(crate) fn gemini_error_sse(status: StatusCode, message: &str) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({
            "error": {
                "code": status.as_u16(),
                "message": message,
                "status": google_rpc_status(status)
            }
        })
    ))
}

fn google_rpc_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::CONFLICT => "ABORTED",
        StatusCode::TOO_MANY_REQUESTS => "RESOURCE_EXHAUSTED",
        StatusCode::INTERNAL_SERVER_ERROR => "INTERNAL",
        StatusCode::SERVICE_UNAVAILABLE => "UNAVAILABLE",
        StatusCode::GATEWAY_TIMEOUT => "DEADLINE_EXCEEDED",
        _ => "UNKNOWN",
    }
}

fn chat_usage_to_gemini_usage(usage: &Value) -> Option<Value> {
    let prompt_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
    let completion_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
    if prompt_tokens.is_none() && completion_tokens.is_none() && total_tokens.is_none() {
        return None;
    }
    Some(json!({
        "promptTokenCount": prompt_tokens.unwrap_or(0),
        "candidatesTokenCount": completion_tokens.unwrap_or(0),
        "totalTokenCount": total_tokens.unwrap_or_else(|| prompt_tokens.unwrap_or(0) + completion_tokens.unwrap_or(0))
    }))
}

fn chat_finish_reason_to_gemini(reason: &str) -> &'static str {
    match reason {
        "stop" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        _ => "STOP",
    }
}
