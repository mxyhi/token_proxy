use super::buffered::{
    buffer_event_stream_response, build_buffered_response, empty_chat_completion_retry_message,
    response_error_for_status, value_is_absent,
};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::response::{NonRetryableSemanticResponse, RetryableStreamResponse};
use crate::proxy::{
    log::{LogContext, LogWriter},
    token_rate::TokenRateTracker,
};
use axum::{
    body::{to_bytes, Bytes},
    http::{
        header::{CONTENT_LENGTH, CONTENT_TYPE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::Response,
};
use futures_util::stream;
use serde_json::json;
use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::sleep;

fn test_context() -> LogContext {
    LogContext {
        client_ip: None,
        path: "/v1/chat/completions".to_string(),
        provider: "openai".to_string(),
        upstream_id: "airouter".to_string(),
        account_id: None,
        model: Some("gpt-5.4-mini".to_string()),
        mapped_model: None,
        stream: false,
        status: 200,
        upstream_request_id: None,
        request_headers: None,
        request_body: None,
        client_request_body: None,
        ttfb_ms: None,
        timings: Default::default(),
        start: Instant::now(),
    }
}

fn xai_client_tool_mapping() -> token_proxy_protocol::xai_client_tools::XaiClientToolMapping {
    let mut request = json!({"tools": [{"type": "custom", "name": "exec"}]});
    let (mapping, changed) = token_proxy_protocol::xai_client_tools::adapt_request(
        request.as_object_mut().expect("request object"),
    )
    .expect("adapt request");
    assert!(changed);
    mapping
}

async fn buffered_xai_forbidden(body: &'static str) -> Response {
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(CONTENT_TYPE, "application/json")
        .body(reqwest::Body::from(body))
        .expect("response")
        .into();
    let mut context = test_context();
    context.path = "/v1/responses".to_string();
    context.provider = "xai".to_string();
    context.status = StatusCode::FORBIDDEN.as_u16();
    let tracker = TokenRateTracker::new().register(None, None).await;

    build_buffered_response(
        StatusCode::FORBIDDEN,
        upstream_res,
        HeaderMap::new(),
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await
}

#[tokio::test]
async fn buffered_xai_request_policy_forbidden_is_non_retryable() {
    let response = buffered_xai_forbidden(
        r#"{"error":{"code":"new_sensitive","message":"image is sensitive"}}"#,
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response
        .extensions()
        .get::<NonRetryableSemanticResponse>()
        .is_some());
    assert!(response
        .extensions()
        .get::<RetryableStreamResponse>()
        .is_none());
}

#[tokio::test]
async fn buffered_xai_account_and_unknown_forbidden_keep_account_retry_semantics() {
    for body in [
        r#"{"error":{"code":"account_suspended","message":"account suspended"}}"#,
        r#"{"error":{"code":"policy_violation","message":"policy violation"}}"#,
    ] {
        let response = buffered_xai_forbidden(body).await;
        assert!(response
            .extensions()
            .get::<NonRetryableSemanticResponse>()
            .is_none());
    }
}

#[tokio::test]
async fn buffered_xai_json_restores_custom_tool_call() {
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(reqwest::Body::from(
            r#"{"id":"resp_1","output":[{"type":"function_call","id":"i1","call_id":"c1","name":"exec","arguments":"{\"input\":\"pwd\"}"}]}"#,
        ))
        .expect("response")
        .into();
    let mut context = test_context();
    context.path = "/v1/responses".to_string();
    context.provider = "xai".to_string();
    let tracker = TokenRateTracker::new().register(None, None).await;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        HeaderMap::new(),
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        Some(xai_client_tool_mapping()),
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(value["output"][0]["type"], "custom_tool_call");
    assert_eq!(value["output"][0]["input"], "pwd");
    assert!(value["output"][0].get("arguments").is_none());
}

#[tokio::test]
async fn buffered_xai_mislabeled_sse_restores_custom_tool_call() {
    let sse = [
        "data: {\"type\":\"response.created\",\"sequence_number\":0,\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"in_progress\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"sequence_number\":1,\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"id\":\"i1\",\"call_id\":\"c1\",\"name\":\"exec\",\"arguments\":\"{\\\"input\\\":\\\"pwd\\\"}\"}]}}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat();
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(reqwest::Body::from(sse))
        .expect("response")
        .into();
    let mut context = test_context();
    context.path = "/v1/responses".to_string();
    context.provider = "xai".to_string();
    let tracker = TokenRateTracker::new().register(None, None).await;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        HeaderMap::new(),
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        Some(xai_client_tool_mapping()),
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(value["output"][0]["type"], "custom_tool_call");
    assert_eq!(value["output"][0]["input"], "pwd");
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

#[test]
fn value_is_absent_accepts_null_empty_string_and_empty_array() {
    assert!(value_is_absent(None));
    assert!(value_is_absent(Some(&json!(null))));
    assert!(value_is_absent(Some(&json!(""))));
    assert!(value_is_absent(Some(&json!("   "))));
    assert!(value_is_absent(Some(&json!([]))));
    assert!(!value_is_absent(Some(&json!("ok"))));
    assert!(!value_is_absent(Some(
        &json!([{"type":"text","text":"ok"}])
    )));
}

#[test]
fn response_error_for_status_includes_status_and_body() {
    let body = Bytes::from_static(br#"{"error":{"message":"quota denied"}}"#);

    let error = response_error_for_status(StatusCode::BAD_GATEWAY, &body);

    assert_eq!(
        error.as_deref(),
        Some(r#"HTTP 502: {"error":{"message":"quota denied"}}"#)
    );
}

#[test]
fn response_error_for_status_keeps_status_when_body_is_empty() {
    let error = response_error_for_status(StatusCode::TOO_MANY_REQUESTS, &Bytes::new());

    assert_eq!(error.as_deref(), Some("HTTP 429"));
}

#[test]
fn response_error_for_status_ignores_success() {
    let body = Bytes::from_static(br#"{"id":"resp_ok"}"#);

    let error = response_error_for_status(StatusCode::OK, &body);

    assert_eq!(error, None);
}

#[tokio::test]
async fn buffered_responses_502_normalizes_complete_error_contract() {
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(CONTENT_TYPE, "application/json")
        .body(reqwest::Body::from(
            r#"{"error":{"message":"Upstream request failed"}}"#,
        ))
        .expect("response")
        .into();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("999"));
    let tracker = TokenRateTracker::new().register(None, None).await;
    let mut context = test_context();
    context.path = "/v1/responses".to_string();
    context.model = Some("grok-4.5-high".to_string());

    let response = build_buffered_response(
        StatusCode::BAD_GATEWAY,
        upstream_res,
        headers,
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("error JSON");

    assert_eq!(parts.status, StatusCode::BAD_GATEWAY);
    assert_eq!(parts.headers.get(CONTENT_TYPE).unwrap(), "application/json");
    assert!(parts.headers.get(CONTENT_LENGTH).is_none());
    assert_eq!(value["error"]["message"], "Upstream request failed");
    assert_eq!(value["error"]["type"], "server_error");
    assert_eq!(value["error"]["code"], "server_error");
    assert!(value["error"].get("param").is_some());
    assert!(value["error"]["param"].is_null());
}

#[tokio::test]
async fn buffered_responses_complete_error_preserves_body_and_content_length() {
    let body = r#"{ "error": { "type": "server_error", "message": "already complete", "param": null, "code": "upstream_error" } }"#;
    let expected_content_length = body.len().to_string();
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(CONTENT_TYPE, "application/json")
        .body(reqwest::Body::from(body))
        .expect("response")
        .into();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&expected_content_length).expect("content length"),
    );
    let tracker = TokenRateTracker::new().register(None, None).await;
    let mut context = test_context();
    context.path = "/v1/responses".to_string();

    let response = build_buffered_response(
        StatusCode::BAD_GATEWAY,
        upstream_res,
        headers,
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let (parts, response_body) = response.into_parts();
    let response_body = to_bytes(response_body, usize::MAX).await.expect("body");

    assert_eq!(parts.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        parts
            .headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some(expected_content_length.as_str())
    );
    assert_eq!(response_body.as_ref(), body.as_bytes());
}

#[tokio::test]
async fn xai_video_content_preserves_binary_body_and_content_type() {
    let binary = Bytes::from_static(&[0x00, 0x9f, 0xff, 0x01, 0x80]);
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "video/mp4")
        .header(CONTENT_LENGTH, binary.len())
        .body(reqwest::Body::from(binary.clone()))
        .expect("response")
        .into();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&binary.len().to_string()).expect("content length"),
    );
    let tracker = TokenRateTracker::new().register(None, None).await;
    let mut context = test_context();
    context.path = "/v1/videos/video-123/content".to_string();
    context.provider = "xai".to_string();
    context.model = None;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        headers,
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let (parts, response_body) = response.into_parts();
    let response_body = to_bytes(response_body, usize::MAX).await.expect("body");

    assert_eq!(parts.status, StatusCode::OK);
    assert_eq!(parts.headers.get(CONTENT_TYPE).unwrap(), "video/mp4");
    assert_eq!(response_body, binary);
}

#[test]
fn buffer_event_stream_response_converts_chat_completion_chunks_to_json() {
    let sse = Bytes::from(
        [
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":1770000000,\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello \"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":1770000000,\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat(),
    );

    let output = buffer_event_stream_response(&sse).expect("buffer SSE");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["object"], json!("chat.completion"));
    assert_eq!(value["model"], json!("gpt-5.5"));
    assert_eq!(value["choices"][0]["message"]["role"], json!("assistant"));
    assert_eq!(
        value["choices"][0]["message"]["content"],
        json!("hello world")
    );
    assert_eq!(value["choices"][0]["finish_reason"], json!("stop"));
}

#[test]
fn buffer_event_stream_response_converts_empty_choices_chunk_to_empty_stop() {
    let sse = Bytes::from(
        [
            "data: {\"id\":\"\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"gpt-5.5\",\"system_fingerprint\":\"\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":0,\"total_tokens\":12}}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat(),
    );

    let output = buffer_event_stream_response(&sse).expect("buffer SSE");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["object"], json!("chat.completion"));
    assert_eq!(value["model"], json!("gpt-5.5"));
    assert_eq!(value["choices"][0]["message"]["role"], json!("assistant"));
    assert_eq!(value["choices"][0]["message"]["content"], json!(""));
    assert_eq!(value["choices"][0]["finish_reason"], json!("stop"));
    assert_eq!(value["usage"]["total_tokens"], json!(12));
    assert_eq!(
        empty_chat_completion_retry_message(&output, &test_context(), FormatTransform::None)
            .as_deref(),
        Some("Upstream returned empty chat completion content for stop response.")
    );
}

#[test]
fn buffer_event_stream_response_returns_completed_responses_object() {
    let completed = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_1",
            "object": "response",
            "created_at": 1770000000_i64,
            "status": "completed",
            "model": "gpt-5.5",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "done" }
                    ]
                }
            ]
        }
    });
    let sse = Bytes::from(format!("data: {completed}\n\ndata: [DONE]\n\n"));

    let output = buffer_event_stream_response(&sse).expect("buffer SSE");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["object"], json!("response"));
    assert_eq!(value["id"], json!("resp_1"));
    assert_eq!(value["output"][0]["content"][0]["text"], json!("done"));
}

#[test]
fn buffer_event_stream_response_hydrates_missing_terminal_output_ids() {
    let sse = Bytes::from(concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"msg_buffered\",\"type\":\"message\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"fc_buffered\",\"type\":\"function_call\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"rs_unindexed\",\"type\":\"reasoning\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[{\"id\":null,\"type\":\"message\"},{\"id\":\"\",\"type\":\"function_call\"},{\"type\":\"reasoning\"},{\"id\":\"msg_existing\",\"type\":\"message\"}]}}\n\n",
        "data: [DONE]\n\n"
    ));

    let output = buffer_event_stream_response(&sse).expect("buffer SSE");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["output"][0]["id"], json!("msg_buffered"));
    assert_eq!(value["output"][1]["id"], json!("fc_buffered"));
    assert!(value["output"][2].get("id").is_none());
    assert_eq!(value["output"][3]["id"], json!("msg_existing"));
}

#[test]
fn buffer_event_stream_response_synthesizes_response_from_done_item() {
    let sse = Bytes::from(
        [
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"created_at\":1770000000,\"status\":\"in_progress\",\"model\":\"gpt-5.5\"}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"AgentOutput\",\"arguments\":\"{}\",\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat(),
    );

    let output = buffer_event_stream_response(&sse).expect("buffer SSE");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["object"], json!("response"));
    assert_eq!(value["status"], json!("completed"));
    assert_eq!(value["output"][0]["type"], json!("function_call"));
    assert_eq!(value["output"][0]["name"], json!("AgentOutput"));
}

#[test]
fn buffer_event_stream_response_restores_raw_compaction_items_when_terminal_output_is_absent() {
    let raw_item = json!({
        "type": "compaction",
        "id": "cmp_raw",
        "encrypted_content": "encrypted-raw",
        "opaque_payload": { "revision": 2 }
    });
    let summary_item = json!({
        "type": "compaction_summary",
        "id": "cmp_summary",
        "encrypted_content": "encrypted-summary",
        "summary": [{ "type": "summary_text", "text": "compact context" }]
    });

    for terminal_response in [
        json!({
            "id": "resp_compact",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.6-sol",
            "output": []
        }),
        json!({
            "id": "resp_compact",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.6-sol"
        }),
    ] {
        let sse = Bytes::from(format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": raw_item
            }),
            json!({
                "type": "response.output_item.done",
                "output_index": 1,
                "item": summary_item
            }),
            json!({ "type": "response.completed", "response": terminal_response })
        ));

        let output = buffer_event_stream_response(&sse).expect("buffer compact SSE");
        let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

        assert_eq!(value["output"], json!([raw_item, summary_item]));
    }
}

#[tokio::test]
async fn buffered_non_stream_event_stream_chat_completion_returns_json() {
    let sse = [
        "data: {\"id\":\"chatcmpl_1\",\"object\":\"chat.completion.chunk\",\"created\":1770000000,\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat();
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(reqwest::Body::from(sse))
        .expect("response")
        .into();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("999"));
    let tracker = TokenRateTracker::new().register(None, None).await;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        headers,
        test_context(),
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(parts.headers.get(CONTENT_TYPE).unwrap(), "application/json");
    assert!(parts.headers.get(CONTENT_LENGTH).is_none());
    assert_eq!(value["choices"][0]["message"]["content"], json!("ok"));
    assert!(!String::from_utf8_lossy(&body).contains("data:"));
}

#[tokio::test]
async fn buffered_non_stream_total_body_timeout_returns_retryable_504() {
    let upstream_res = reqwest_response_from_delayed_chunks(vec![
        (Duration::ZERO, "{\"id\":\"chatcmpl_1\","),
        (Duration::from_millis(80), "\"choices\":[]}"),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        HeaderMap::new(),
        test_context(),
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_millis(30),
    )
    .await;
    let retryable = response
        .extensions()
        .get::<RetryableStreamResponse>()
        .cloned();
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.expect("body");
    let text = String::from_utf8_lossy(&body);

    assert_eq!(parts.status, StatusCode::GATEWAY_TIMEOUT);
    let retryable = retryable.expect("timeout must be retryable");
    assert!(retryable.should_cooldown);
    assert!(text.contains("Upstream synchronous response timed out after 0s."));
}

#[tokio::test]
async fn buffered_non_stream_responses_event_stream_chat_request_returns_json() {
    let sse = [
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"created_at\":1770000000,\"status\":\"in_progress\",\"model\":\"gpt-5.5\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"AgentOutput\",\"arguments\":\"{}\",\"status\":\"completed\"}}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat();
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(reqwest::Body::from(sse))
        .expect("response")
        .into();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("999"));
    let tracker = TokenRateTracker::new().register(None, None).await;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        headers,
        test_context(),
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(parts.status, StatusCode::OK);
    assert_eq!(parts.headers.get(CONTENT_TYPE).unwrap(), "application/json");
    assert_eq!(value["object"], json!("chat.completion"));
    assert_eq!(value["choices"][0]["finish_reason"], json!("tool_calls"));
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        json!("AgentOutput")
    );
}

#[tokio::test]
async fn buffered_json_response_with_responses_sse_body_returns_json() {
    let sse = [
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_json_sse\",\"object\":\"response\",\"created_at\":1770000000,\"status\":\"in_progress\",\"model\":\"gpt-5.5\"}}\n\n",
        "data: {\"type\":\"response.output_text.done\",\"output_index\":0,\"text\":\"hello\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_json_sse\",\"object\":\"response\",\"created_at\":1770000000,\"status\":\"completed\",\"model\":\"gpt-5.5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"total_tokens\":12}}}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat();
    let upstream_res = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(reqwest::Body::from(sse))
        .expect("response")
        .into();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("999"));
    let tracker = TokenRateTracker::new().register(None, None).await;

    let response = build_buffered_response(
        StatusCode::OK,
        upstream_res,
        headers,
        test_context(),
        Arc::new(LogWriter::new(None)),
        tracker,
        FormatTransform::None,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await;
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

    assert_eq!(parts.status, StatusCode::OK);
    assert_eq!(parts.headers.get(CONTENT_TYPE).unwrap(), "application/json");
    assert!(parts.headers.get(CONTENT_LENGTH).is_none());
    assert_eq!(value["object"], json!("chat.completion"));
    assert_eq!(value["choices"][0]["message"]["content"], json!("hello"));
    assert_eq!(value["usage"]["prompt_tokens"], json!(10));
    assert!(!String::from_utf8_lossy(&body).contains("data:"));
}

#[test]
fn empty_chat_completion_retry_message_matches_null_stop_response() {
    let bytes = Bytes::from(
        json!({
            "id": "chatcmpl_bad",
            "object": "chat.completion",
            "created": 1775879402_i64,
            "model": "gpt-5.4-mini-2026-03-17",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "reasoning_content": null,
                        "tool_calls": null
                    },
                    "finish_reason": "stop"
                }
            ]
        })
        .to_string(),
    );

    let message =
        empty_chat_completion_retry_message(&bytes, &test_context(), FormatTransform::None);
    assert_eq!(
        message.as_deref(),
        Some("Upstream returned empty chat completion content for stop response.")
    );
}

#[test]
fn empty_chat_completion_retry_message_ignores_normal_text_response() {
    let bytes = Bytes::from(
        json!({
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "feat(server): support env port"
                    },
                    "finish_reason": "stop"
                }
            ]
        })
        .to_string(),
    );

    assert!(
        empty_chat_completion_retry_message(&bytes, &test_context(), FormatTransform::None)
            .is_none()
    );
}

#[test]
fn empty_chat_completion_retry_message_ignores_tool_calls_response() {
    let bytes = Bytes::from(
        json!({
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "foo",
                                    "arguments": "{}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })
        .to_string(),
    );

    assert!(
        empty_chat_completion_retry_message(&bytes, &test_context(), FormatTransform::None)
            .is_none()
    );
}

#[test]
fn empty_chat_completion_retry_message_applies_to_transformed_chat_output() {
    let bytes = Bytes::from(
        json!({
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null
                    },
                    "finish_reason": "stop"
                }
            ]
        })
        .to_string(),
    );
    let mut transformed_context = test_context();
    transformed_context.provider = "openai-response".to_string();
    assert_eq!(
        empty_chat_completion_retry_message(
            &bytes,
            &transformed_context,
            FormatTransform::ResponsesToChat
        )
        .as_deref(),
        Some("Upstream returned empty chat completion content for stop response.")
    );
}

#[test]
fn empty_chat_completion_retry_message_skips_non_chat_outputs() {
    let bytes = Bytes::from(
        json!({
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null
                    },
                    "finish_reason": "stop"
                }
            ]
        })
        .to_string(),
    );

    assert!(empty_chat_completion_retry_message(
        &bytes,
        &test_context(),
        FormatTransform::ChatToResponses
    )
    .is_none());
}

async fn buffered_codex_response(
    body: String,
    content_type: Option<&str>,
    transform: FormatTransform,
) -> Response {
    let mut upstream = axum::http::Response::builder().status(StatusCode::OK);
    if let Some(content_type) = content_type {
        upstream = upstream.header(CONTENT_TYPE, content_type);
    }
    let upstream = upstream
        .body(reqwest::Body::from(body))
        .expect("response")
        .into();
    let mut context = test_context();
    context.provider = "codex".into();
    let tracker = TokenRateTracker::new().register(None, None).await;
    build_buffered_response(
        StatusCode::OK,
        upstream,
        HeaderMap::new(),
        context,
        Arc::new(LogWriter::new(None)),
        tracker,
        transform,
        None,
        None,
        None,
        None,
        Duration::from_secs(1),
    )
    .await
}

fn codex_terminal_sse(status: &str, text: &str, reason: Option<&str>) -> String {
    format!(
        "event: response.{status}\ndata: {}\n\n",
        json!({
            "type": format!("response.{status}"), "response": {
                "id": "resp_codex", "object": "response", "status": status, "error": null,
                "model": "gpt-6-astra", "incomplete_details": {"reason": reason},
                "output": [{"type": "message", "role":"assistant", "content": [{"type": "output_text", "text": text}]}],
                "usage": {"input_tokens": 12, "output_tokens": if text.is_empty() { 0 } else { 3 }, "total_tokens": 15}
            }
        })
    )
}

#[tokio::test]
async fn buffered_codex_sse_ignores_content_type_and_null_error() {
    for content_type in [
        None,
        Some("application/json"),
        Some("text/plain"),
        Some("text/event-stream"),
    ] {
        let response = buffered_codex_response(
            codex_terminal_sse("completed", "hello", None),
            content_type,
            FormatTransform::CodexToChat,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{content_type:?}");
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["choices"][0]["message"]["content"], "hello");
        assert_eq!(value["usage"]["completion_tokens"], 3);
    }
}

#[tokio::test]
async fn buffered_codex_sse_requires_terminal_even_after_output() {
    for event in [
        json!({"type":"response.created", "response":{"id":"resp_1", "object":"response", "status":"in_progress"}}),
        json!({"type":"response.output_text.delta", "delta":"partial"}),
        json!({"type":"response.output_item.done", "item":{"type":"function_call", "name":"lookup", "arguments":"{}"}}),
    ] {
        for suffix in ["", "data: [DONE]\n\n"] {
            let sse = format!("data: {event}\n\n{suffix}");
            let response = buffered_codex_response(
                sse,
                Some("application/json"),
                FormatTransform::CodexToChat,
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let retry = response
                .extensions()
                .get::<RetryableStreamResponse>()
                .expect("retryable EOF");
            assert!(!retry.should_cooldown);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert!(String::from_utf8_lossy(&body).contains("before response.completed"));
        }
    }
}

#[tokio::test]
async fn buffered_codex_sse_preserves_failures_and_request_scope() {
    for (event, status, message) in [
        (
            json!({"type":"error", "message":"model is at capacity"}),
            StatusCode::SERVICE_UNAVAILABLE,
            "model is at capacity",
        ),
        (
            json!({"type":"response.failed", "response":{"error":{"code":"context_length_exceeded", "message":"context window exceeded"}}}),
            StatusCode::BAD_REQUEST,
            "context window exceeded",
        ),
        (
            json!({"type":"response.failed", "status":429, "response":{"error":{"message":"try later"}}}),
            StatusCode::TOO_MANY_REQUESTS,
            "try later",
        ),
        (
            json!({"type":"response.cancelled", "response":{"id":"resp_1", "object":"response", "status":"cancelled", "error":null}}),
            StatusCode::BAD_GATEWAY,
            "cancelled",
        ),
    ] {
        let response = buffered_codex_response(
            format!("data: {event}\n\n"),
            None,
            FormatTransform::CodexToResponses,
        )
        .await;
        assert_eq!(response.status(), status);
        if status == StatusCode::BAD_REQUEST {
            assert!(response
                .extensions()
                .get::<NonRetryableSemanticResponse>()
                .is_some());
        }
        if let Some(retry) = response.extensions().get::<RetryableStreamResponse>() {
            assert!(!retry.should_cooldown);
        }
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains(message));
    }
}

#[tokio::test]
async fn buffered_codex_sse_maps_incomplete_and_rejects_empty_output() {
    for (reason, finish_reason) in [
        ("max_tokens", "length"),
        ("max_output_tokens", "length"),
        ("content_filter", "content_filter"),
    ] {
        let response = buffered_codex_response(
            codex_terminal_sse("incomplete", "partial", Some(reason)),
            None,
            FormatTransform::CodexToChat,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["choices"][0]["finish_reason"], finish_reason);
    }
    let response = buffered_codex_response(
        codex_terminal_sse("incomplete", "", Some("max_output_tokens")),
        None,
        FormatTransform::CodexToResponses,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn buffered_codex_sse_hydrates_tools_and_usage_for_responses() {
    let item = json!({"id":"fc_1", "type":"function_call", "call_id":"call_1", "name":"lookup", "arguments":"{}"});
    let done = json!({"type":"response.output_item.done", "output_index":0, "item":item});
    let terminal = json!({"type":"response.completed", "response":{"id":"resp_1", "object":"response", "status":"completed", "error":null, "output":[], "usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15}}});
    // 心跳数量不应影响 Codex SSE 识别，尤其是响应头缺失时。
    let sse = format!(
        "{}data: {done}\n\ndata: {terminal}\n\n",
        ": keepalive\n\n".repeat(20)
    );
    let response = buffered_codex_response(sse, None, FormatTransform::CodexToResponses).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["output"], json!([item]));
    assert_eq!(value["usage"]["total_tokens"], 15);
}

#[tokio::test]
async fn buffered_codex_sse_shared_entry_supports_anthropic_and_images() {
    let response = buffered_codex_response(
        codex_terminal_sse("completed", "hello", None),
        None,
        FormatTransform::CodexToAnthropic,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["content"][0]["text"], "hello");
    let terminal = json!({"type":"response.completed", "response":{"id":"resp_image", "object":"response", "status":"completed", "error":null, "output":[{"type":"image_generation_call","result":"aW1hZ2U="}]}});
    let response = buffered_codex_response(
        format!("data: {terminal}\n\n"),
        Some("text/plain"),
        FormatTransform::CodexToImagesGenerations,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["data"][0]["b64_json"], "aW1hZ2U=");
}

#[tokio::test]
async fn buffered_codex_sse_rejects_malformed_payload_without_json_fallback() {
    let response = buffered_codex_response(
        "data: {invalid}\n\n".to_string(),
        Some("application/json"),
        FormatTransform::CodexToChat,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let message = String::from_utf8_lossy(&body);
    assert!(message.contains("Invalid event-stream JSON payload"));
    assert!(!message.contains("non-JSON success payload"));
}

#[tokio::test]
async fn buffered_codex_preserves_normal_json_compatibility() {
    let payload = json!({"id":"resp_json", "object":"response", "status":"completed", "error":null, "output":[{"type":"message", "content":[{"type":"output_text", "text":"hello"}]}]});
    let response = buffered_codex_response(
        payload.to_string(),
        Some("application/json"),
        FormatTransform::CodexToChat,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["choices"][0]["message"]["content"], "hello");
}

fn responses_text_recovery_sse(output: serde_json::Value, status: &str) -> String {
    let events = [
        json!({"type":"response.output_item.added", "output_index":0, "item":{"id":"msg_a", "type":"message", "role":"assistant", "content":[]}}),
        json!({"type":"response.output_text.delta", "item_id":"msg_a", "output_index":0, "content_index":0, "delta":"hello"}),
        json!({"type":"response.output_text.delta", "item_id":"msg_a", "output_index":0, "content_index":0, "delta":" world"}),
        json!({"type":"response.output_text.done", "item_id":"msg_a", "output_index":0, "content_index":0, "text":""}),
        json!({"type":"response.output_item.done", "output_index":0, "item":{"id":"msg_a", "type":"message", "role":"assistant", "content":[]}}),
        json!({"type":format!("response.{status}"), "response":{"id":"resp_recovery", "object":"response", "model":"gpt-6-astra", "status":status, "output":output, "incomplete_details":{"reason":"max_output_tokens"}, "usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15}}}),
    ];
    events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect()
}

#[test]
fn buffered_responses_text_recovery_preserves_terminal_fields() {
    for content in [
        json!([]),
        json!([{"type":"output_text", "text":"", "annotations":[]}]),
    ] {
        let output = json!([{"id":"msg_a", "type":"message", "role":"assistant", "status":"incomplete", "content":content}]);
        let sse = responses_text_recovery_sse(output, "incomplete");
        let body = buffer_event_stream_response(&Bytes::from(sse)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["output"][0]["content"][0]["text"], "hello world");
        assert_eq!(value["output"][0]["status"], "incomplete");
        assert_eq!(value["status"], "incomplete");
        assert_eq!(value["incomplete_details"]["reason"], "max_output_tokens");
        assert_eq!(value["usage"]["total_tokens"], 15);
    }
}

#[test]
fn buffered_responses_text_recovery_preserves_final_text_and_refusal() {
    for content in [
        json!([{"type":"output_text", "text":"final authoritative text", "annotations":[{"type":"citation"}]}]),
        json!([{"type":"refusal", "refusal":"cannot comply"}]),
    ] {
        let output =
            json!([{"id":"msg_a", "type":"message", "role":"assistant", "content":content}]);
        let body = buffer_event_stream_response(&Bytes::from(responses_text_recovery_sse(
            output.clone(),
            "completed",
        )))
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["output"], output);
    }
}

#[test]
fn buffered_responses_text_recovery_aligns_items_and_content() {
    let tool = json!({"type":"function_call", "id":"fc_1", "call_id":"call_1", "name":"lookup", "arguments":"{}"});
    let events = [
        json!({"type":"response.output_item.added", "output_index":0, "item":{"type":"message", "id":"msg_a", "content":[]}}),
        json!({"type":"response.output_item.done", "output_index":1, "item":tool}),
        json!({"type":"response.output_item.added", "output_index":2, "item":{"type":"message", "id":"msg_b", "content":[]}}),
        json!({"type":"response.output_text.delta", "item_id":"msg_a", "output_index":0, "content_index":0, "delta":"first"}),
        json!({"type":"response.output_text.delta", "item_id":"msg_b", "output_index":2, "content_index":0, "delta":"second"}),
        json!({"type":"response.output_text.delta", "item_id":"msg_b", "content_index":1, "delta":"another part"}),
        json!({"type":"response.output_item.done", "output_index":2, "item":{"type":"message", "id":"msg_b", "content":[]}}),
        // IDs take precedence over final array positions; never fill unrelated messages.
        json!({"type":"response.completed", "response":{"status":"completed", "output":[
            {"type":"message", "id":"msg_b", "content":[{"type":"output_text", "text":""}, {"type":"output_text", "text":""}]},
            tool,
            {"type":"message", "id":"msg_a", "content":[]},
            {"type":"message", "id":"msg_unrelated", "content":[]}
        ]}}),
    ];
    let sse: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    let body = buffer_event_stream_response(&Bytes::from(sse)).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["output"][0]["content"][0]["text"], "second");
    assert_eq!(value["output"][0]["content"][1]["text"], "another part");
    assert_eq!(value["output"][1], tool);
    assert_eq!(value["output"][2]["content"][0]["text"], "first");
    assert_eq!(value["output"][3]["content"], json!([]));
}

#[tokio::test]
async fn buffered_responses_text_recovery_reaches_all_codex_formats() {
    for (transform, pointer) in [
        (
            FormatTransform::CodexToResponses,
            "/output/0/content/0/text",
        ),
        (FormatTransform::CodexToChat, "/choices/0/message/content"),
        (FormatTransform::CodexToAnthropic, "/content/0/text"),
    ] {
        let output = json!([{"id":"msg_a", "type":"message", "role":"assistant", "content":[]}]);
        let response = buffered_codex_response(
            responses_text_recovery_sse(output, "incomplete"),
            None,
            transform,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.pointer(pointer), Some(&json!("hello world")));
        match transform {
            FormatTransform::CodexToResponses => assert_eq!(value["status"], "incomplete"),
            FormatTransform::CodexToChat => {
                assert_eq!(value["choices"][0]["finish_reason"], "length")
            }
            FormatTransform::CodexToAnthropic => assert_eq!(value["stop_reason"], "max_tokens"),
            _ => unreachable!(),
        }
    }
}

#[test]
fn buffered_responses_text_recovery_does_not_reuse_ids_or_guess_sparse_content() {
    let events = [
        json!({"type":"response.output_text.delta", "output_index":0, "content_index":0, "delta":"first"}),
        json!({"type":"response.output_item.added", "output_index":0, "item":{"type":"message", "id":"msg_old", "content":[]}}),
        json!({"type":"response.output_item.added", "output_index":0, "item":{"type":"message", "id":"msg_new", "content":[]}}),
        json!({"type":"response.output_text.delta", "output_index":0, "content_index":0, "delta":"second"}),
        json!({"type":"response.output_text.delta", "item_id":"msg_sparse", "output_index":2, "content_index":2, "delta":"must not shift"}),
        json!({"type":"response.completed", "response":{"status":"completed", "output":[
            {"type":"message", "id":"msg_new", "content":[]},
            {"type":"message", "id":"msg_old", "content":[]},
            {"type":"message", "id":"msg_sparse", "content":[{"type":"refusal", "refusal":"no"}]}
        ]}}),
    ];
    let sse: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    let body = buffer_event_stream_response(&Bytes::from(sse)).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["output"][0]["content"][0]["text"], "second");
    assert_eq!(value["output"][1]["content"][0]["text"], "first");
    assert_eq!(
        value["output"][2]["content"],
        json!([{"type":"refusal", "refusal":"no"}])
    );
}

#[test]
fn buffered_responses_text_recovery_ignores_ambiguous_anonymous_delta() {
    for items in [
        json!([{"type":"function_call", "id":"fc_1", "name":"lookup", "arguments":"{}"}]),
        json!([{"type":"message", "id":"msg_a", "content":[]}, {"type":"message", "id":"msg_b", "content":[]}]),
    ] {
        let mut sse = String::new();
        for (index, item) in items.as_array().unwrap().iter().enumerate() {
            let event =
                json!({"type":"response.output_item.added", "output_index":index, "item":item});
            sse.push_str(&format!("data: {event}\n\n"));
        }
        let delta = json!({"type":"response.output_text.delta", "delta":"unrelated"});
        let terminal =
            json!({"type":"response.completed", "response":{"status":"completed", "output":items}});
        sse.push_str(&format!("data: {delta}\n\ndata: {terminal}\n\n"));
        let body = buffer_event_stream_response(&Bytes::from(sse)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["output"], items);
    }
}
