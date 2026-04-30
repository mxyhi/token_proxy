use super::buffered::{
    buffer_event_stream_response, build_buffered_response, empty_chat_completion_retry_message,
    value_is_absent,
};
use crate::proxy::openai_compat::FormatTransform;
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
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn test_context() -> LogContext {
    LogContext {
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
        ttfb_ms: None,
        timings: Default::default(),
        start: Instant::now(),
    }
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
