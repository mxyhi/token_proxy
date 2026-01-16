//! Gemini 流式响应 → OpenAI Chat 流式响应转换

use axum::body::Bytes;
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc};

use crate::proxy::log::{build_log_entry, LogContext, LogWriter};
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
            model: context.model.clone().unwrap_or_else(|| "gemini".to_string()),
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
                self.push_done(gemini_finish_reason_to_chat(finish_reason, false));
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
                let tool_call =
                    gemini_function_call_to_chat_tool_call(function_call, self.tool_call_index);
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
            let chat_reason = gemini_finish_reason_to_chat(Some(reason), has_tool_calls);
            self.push_done(chat_reason);
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
            Some(finish_reason),
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
