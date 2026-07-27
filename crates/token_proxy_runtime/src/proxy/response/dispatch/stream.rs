use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{stream::try_unfold, StreamExt};
use serde_json::{json, Map, Value};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::timeout;

use super::super::super::{
    codex_compat, gemini_compat, http,
    log::{build_log_entry, LogContext, LogWriter, UsageSnapshot},
    openai_compat::{
        images::{image_generation_usage, image_output_mime_type},
        FormatTransform,
    },
    redact::redact_query_param_value,
    server_helpers::log_debug_headers_body,
    sse::SseEventParser,
    token_rate::RequestTokenTracker,
};
use super::super::{
    anthropic_to_responses, chat_to_responses, kiro_to_anthropic, responses_error,
    responses_to_anthropic, responses_to_chat, streaming, upstream_stream,
    NonRetryableSemanticResponse, RetryableStreamResponse, PROVIDER_CODEX, PROVIDER_GEMINI,
    PROVIDER_OPENAI, PROVIDER_OPENAI_RESPONSES, PROVIDER_XAI,
};
use super::buffered;
use token_proxy_protocol::xai_forbidden::{
    classify_payload, payload_error_code, XaiForbiddenScope,
};

type UpstreamBytesStream = futures_util::stream::BoxStream<
    'static,
    Result<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>,
>;
type ResponseStream = futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>;
const DEBUG_BODY_LOG_LIMIT_BYTES: usize = usize::MAX;

struct XaiClientToolStreamState {
    upstream: UpstreamBytesStream,
    parser: SseEventParser,
    restorer: token_proxy_protocol::xai_client_tools::XaiClientToolStreamRestorer,
    out: VecDeque<Bytes>,
    ended: bool,
}

fn restore_xai_client_tool_stream(
    upstream: UpstreamBytesStream,
    mapping: token_proxy_protocol::xai_client_tools::XaiClientToolMapping,
    restore_client_tools: bool,
) -> UpstreamBytesStream {
    try_unfold(
        XaiClientToolStreamState {
            upstream,
            parser: SseEventParser::new(),
            restorer: if restore_client_tools {
                token_proxy_protocol::xai_client_tools::XaiClientToolStreamRestorer::new(mapping)
            } else {
                token_proxy_protocol::xai_client_tools::XaiClientToolStreamRestorer::new_filter_only(
                    mapping,
                )
            },
            out: VecDeque::new(),
            ended: false,
        },
        |mut state| async move {
            loop {
                if let Some(chunk) = state.out.pop_front() {
                    return Ok(Some((chunk, state)));
                }
                if state.ended {
                    return Ok(None);
                }
                match state.upstream.next().await {
                    Some(Ok(chunk)) => {
                        let mut events = Vec::new();
                        state.parser.push_chunk(&chunk, |event| events.push(event));
                        for event in events {
                            state.push_event(&event);
                        }
                    }
                    Some(Err(error)) => return Err(error),
                    None => {
                        state.ended = true;
                        let mut events = Vec::new();
                        state.parser.finish(|event| events.push(event));
                        for event in events {
                            state.push_event(&event);
                        }
                    }
                }
            }
        },
    )
    .boxed()
}

impl XaiClientToolStreamState {
    fn push_event(&mut self, event: &str) {
        if event.trim() == "[DONE]" {
            self.out.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
            return;
        }
        match self.restorer.restore_event(event) {
            Ok(events) => {
                for event in events {
                    self.out
                        .push_back(Bytes::from(format!("data: {event}\n\n")));
                }
            }
            Err(error) => {
                // 非工具事件若不是合法 JSON，保留原 data，避免代理扩大上游协议故障。
                tracing::warn!(error = %error, "failed to restore xAI client tool stream event");
                self.out
                    .push_back(Bytes::from(format!("data: {event}\n\n")));
            }
        }
    }
}

pub(super) async fn build_stream_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    xai_client_tools: Option<token_proxy_protocol::xai_client_tools::XaiClientToolMapping>,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    response_format: Option<&str>,
    stream_first_output_timeout_remaining: Duration,
    stream_first_output_timeout: Duration,
    sync_response_timeout: Duration,
) -> Response {
    let mut context = context;
    let upstream = match prepare_upstream_stream(
        status,
        &headers,
        upstream_res,
        response_transform,
        &mut context,
        &log,
        stream_first_output_timeout_remaining,
        stream_first_output_timeout,
        sync_response_timeout,
    )
    .await
    {
        Ok(stream) => stream,
        Err(response) => return response,
    };
    log_debug_headers_body(
        "upstream.response.headers",
        Some(&headers),
        None,
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    let upstream = log_upstream_stream_if_debug(upstream);
    let upstream = match xai_client_tools {
        Some(mapping) if response_transform == FormatTransform::None => {
            restore_xai_client_tool_stream(upstream, mapping, true)
        }
        Some(mapping) if mapping.filters_internal_x_search() => {
            restore_xai_client_tool_stream(upstream, mapping, false)
        }
        _ => upstream,
    };

    let stream = stream_for_transform(
        response_transform,
        upstream,
        context,
        log,
        request_tracker,
        estimated_input_tokens,
        model_override,
        response_format,
        sync_response_timeout,
    );
    log_debug_headers_body(
        "outbound.response.headers",
        Some(&headers),
        None,
        DEBUG_BODY_LOG_LIMIT_BYTES,
    )
    .await;
    let stream = log_response_stream_if_debug(stream);
    let body = Body::from_stream(stream);
    http::build_response(status, headers, body)
}

fn stream_for_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
    model_override: Option<&str>,
    response_format: Option<&str>,
    sync_response_timeout: Duration,
) -> ResponseStream {
    if is_simple_transform(transform) {
        return stream_for_simple_transform(
            transform,
            upstream,
            context,
            log,
            request_tracker,
            model_override,
            estimated_input_tokens,
            response_format,
            sync_response_timeout,
        );
    }
    stream_for_composed_transform(
        transform,
        upstream,
        context,
        log,
        request_tracker,
        response_format,
    )
}

fn is_simple_transform(transform: FormatTransform) -> bool {
    matches!(
        transform,
        FormatTransform::None
            | FormatTransform::ResponsesToChat
            | FormatTransform::ChatToResponses
            | FormatTransform::ResponsesToAnthropic
            | FormatTransform::AnthropicToResponses
            | FormatTransform::GeminiToChat
            | FormatTransform::ChatToGemini
            | FormatTransform::KiroToAnthropic
            | FormatTransform::CodexToChat
            | FormatTransform::CodexToResponses
            | FormatTransform::CodexToImagesGenerations
            | FormatTransform::ChatToCodex
            | FormatTransform::ResponsesToCodex
            | FormatTransform::ResponsesCompactToCodex
    )
}

fn stream_for_simple_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    response_format: Option<&str>,
    sync_response_timeout: Duration,
) -> ResponseStream {
    match transform {
        FormatTransform::None
        | FormatTransform::ResponsesToChat
        | FormatTransform::ChatToResponses
        | FormatTransform::ResponsesToAnthropic
        | FormatTransform::AnthropicToResponses => stream_for_basic_transform(
            transform,
            upstream,
            context,
            log,
            request_tracker,
            model_override,
            sync_response_timeout,
        ),
        _ => stream_for_simple_extended(
            transform,
            upstream,
            context,
            log,
            request_tracker,
            estimated_input_tokens,
            response_format,
            sync_response_timeout,
        ),
    }
}

fn stream_for_basic_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    model_override: Option<&str>,
    sync_response_timeout: Duration,
) -> ResponseStream {
    match transform {
        FormatTransform::None => {
            let semantic_timeout =
                openai_semantic_timeout(&context.provider, &context.path, sync_response_timeout);
            stream_with_optional_model_override(
                upstream,
                context,
                log,
                request_tracker,
                model_override,
                semantic_timeout,
            )
        }
        FormatTransform::ResponsesToChat => {
            responses_to_chat::stream_responses_to_chat(upstream, context, log, request_tracker)
                .boxed()
        }
        FormatTransform::ChatToResponses => {
            chat_to_responses::stream_chat_to_responses(upstream, context, log, request_tracker)
                .boxed()
        }
        FormatTransform::ResponsesToAnthropic => {
            responses_to_anthropic::stream_responses_to_anthropic(
                upstream,
                context,
                log,
                request_tracker,
            )
            .boxed()
        }
        FormatTransform::AnthropicToResponses => {
            anthropic_to_responses::stream_anthropic_to_responses(
                upstream,
                context,
                log,
                request_tracker,
            )
            .boxed()
        }
        _ => streaming::stream_with_logging(upstream, context, log, request_tracker).boxed(),
    }
}

fn stream_for_simple_extended(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    estimated_input_tokens: Option<u64>,
    response_format: Option<&str>,
    sync_response_timeout: Duration,
) -> ResponseStream {
    match transform {
        FormatTransform::GeminiToChat => {
            gemini_compat::stream_gemini_to_chat(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::ChatToGemini => {
            gemini_compat::stream_chat_to_gemini(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::KiroToAnthropic => kiro_to_anthropic::stream_kiro_to_anthropic(
            upstream,
            context,
            log,
            request_tracker,
            estimated_input_tokens,
        )
        .boxed(),
        FormatTransform::CodexToChat => {
            codex_compat::stream_codex_to_chat(upstream, context, log, request_tracker).boxed()
        }
        FormatTransform::CodexToResponses => {
            codex_compat::stream_codex_to_responses_with_semantic_timeout(
                upstream,
                context,
                log,
                request_tracker,
                Some(sync_response_timeout),
            )
            .boxed()
        }
        FormatTransform::CodexToImagesGenerations => stream_codex_to_images_generation(
            upstream,
            context,
            log,
            request_tracker,
            response_format,
        ),
        FormatTransform::ChatToCodex
        | FormatTransform::ResponsesToCodex
        | FormatTransform::ResponsesCompactToCodex => {
            streaming::stream_with_logging(upstream, context, log, request_tracker).boxed()
        }
        _ => streaming::stream_with_logging(upstream, context, log, request_tracker).boxed(),
    }
}

#[derive(Clone, Default)]
struct ImageGenerationStreamMeta {
    created_at: i64,
    output_format: Option<String>,
    size: Option<String>,
    background: Option<String>,
    quality: Option<String>,
    model: Option<String>,
}

#[derive(Clone)]
struct ImageGenerationStreamResult {
    result: String,
    revised_prompt: Option<String>,
    meta: ImageGenerationStreamMeta,
}

struct ImageGenerationStreamState {
    upstream: UpstreamBytesStream,
    parser: SseEventParser,
    out: VecDeque<Bytes>,
    response_format: Option<String>,
    meta: ImageGenerationStreamMeta,
    usage: Option<Value>,
    pending: Vec<ImageGenerationStreamResult>,
    seen: HashSet<String>,
    terminal_seen: bool,
    upstream_ended: bool,
}

fn stream_codex_to_images_generation(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_format: Option<&str>,
) -> ResponseStream {
    let transformed = try_unfold(
        ImageGenerationStreamState::new(upstream, response_format),
        |state| async move { state.step().await },
    )
    .boxed();
    streaming::stream_with_logging(transformed, context, log, request_tracker).boxed()
}

impl ImageGenerationStreamState {
    fn new(upstream: UpstreamBytesStream, response_format: Option<&str>) -> Self {
        Self {
            upstream,
            parser: SseEventParser::new(),
            out: VecDeque::new(),
            response_format: response_format.map(str::to_string),
            meta: ImageGenerationStreamMeta::default(),
            usage: None,
            pending: Vec::new(),
            seen: HashSet::new(),
            terminal_seen: false,
            upstream_ended: false,
        }
    }

    async fn step(mut self) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        loop {
            if let Some(next) = self.out.pop_front() {
                return Ok(Some((next, self)));
            }
            if self.terminal_seen || self.upstream_ended {
                return Ok(None);
            }

            match self.upstream.next().await {
                Some(Ok(chunk)) => {
                    let mut events = Vec::new();
                    self.parser.push_chunk(&chunk, |data| events.push(data));
                    for data in events {
                        self.process_data(&data);
                    }
                }
                Some(Err(err)) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
                }
                None => {
                    self.upstream_ended = true;
                    let mut events = Vec::new();
                    self.parser.finish(|data| events.push(data));
                    for data in events {
                        self.process_data(&data);
                    }
                    if !self.terminal_seen {
                        self.emit_pending_or_error(
                            "stream disconnected before image generation completed",
                        );
                    }
                }
            }
        }
    }

    fn process_data(&mut self, data: &str) {
        if data.trim() == "[DONE]" {
            if !self.terminal_seen {
                self.emit_pending_or_error("stream disconnected before image generation completed");
            }
            self.terminal_seen = true;
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        self.merge_meta_from_event(&value);
        match value.get("type").and_then(Value::as_str) {
            Some("response.image_generation_call.partial_image") => self.emit_partial(&value),
            Some("response.output_item.done") => self.push_pending_output_item(&value),
            Some("response.completed") => {
                self.merge_usage_from_completed(&value);
                let results = self.completed_results(&value);
                if results.is_empty() {
                    self.emit_pending_or_error("upstream did not return image output");
                } else {
                    for result in results {
                        self.emit_completed(result);
                    }
                }
                self.terminal_seen = true;
            }
            Some("response.failed")
            | Some("response.incomplete")
            | Some("response.canceled")
            | Some("response.cancelled")
            | Some("error") => {
                self.emit_error("upstream image generation failed");
                self.terminal_seen = true;
            }
            _ => {}
        }
    }

    fn merge_usage_from_completed(&mut self, value: &Value) {
        let Some(response) = value.get("response").and_then(Value::as_object) else {
            return;
        };
        self.usage = image_generation_usage(response);
    }

    fn merge_meta_from_event(&mut self, value: &Value) {
        if let Some(created_at) = value
            .get("response")
            .and_then(|response| response.get("created_at"))
            .and_then(Value::as_i64)
            .filter(|created_at| *created_at > 0)
        {
            self.meta.created_at = created_at;
        }
        let Some(tool) = value
            .get("response")
            .and_then(|response| response.get("tools"))
            .and_then(Value::as_array)
            .and_then(|tools| tools.first())
        else {
            return;
        };
        set_optional_string(&mut self.meta.output_format, tool, "output_format");
        set_optional_string(&mut self.meta.size, tool, "size");
        set_optional_string(&mut self.meta.background, tool, "background");
        set_optional_string(&mut self.meta.quality, tool, "quality");
        set_optional_string(&mut self.meta.model, tool, "model");
    }

    fn emit_partial(&mut self, value: &Value) {
        let Some(result) = value
            .get("partial_image_b64")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        let mut meta = self.meta.clone();
        set_optional_string(&mut meta.output_format, value, "output_format");
        set_optional_string(&mut meta.background, value, "background");
        let mut payload = self.image_payload("image_generation.partial_image", result, &meta);
        if let Some(index) = value.get("partial_image_index").and_then(Value::as_i64) {
            payload.insert("partial_image_index".to_string(), json!(index));
        }
        self.push_event("image_generation.partial_image", payload);
    }

    fn push_pending_output_item(&mut self, value: &Value) {
        let Some(item) = value.get("item").filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("image_generation_call")
        }) else {
            return;
        };
        let Some(result) = self.result_from_item(item) else {
            return;
        };
        self.pending.push(result);
    }

    fn completed_results(&self, value: &Value) -> Vec<ImageGenerationStreamResult> {
        value
            .get("response")
            .and_then(|response| response.get("output"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("type").and_then(Value::as_str) == Some("image_generation_call")
                    })
                    .filter_map(|item| self.result_from_item(item))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn result_from_item(&self, item: &Value) -> Option<ImageGenerationStreamResult> {
        let result = item
            .get("result")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .to_string();
        let mut meta = self.meta.clone();
        set_optional_string(&mut meta.output_format, item, "output_format");
        set_optional_string(&mut meta.size, item, "size");
        set_optional_string(&mut meta.background, item, "background");
        set_optional_string(&mut meta.quality, item, "quality");
        Some(ImageGenerationStreamResult {
            result,
            revised_prompt: optional_string(item, "revised_prompt"),
            meta,
        })
    }

    fn emit_pending_completed(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for result in pending {
            self.emit_completed(result);
        }
    }

    fn emit_pending_or_error(&mut self, message: &str) {
        if self.pending.is_empty() {
            self.emit_error(message);
            return;
        }
        self.emit_pending_completed();
    }

    fn emit_completed(&mut self, result: ImageGenerationStreamResult) {
        let key = format!(
            "{}|{}",
            result.meta.output_format.as_deref().unwrap_or_default(),
            result.result
        );
        if !self.seen.insert(key) {
            return;
        }
        let mut payload =
            self.image_payload("image_generation.completed", &result.result, &result.meta);
        if let Some(revised_prompt) = result.revised_prompt {
            payload.insert("revised_prompt".to_string(), Value::String(revised_prompt));
        }
        if let Some(usage) = self.usage.clone() {
            payload.insert("usage".to_string(), usage);
        }
        self.push_event("image_generation.completed", payload);
    }

    fn image_payload(
        &self,
        event_type: &str,
        result: &str,
        meta: &ImageGenerationStreamMeta,
    ) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("type".to_string(), Value::String(event_type.to_string()));
        payload.insert(
            "created_at".to_string(),
            json!(if meta.created_at > 0 {
                meta.created_at
            } else {
                now_unix_seconds()
            }),
        );
        payload.insert("b64_json".to_string(), Value::String(result.to_string()));
        if self
            .response_format
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("url"))
        {
            let mime_type = image_output_mime_type(meta.output_format.as_deref());
            payload.insert(
                "url".to_string(),
                Value::String(format!("data:{mime_type};base64,{result}")),
            );
        }
        insert_optional_string(&mut payload, "output_format", meta.output_format.as_deref());
        insert_optional_string(&mut payload, "background", meta.background.as_deref());
        insert_optional_string(&mut payload, "quality", meta.quality.as_deref());
        insert_optional_string(&mut payload, "size", meta.size.as_deref());
        insert_optional_string(&mut payload, "model", meta.model.as_deref());
        payload
    }

    fn emit_error(&mut self, message: &str) {
        self.out.push_back(image_generation_error_sse(message));
    }

    fn push_event(&mut self, event_name: &str, payload: Map<String, Value>) {
        self.out
            .push_back(image_generation_event_sse(event_name, payload));
    }
}

fn image_generation_error_sse(message: &str) -> Bytes {
    let payload = json!({
        "type": "error",
        "error": {
            "type": "upstream_error",
            "message": message
        }
    });
    let Value::Object(payload) = payload else {
        unreachable!("image generation error payload is always an object");
    };
    image_generation_event_sse("error", payload)
}

fn image_generation_event_sse(event_name: &str, payload: Map<String, Value>) -> Bytes {
    let body = serde_json::to_string(&Value::Object(payload)).unwrap_or_else(|_| {
        "{\"type\":\"error\",\"error\":{\"type\":\"proxy_error\",\"message\":\"failed to serialize image stream event\"}}".to_string()
    });
    Bytes::from(format!("event: {event_name}\ndata: {body}\n\n"))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn set_optional_string(target: &mut Option<String>, source: &Value, key: &str) {
    if let Some(value) = optional_string(source, key) {
        *target = Some(value);
    }
}

fn insert_optional_string(target: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    target.insert(key.to_string(), Value::String(value.to_string()));
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn stream_for_composed_transform(
    transform: FormatTransform,
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    _response_format: Option<&str>,
) -> ResponseStream {
    match transform {
        FormatTransform::ChatToAnthropic => {
            stream_chat_to_anthropic(upstream, context, log, request_tracker)
        }
        FormatTransform::AnthropicToChat => {
            stream_anthropic_to_chat(upstream, context, log, request_tracker)
        }
        FormatTransform::GeminiToAnthropic => {
            stream_gemini_to_anthropic(upstream, context, log, request_tracker)
        }
        FormatTransform::AnthropicToGemini => {
            stream_anthropic_to_gemini(upstream, context, log, request_tracker)
        }
        FormatTransform::ResponsesToGemini => {
            stream_responses_to_gemini(upstream, context, log, request_tracker)
        }
        FormatTransform::GeminiToResponses => {
            stream_gemini_to_responses(upstream, context, log, request_tracker)
        }
        FormatTransform::CodexToAnthropic => {
            stream_codex_to_anthropic(upstream, context, log, request_tracker)
        }
        _ => streaming::stream_with_logging(upstream, context, log, request_tracker).boxed(),
    }
}

fn stream_chat_to_anthropic(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let responses_stream = chat_to_responses::stream_chat_to_responses(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    responses_to_anthropic::stream_responses_to_anthropic(
        responses_stream,
        context,
        log,
        request_tracker,
    )
    .boxed()
}

fn stream_anthropic_to_chat(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let responses_stream = anthropic_to_responses::stream_anthropic_to_responses(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    responses_to_chat::stream_responses_to_chat(responses_stream, context, log, request_tracker)
        .boxed()
}

fn stream_codex_to_anthropic(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let responses_stream = codex_compat::stream_codex_to_responses(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    responses_to_anthropic::stream_responses_to_anthropic(
        responses_stream,
        context,
        log,
        request_tracker,
    )
    .boxed()
}

fn stream_gemini_to_anthropic(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let first_log = Arc::new(LogWriter::new(None));
    let first_tracker = RequestTokenTracker::disabled();
    let chat_stream =
        gemini_compat::stream_gemini_to_chat(upstream, context.clone(), first_log, first_tracker)
            .boxed();
    let second_log = Arc::new(LogWriter::new(None));
    let second_tracker = RequestTokenTracker::disabled();
    let responses_stream = chat_to_responses::stream_chat_to_responses(
        chat_stream,
        context.clone(),
        second_log,
        second_tracker,
    )
    .boxed();
    responses_to_anthropic::stream_responses_to_anthropic(
        responses_stream,
        context,
        log,
        request_tracker,
    )
    .boxed()
}

fn stream_anthropic_to_gemini(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let first_log = Arc::new(LogWriter::new(None));
    let first_tracker = RequestTokenTracker::disabled();
    let responses_stream = anthropic_to_responses::stream_anthropic_to_responses(
        upstream,
        context.clone(),
        first_log,
        first_tracker,
    )
    .boxed();
    let second_log = Arc::new(LogWriter::new(None));
    let second_tracker = RequestTokenTracker::disabled();
    let chat_stream = responses_to_chat::stream_responses_to_chat(
        responses_stream,
        context.clone(),
        second_log,
        second_tracker,
    )
    .boxed();
    gemini_compat::stream_chat_to_gemini(chat_stream, context, log, request_tracker).boxed()
}

fn stream_responses_to_gemini(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let chat_stream = responses_to_chat::stream_responses_to_chat(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    gemini_compat::stream_chat_to_gemini(chat_stream, context, log, request_tracker).boxed()
}

fn stream_gemini_to_responses(
    upstream: UpstreamBytesStream,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
) -> ResponseStream {
    let intermediate_log = Arc::new(LogWriter::new(None));
    let intermediate_tracker = RequestTokenTracker::disabled();
    let chat_stream = gemini_compat::stream_gemini_to_chat(
        upstream,
        context.clone(),
        intermediate_log,
        intermediate_tracker,
    )
    .boxed();
    chat_to_responses::stream_chat_to_responses(chat_stream, context, log, request_tracker).boxed()
}

async fn prepare_upstream_stream(
    status: StatusCode,
    headers: &HeaderMap,
    upstream_res: reqwest::Response,
    response_transform: FormatTransform,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    stream_first_output_timeout_remaining: Duration,
    stream_first_output_timeout: Duration,
    sync_response_timeout: Duration,
) -> Result<
    futures_util::stream::BoxStream<
        'static,
        Result<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>,
    >,
    Response,
> {
    match timeout(
        stream_first_output_timeout_remaining,
        prepare_upstream_stream_inner(
            status,
            headers,
            upstream_res,
            response_transform,
            context,
            log,
            sync_response_timeout,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(stream_first_output_timeout_response(
            context,
            log,
            stream_first_output_timeout,
        )),
    }
}

async fn prepare_upstream_stream_inner(
    status: StatusCode,
    headers: &HeaderMap,
    upstream_res: reqwest::Response,
    response_transform: FormatTransform,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    sync_response_timeout: Duration,
) -> Result<UpstreamBytesStream, Response> {
    let mut upstream =
        upstream_stream::with_idle_timeout(upstream_res.bytes_stream(), sync_response_timeout);
    let mut buffered_chunks: Vec<Bytes> = Vec::new();
    let mut responses_prelude = responses_prelude_inspector(response_transform, context);
    loop {
        match upstream.next().await {
            Some(Ok(chunk)) => {
                context.mark_upstream_first_byte();
                buffered_chunks.push(chunk);
                let latest = buffered_chunks.last().expect("buffered chunk just pushed");
                if let Some(inspector) = responses_prelude.as_mut() {
                    match inspector.inspect_chunk(latest) {
                        responses_error::ResponsesPreludeDecision::Pending => continue,
                        responses_error::ResponsesPreludeDecision::RetryableError(error) => {
                            return Err(responses_prelude_retry_response(
                                status,
                                headers,
                                response_transform,
                                error,
                                context,
                                log,
                            ));
                        }
                        responses_error::ResponsesPreludeDecision::ReadyForPassThrough => {}
                    }
                }
                return Ok(chain_buffered_chunks(buffered_chunks, upstream, context));
            }
            Some(Err(err)) => {
                return Err(stream_error_response(
                    err,
                    context,
                    log,
                    sync_response_timeout,
                ));
            }
            None => {
                if buffered_chunks.is_empty() {
                    return Err(http::build_response(status, headers.clone(), Body::empty()));
                }
                return Ok(chain_buffered_chunks(buffered_chunks, upstream, context));
            }
        }
    }
}

fn stream_first_output_timeout_response(
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    stream_first_output_timeout: Duration,
) -> Response {
    let timeout_secs = stream_first_output_timeout.as_secs();
    let message = format!("Upstream stream first output timed out after {timeout_secs}s.");
    tracing::warn!(
        provider = %context.provider,
        upstream = %context.upstream_id,
        account = ?context.account_id,
        path = %context.path,
        timeout_secs,
        "upstream stream first output timeout"
    );
    context.status = StatusCode::GATEWAY_TIMEOUT.as_u16();
    let empty_usage = UsageSnapshot::default();
    let entry = build_log_entry(context, empty_usage, Some(message.clone()));
    log.clone().write_detached(entry);
    let mut response = http::error_response(StatusCode::GATEWAY_TIMEOUT, &message);
    response.extensions_mut().insert(RetryableStreamResponse {
        status: StatusCode::GATEWAY_TIMEOUT,
        message,
        should_cooldown: true,
    });
    response
}

fn responses_prelude_inspector(
    response_transform: FormatTransform,
    context: &LogContext,
) -> Option<responses_error::ResponsesPreludeInspector> {
    let responses_upstream = matches!(
        response_transform,
        FormatTransform::ResponsesToChat
            | FormatTransform::ResponsesToAnthropic
            | FormatTransform::ResponsesToGemini
            | FormatTransform::CodexToChat
            | FormatTransform::CodexToResponses
            | FormatTransform::CodexToImagesGenerations
            | FormatTransform::CodexToAnthropic
    ) || response_transform == FormatTransform::None
        && is_openai_responses_stream_path(&context.path)
        && matches!(
            context.provider.as_str(),
            PROVIDER_OPENAI | PROVIDER_OPENAI_RESPONSES | PROVIDER_CODEX | PROVIDER_XAI
        );

    // 只缓冲 Responses 生命周期 prelude；首个业务事件放行后，后续错误绝不再拼接第二条流。
    responses_upstream.then(responses_error::ResponsesPreludeInspector::new)
}

fn responses_prelude_retry_response(
    status: StatusCode,
    headers: &HeaderMap,
    response_transform: FormatTransform,
    error: responses_error::ResponsesStreamError,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
) -> Response {
    context.mark_upstream_first_byte();
    let is_request_policy_rejection = context.provider == PROVIDER_XAI
        && classify_responses_error_scope(&error) == XaiForbiddenScope::RequestContentPolicy;
    if is_request_policy_rejection {
        context.status = StatusCode::FORBIDDEN.as_u16();
        // 策略错误可能对应敏感 prompt/media；请求日志不持久化该正文。
        context.request_body = None;
        let error_code = responses_error_safe_code(&error);
        tracing::warn!(
            provider = %context.provider,
            upstream = %context.upstream_id,
            account = ?context.account_id,
            path = %context.path,
            status = StatusCode::FORBIDDEN.as_u16(),
            error_type = %error.error_type,
            error_code = ?error_code,
            forbidden_scope = ?XaiForbiddenScope::RequestContentPolicy,
            "xAI request rejected by content policy before first business output"
        );
        let log_message = "xAI request rejected by content policy.".to_string();
        let empty_usage = UsageSnapshot::default();
        let entry = build_log_entry(context, empty_usage, Some(log_message));
        log.clone().write_detached(entry);

        let mut response = http::error_response(StatusCode::FORBIDDEN, error.display_message());
        response
            .extensions_mut()
            .insert(NonRetryableSemanticResponse);
        return response;
    }
    context.status = error.status.as_u16();
    let message = error.display_message();
    tracing::warn!(
        provider = %context.provider,
        upstream = %context.upstream_id,
        account = ?context.account_id,
        path = %context.path,
        status = error.status.as_u16(),
        error_type = %error.error_type,
        error_code = ?error.code,
        "retryable Responses stream error before first business output"
    );
    let empty_usage = UsageSnapshot::default();
    let entry = build_log_entry(context, empty_usage, Some(message.clone()));
    log.clone().write_detached(entry);

    let body = Body::from(responses_prelude_error_body(response_transform, &error));
    let mut response = http::build_response(status, headers.clone(), body);
    // capacity 类错误不触发跨请求冷却，但仍可原地/跨上游重试。
    let is_capacity = buffered::is_capacity_retry_error(&message, &message);
    let cooldown_hint = buffered::xai_account_cooldown_hint(
        &context.provider,
        error.status,
        &Bytes::from(message.clone()),
    );
    response.extensions_mut().insert(RetryableStreamResponse {
        status: error.status,
        message,
        should_cooldown: !is_capacity,
    });
    if let Some(hint) = cooldown_hint {
        response.extensions_mut().insert(hint);
    }
    response
}

fn classify_responses_error_scope(
    error: &responses_error::ResponsesStreamError,
) -> XaiForbiddenScope {
    // 复用 HTTP 分类器的结构化字段规则，避免 SSE 与普通响应产生不同的账号语义。
    let payload = responses_error_payload(error);
    classify_payload(payload.as_bytes())
}

fn responses_error_safe_code(error: &responses_error::ResponsesStreamError) -> Option<String> {
    payload_error_code(responses_error_payload(error).as_bytes())
}

fn responses_error_payload(error: &responses_error::ResponsesStreamError) -> String {
    Value::Object(error.openai_error_object()).to_string()
}

fn responses_prelude_error_body(
    response_transform: FormatTransform,
    error: &responses_error::ResponsesStreamError,
) -> Bytes {
    // 重试耗尽后该响应会直接返回客户端，必须按入站协议编码而不是沿用 Responses SSE。
    let message = error.display_message();
    match response_transform {
        FormatTransform::ResponsesToChat | FormatTransform::CodexToChat => {
            sse_with_done(codex_compat::stream_chat_error_sse(&message))
        }
        FormatTransform::ResponsesToAnthropic | FormatTransform::CodexToAnthropic => {
            responses_to_anthropic::anthropic_error_sse(error)
        }
        FormatTransform::ResponsesToGemini => {
            gemini_compat::gemini_error_sse(error.status, &error.message)
        }
        FormatTransform::CodexToImagesGenerations => image_generation_error_sse(&error.message),
        FormatTransform::None | FormatTransform::CodexToResponses => {
            sse_with_done(codex_compat::stream_responses_error_sse(&message, 0))
        }
        _ => unreachable!("Responses prelude inspector only accepts Responses upstreams"),
    }
}

fn sse_with_done(event: Bytes) -> Bytes {
    Bytes::from([event.as_ref(), b"data: [DONE]\n\n"].concat())
}

#[cfg(test)]
#[path = "stream.test.rs"]
mod policy_tests;

fn chain_buffered_chunks(
    chunks: Vec<Bytes>,
    upstream: UpstreamBytesStream,
    context: &mut LogContext,
) -> UpstreamBytesStream {
    context.mark_upstream_first_byte();
    futures_util::stream::iter(
        chunks
            .into_iter()
            .map(Ok::<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>),
    )
    .chain(upstream)
    .boxed()
}

fn stream_error_response(
    err: upstream_stream::UpstreamStreamError<reqwest::Error>,
    context: &mut LogContext,
    log: &Arc<LogWriter>,
    sync_response_timeout: Duration,
) -> Response {
    let (status, message) = match err {
        upstream_stream::UpstreamStreamError::IdleTimeout(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            format!(
                "Upstream response timed out after {}s.",
                sync_response_timeout.as_secs()
            ),
        ),
        upstream_stream::UpstreamStreamError::Upstream(err) => {
            let raw = err.to_string();
            let message = if context.provider == PROVIDER_GEMINI {
                redact_query_param_value(&raw, "key")
            } else {
                raw
            };
            (
                StatusCode::BAD_GATEWAY,
                format!("Failed to read upstream response: {message}"),
            )
        }
    };

    tracing::warn!(
        provider = %context.provider,
        upstream = %context.upstream_id,
        account = ?context.account_id,
        path = %context.path,
        status = status.as_u16(),
        error = %message,
        "upstream stream read failed before first business output"
    );
    context.status = status.as_u16();
    let empty_usage = UsageSnapshot::default();
    let entry = build_log_entry(context, empty_usage, Some(message.clone()));
    log.clone().write_detached(entry);
    let mut response = http::error_response(status, &message);
    response.extensions_mut().insert(RetryableStreamResponse {
        status,
        message,
        should_cooldown: true,
    });
    response
}

fn log_upstream_stream_if_debug(upstream: UpstreamBytesStream) -> UpstreamBytesStream {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return upstream;
    }
    upstream
        .map(|item| {
            if let Ok(chunk) = &item {
                let text = String::from_utf8_lossy(chunk);
                tracing::debug!(
                    stage = "upstream.response.chunk",
                    bytes = chunk.len(),
                    body = %text,
                    "debug dump"
                );
            } else if let Err(err) = &item {
                tracing::debug!(stage = "upstream.response.chunk.error", error = %err, "debug dump");
            }
            item
        })
        .boxed()
}

fn log_response_stream_if_debug(stream: ResponseStream) -> ResponseStream {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return stream;
    }
    stream
        .map(|item| {
            if let Ok(chunk) = &item {
                let text = String::from_utf8_lossy(chunk);
                tracing::debug!(
                    stage = "outbound.response.chunk",
                    bytes = chunk.len(),
                    body = %text,
                    "debug dump"
                );
            } else if let Err(err) = &item {
                tracing::debug!(stage = "outbound.response.chunk.error", error = %err, "debug dump");
            }
            item
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::openai_compat::CHAT_PATH;
    use crate::proxy::response::AccountCooldownHint;
    use axum::body::to_bytes;
    use axum::http::HeaderMap;
    use futures_util::stream;
    use std::io;
    use tokio::time::sleep;

    fn test_context() -> LogContext {
        LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: PROVIDER_CODEX.to_string(),
            upstream_id: "codex-test".to_string(),
            account_id: Some("codex-a.json".to_string()),
            model: Some("gpt-5.5".to_string()),
            mapped_model: None,
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: std::time::Instant::now(),
        }
    }

    fn reqwest_response_from_delayed_chunks(
        chunks: Vec<(Duration, &'static str)>,
    ) -> reqwest::Response {
        let stream = stream::unfold((0usize, chunks), |(index, chunks)| async move {
            let (delay, chunk) = chunks.get(index)?;
            sleep(*delay).await;
            Some((
                Ok::<Bytes, io::Error>(Bytes::from_static(chunk.as_bytes())),
                (index + 1, chunks),
            ))
        });
        let body = reqwest::Body::wrap_stream(stream);
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .expect("http response")
            .into()
    }

    fn reqwest_response_from_items(items: Vec<Result<Bytes, io::Error>>) -> reqwest::Response {
        let body = reqwest::Body::wrap_stream(stream::iter(items));
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .expect("http response")
            .into()
    }

    #[tokio::test]
    async fn xai_client_tool_stream_wrapper_restores_lifecycle() {
        let mut request = json!({"tools": [{"type": "custom", "name": "exec"}]});
        let (mapping, _) = token_proxy_protocol::xai_client_tools::adapt_request(
            request.as_object_mut().expect("request object"),
        )
        .expect("adapt request");
        let upstream = stream::iter(vec![Ok::<_, upstream_stream::UpstreamStreamError<
            reqwest::Error,
        >>(Bytes::from_static(
            b"data: {\"type\":\"response.output_item.added\",\"sequence_number\":4,\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"i1\",\"call_id\":\"c1\",\"name\":\"exec\",\"arguments\":\"\"}}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"sequence_number\":5,\"output_index\":0,\"item_id\":\"i1\",\"delta\":\"{\\\"input\\\":\\\"pwd\\\"}\"}\n\ndata: {\"type\":\"response.function_call_arguments.done\",\"sequence_number\":6,\"output_index\":0,\"item_id\":\"i1\",\"call_id\":\"c1\",\"name\":\"exec\",\"arguments\":\"{\\\"input\\\":\\\"pwd\\\"}\"}\n\ndata: [DONE]\n\n",
        ))])
        .boxed();

        let chunks = restore_xai_client_tool_stream(upstream, mapping, true)
            .collect::<Vec<_>>()
            .await;
        let body = chunks
            .into_iter()
            .map(|chunk| chunk.expect("stream chunk"))
            .fold(Vec::new(), |mut body, chunk| {
                body.extend_from_slice(&chunk);
                body
            });
        let body = String::from_utf8(body).expect("UTF-8 SSE");

        assert!(body.contains("custom_tool_call"));
        assert!(body.contains("response.custom_tool_call_input.done"));
        assert!(body.contains("\"sequence_number\":5"));
        assert!(body.contains("data: [DONE]"));
        assert!(!body.contains("response.function_call_arguments.delta"));
    }

    #[tokio::test]
    async fn stream_first_output_timeout_returns_retryable_response() {
        let upstream_res = reqwest_response_from_delayed_chunks(vec![(
            Duration::from_millis(80),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        )]);
        let mut context = test_context();
        context.provider = PROVIDER_OPENAI.to_string();
        context.upstream_id = "openai-slow".to_string();
        context.account_id = Some("acct-1".to_string());
        context.path = CHAT_PATH.to_string();
        let log = Arc::new(LogWriter::new(None));

        let response = match prepare_upstream_stream(
            StatusCode::OK,
            &HeaderMap::new(),
            upstream_res,
            FormatTransform::None,
            &mut context,
            &log,
            Duration::from_millis(20),
            Duration::from_millis(20),
            Duration::from_secs(30),
        )
        .await
        {
            Ok(_) => panic!("first output timeout should return retryable response"),
            Err(response) => response,
        };

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(response
            .extensions()
            .get::<RetryableStreamResponse>()
            .is_some());
    }

    #[tokio::test]
    async fn first_transport_error_returns_retryable_response() {
        let upstream_res = reqwest_response_from_items(vec![Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "connection reset by peer",
        ))]);
        let mut context = test_context();
        context.provider = PROVIDER_OPENAI_RESPONSES.to_string();
        let log = Arc::new(LogWriter::new(None));

        let response = match prepare_upstream_stream(
            StatusCode::OK,
            &HeaderMap::new(),
            upstream_res,
            FormatTransform::None,
            &mut context,
            &log,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .await
        {
            Ok(_) => panic!("first body read error should trigger failover"),
            Err(response) => response,
        };

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(response
            .extensions()
            .get::<RetryableStreamResponse>()
            .is_some());
    }

    #[tokio::test]
    async fn native_responses_prelude_retries_unknown_error_before_output() {
        let upstream_res = reqwest_response_from_delayed_chunks(vec![(
            Duration::ZERO,
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"server_error\",\"code\":\"unexpected\",\"message\":\"try later\"}}}\n\n",
        )]);
        let mut context = test_context();
        context.provider = PROVIDER_OPENAI_RESPONSES.to_string();
        let log = Arc::new(LogWriter::new(None));

        let response = match prepare_upstream_stream(
            StatusCode::OK,
            &HeaderMap::new(),
            upstream_res,
            FormatTransform::None,
            &mut context,
            &log,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .await
        {
            Ok(_) => panic!("unknown pre-output error should trigger failover"),
            Err(response) => response,
        };

        let retry = response
            .extensions()
            .get::<RetryableStreamResponse>()
            .expect("retry marker");
        assert!(retry.message.contains("unexpected"));

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read retry response body");
        let body = String::from_utf8(body.to_vec()).expect("SSE body");
        assert!(
            body.contains("\"sequence_number\":0"),
            "prelude terminal event must start the Responses sequence: {body}"
        );
    }

    #[tokio::test]
    async fn xai_responses_prelude_exposes_unauthorized_semantic_status() {
        let upstream_res = reqwest_response_from_delayed_chunks(vec![(
            Duration::ZERO,
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"authentication_error\",\"code\":\"unauthorized\",\"message\":\"token expired\"}}}\n\n",
        )]);
        let mut context = test_context();
        context.provider = PROVIDER_XAI.to_string();
        context.upstream_id = "xai-test".to_string();
        context.account_id = Some("xai-user".to_string());
        let log = Arc::new(LogWriter::new(None));

        let response = match prepare_upstream_stream(
            StatusCode::OK,
            &HeaderMap::new(),
            upstream_res,
            FormatTransform::None,
            &mut context,
            &log,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .await
        {
            Ok(_) => panic!("xAI pre-output error should trigger account failover"),
            Err(response) => response,
        };

        let retry = response
            .extensions()
            .get::<RetryableStreamResponse>()
            .expect("retry marker");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(retry.status, StatusCode::UNAUTHORIZED);
        assert!(retry.message.contains("unauthorized"));
    }

    #[tokio::test]
    async fn xai_responses_prelude_free_usage_shapes_mark_429_and_24_hour_cooldown() {
        for (shape, event) in [
            (
                "top-level status",
                "data: {\"type\":\"error\",\"status\":429,\"error\":{\"code\":\"subscription:free-usage-exhausted\",\"message\":\"You've used all the included free usage for now.\"}}\n\n",
            ),
            (
                "nested invalid request",
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"invalid_request_error\",\"code\":\"subscription:free-usage-exhausted\",\"message\":\"You've used all the included free usage for now.\"}}}\n\n",
            ),
        ] {
            let upstream_res =
                reqwest_response_from_delayed_chunks(vec![(Duration::ZERO, event)]);
            let mut context = test_context();
            context.provider = PROVIDER_XAI.to_string();
            context.upstream_id = "xai-test".to_string();
            context.account_id = Some("xai-user".to_string());
            let log = Arc::new(LogWriter::new(None));

            let response = match prepare_upstream_stream(
                StatusCode::OK,
                &HeaderMap::new(),
                upstream_res,
                FormatTransform::None,
                &mut context,
                &log,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(30),
            )
            .await
            {
                Ok(_) => panic!("xAI free usage error should trigger account failover: {shape}"),
                Err(response) => response,
            };

            let retry = response
                .extensions()
                .get::<RetryableStreamResponse>()
                .expect("retry marker");
            let cooldown = response
                .extensions()
                .get::<AccountCooldownHint>()
                .expect("free usage cooldown marker");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(retry.status, StatusCode::TOO_MANY_REQUESTS);
            assert!(retry.should_cooldown);
            assert_eq!(cooldown.duration, Duration::from_secs(24 * 60 * 60));
            assert_eq!(cooldown.reason, "free_usage_exhausted");
        }
    }

    #[tokio::test]
    async fn native_responses_prelude_passes_invalid_request_without_retry() {
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"invalid_request_error\",\"code\":\"context_length_exceeded\",\"message\":\"context window exceeded\"}}}\n\n";
        let upstream_res = reqwest_response_from_delayed_chunks(vec![(Duration::ZERO, failed)]);
        let mut context = test_context();
        context.provider = PROVIDER_OPENAI_RESPONSES.to_string();
        let log = Arc::new(LogWriter::new(None));

        let mut stream = prepare_upstream_stream(
            StatusCode::OK,
            &HeaderMap::new(),
            upstream_res,
            FormatTransform::None,
            &mut context,
            &log,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .await
        .expect("invalid request must stay on the current response");

        let chunk = stream
            .next()
            .await
            .expect("buffered failed event")
            .expect("valid chunk");
        assert_eq!(chunk.as_ref(), failed.as_bytes());
    }

    #[tokio::test]
    async fn responses_error_after_business_output_is_never_retried() {
        let upstream_res = reqwest_response_from_delayed_chunks(vec![
            (
                Duration::ZERO,
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            ),
            (
                Duration::ZERO,
                "data: {\"type\":\"error\",\"error\":{\"code\":\"server_overloaded\",\"message\":\"busy\"}}\n\n",
            ),
        ]);
        let mut context = test_context();
        context.provider = PROVIDER_OPENAI_RESPONSES.to_string();
        let log = Arc::new(LogWriter::new(None));

        let stream = prepare_upstream_stream(
            StatusCode::OK,
            &HeaderMap::new(),
            upstream_res,
            FormatTransform::None,
            &mut context,
            &log,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .await
        .expect("first business output commits the stream");
        let chunks = stream.collect::<Vec<_>>().await;
        let body = chunks
            .into_iter()
            .map(|chunk| chunk.expect("valid current stream chunk"))
            .fold(Vec::new(), |mut body, chunk| {
                body.extend_from_slice(&chunk);
                body
            });
        let body = String::from_utf8(body).expect("SSE text");

        assert!(body.contains("partial"));
        assert!(body.contains("server_overloaded"));
    }

    #[tokio::test]
    async fn codex_prelude_waits_until_first_business_output() {
        let upstream_res = reqwest_response_from_delayed_chunks(vec![
            (
                Duration::ZERO,
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
            ),
            (
                Duration::from_millis(200),
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            ),
        ]);
        let mut context = test_context();
        let log = Arc::new(LogWriter::new(None));

        let prepared = tokio::time::timeout(
            Duration::from_millis(300),
            prepare_upstream_stream(
                StatusCode::OK,
                &HeaderMap::new(),
                upstream_res,
                FormatTransform::CodexToResponses,
                &mut context,
                &log,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(30),
            ),
        )
        .await;

        assert!(
            prepared.is_ok(),
            "Codex prelude should release once first business output is visible"
        );
    }

    #[tokio::test]
    async fn openai_chat_passthrough_heartbeats_do_not_use_responses_semantic_timeout() {
        let upstream = stream::unfold(0usize, |index| async move {
            if index == 0 {
                return Some((
                    Ok::<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>(
                        Bytes::from_static(
                            b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
                        ),
                    ),
                    1,
                ));
            }
            sleep(Duration::from_millis(10)).await;
            Some((
                Ok::<Bytes, upstream_stream::UpstreamStreamError<reqwest::Error>>(
                    Bytes::from_static(b":\n\n"),
                ),
                index + 1,
            ))
        })
        .boxed();
        let mut context = test_context();
        context.path = CHAT_PATH.to_string();
        context.provider = PROVIDER_OPENAI.to_string();
        let log = Arc::new(LogWriter::new(None));
        let request_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let mut stream = stream_for_basic_transform(
            FormatTransform::None,
            upstream,
            context,
            log,
            request_tracker,
            None,
            Duration::from_millis(35),
        );

        for index in 0..5 {
            match tokio::time::timeout(Duration::from_millis(120), stream.next())
                .await
                .expect("chat passthrough should keep yielding heartbeat chunks")
            {
                Some(Ok(_)) => {}
                Some(Err(err)) => {
                    panic!("chat passthrough chunk {index} should not fail: {err}");
                }
                None => panic!("chat passthrough ended before chunk {index}"),
            }
        }
    }
}

fn stream_with_optional_model_override<E>(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    model_override: Option<&str>,
    semantic_timeout: Option<Duration>,
) -> futures_util::stream::BoxStream<'static, Result<Bytes, std::io::Error>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    if let Some(model_override) = model_override {
        if should_rewrite_sse_model(&context.provider) {
            return streaming::stream_with_logging_and_model_override_semantic_timeout(
                upstream,
                context,
                log,
                model_override.to_string(),
                request_tracker,
                semantic_timeout,
            )
            .boxed();
        }
    }
    streaming::stream_with_logging_and_semantic_timeout(
        upstream,
        context,
        log,
        request_tracker,
        semantic_timeout,
    )
    .boxed()
}

// 只对 data-only SSE 的提供商做行级重写，避免破坏带 event: 行的流。
fn should_rewrite_sse_model(provider: &str) -> bool {
    provider == PROVIDER_OPENAI
        || provider == PROVIDER_OPENAI_RESPONSES
        || provider == PROVIDER_GEMINI
        || provider == PROVIDER_CODEX
        || provider == PROVIDER_XAI
}

fn openai_semantic_timeout(provider: &str, path: &str, timeout: Duration) -> Option<Duration> {
    if is_openai_responses_stream_path(path)
        && (provider == PROVIDER_OPENAI
            || provider == PROVIDER_OPENAI_RESPONSES
            || provider == PROVIDER_CODEX
            || provider == PROVIDER_XAI)
    {
        Some(timeout)
    } else {
        None
    }
}

fn is_openai_responses_stream_path(path: &str) -> bool {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    path == "/v1/responses" || path == "/v1/responses/compact" || path.starts_with("/v1/responses/")
}
