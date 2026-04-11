use super::buffered::{empty_chat_completion_retry_message, value_is_absent};
use crate::proxy::log::LogContext;
use crate::proxy::openai_compat::FormatTransform;
use axum::body::Bytes;
use serde_json::json;
use std::time::Instant;

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
