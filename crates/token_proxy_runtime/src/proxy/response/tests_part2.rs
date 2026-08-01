use axum::body::Bytes;
use futures_util::{future, StreamExt};
use serde_json::{json, Value};
use sqlx::Row;
use std::{sync::Arc, time::Duration, time::Instant};

use crate::proxy::log::{LogContext, LogWriter};

#[test]
fn stream_with_logging_marks_first_output_on_responses_non_preamble_event() {
    super::run_async(async {
        let (log, context, sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunks: Vec<Bytes> =
            super::super::streaming::stream_with_logging(upstream, context, log, token_tracker)
                .map(|item| item.expect("stream item"))
                .collect()
                .await;

        assert_eq!(chunks.len(), 3);
        super::wait_for_log_rows(&sqlite_pool, 1).await;
        let row = sqlx::query("SELECT first_output_ms FROM request_logs ORDER BY id LIMIT 1")
            .fetch_one(&sqlite_pool)
            .await
            .expect("request log row");
        let first_output_ms = row
            .try_get::<Option<i64>, _>("first_output_ms")
            .expect("first_output_ms");
        assert!(
            first_output_ms.is_some(),
            "first non-preamble Responses event should count as client output"
        );
    });
}

#[test]
fn stream_with_logging_does_not_count_responses_final_snapshot_text_for_token_rate() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"final text\"}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);
        let token_rate = crate::proxy::token_rate::TokenRateTracker::new();
        let token_tracker = token_rate.register(None, None).await;
        let stream =
            super::super::streaming::stream_with_logging(upstream, context, log, token_tracker);
        futures_util::pin_mut!(stream);
        let first_chunk = stream
            .next()
            .await
            .expect("first stream item")
            .expect("stream ok");
        let snapshot = token_rate.snapshot().await;

        assert!(
            String::from_utf8_lossy(&first_chunk).contains("response.output_text.done"),
            "chunk: {first_chunk:?}"
        );
        assert_eq!(snapshot.output, 0, "snapshot: {snapshot:?}");
    });
}

#[test]
fn stream_with_logging_closes_after_responses_terminal_without_upstream_close() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::unfold(0usize, |index| async move {
            if index == 0 {
                return Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(
                        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
                    )),
                    1,
                ));
            }
            future::pending::<Option<(Result<Bytes, std::io::Error>, usize)>>().await
        })
        .boxed();
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let stream =
            super::super::streaming::stream_with_logging(upstream, context, log, token_tracker);

        let chunks = tokio::time::timeout(
            Duration::from_millis(200),
            stream
                .map(|item| item.expect("stream item"))
                .collect::<Vec<Bytes>>(),
        )
        .await
        .expect("stream should end after terminal Responses event");

        let body = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<String>();
        assert!(body.contains("\"type\":\"response.completed\""));
        assert!(body.contains("data: [DONE]"));
    });
}

#[test]
fn stream_with_logging_semantic_timeout_emits_response_failed_and_done() {
    super::run_async(async {
        let (log, mut context, sqlite_pool) = super::setup_responses_stream().await;
        context.request_headers = Some("{}".to_string());
        let upstream = futures_util::stream::unfold(0usize, |index| async move {
            if index == 0 {
                return Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(
                        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\",\"sequence_number\":7}\n\n",
                    )),
                    1,
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((Ok::<Bytes, std::io::Error>(Bytes::from(":\n\n")), index + 1))
        })
        .boxed();
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let stream = super::super::streaming::stream_with_logging_and_semantic_timeout(
            upstream,
            context,
            log,
            token_tracker,
            Some(Duration::from_millis(40)),
        );

        let chunks = tokio::time::timeout(
            Duration::from_millis(500),
            stream
                .map(|item| item.expect("semantic timeout should be an SSE event"))
                .collect::<Vec<Bytes>>(),
        )
        .await
        .expect("heartbeat-only stream should not hang");
        let body = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<String>();

        assert!(body.contains("event: response.failed"), "chunks: {body}");
        assert!(
            body.contains("\"type\":\"response.failed\""),
            "chunks: {body}"
        );
        assert!(body.contains("\"status\":\"failed\""), "chunks: {body}");
        assert!(body.contains("semantic timeout"), "chunks: {body}");
        assert!(body.contains("data: [DONE]"), "chunks: {body}");
        assert!(
            body.contains("\"sequence_number\":8"),
            "terminal event must continue the Responses sequence: {body}"
        );
        let failed = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.contains("response.failed"))
            .and_then(|line| serde_json::from_str::<Value>(line).ok())
            .expect("response.failed payload");
        assert!(failed["response"]["created_at"].as_i64().is_some());
        assert_eq!(failed["response"]["model"], json!("unit-model"));

        super::wait_for_log_rows(&sqlite_pool, 1).await;
        let row = sqlx::query("SELECT response_error FROM request_logs ORDER BY id LIMIT 1")
            .fetch_one(&sqlite_pool)
            .await
            .expect("request log row");
        let response_error = row
            .try_get::<Option<String>, _>("response_error")
            .expect("response_error");
        assert!(
            response_error
                .as_deref()
                .is_some_and(|value| value.contains("semantic timeout")),
            "unexpected response_error: {response_error:?}"
        );
    });
}

#[test]
fn stream_with_logging_upstream_error_emits_response_failed_after_stream_started() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\",\"sequence_number\":7}\n\n",
            )),
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "upstream reset",
            )),
        ]);
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let stream = super::super::streaming::stream_with_logging_and_semantic_timeout(
            upstream,
            context,
            log,
            token_tracker,
            Some(Duration::from_secs(30)),
        );

        let chunks = stream
            .map(|item| item.expect("upstream error should become response.failed SSE"))
            .collect::<Vec<Bytes>>()
            .await;
        let body = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<String>();

        assert!(body.contains("event: response.failed"), "chunks: {body}");
        assert!(
            body.contains("\"type\":\"response.failed\""),
            "chunks: {body}"
        );
        assert!(body.contains("upstream reset"), "chunks: {body}");
        assert!(body.contains("data: [DONE]"), "chunks: {body}");
        assert!(
            body.contains("\"sequence_number\":8"),
            "terminal event must continue the Responses sequence: {body}"
        );
    });
}

#[test]
fn stream_with_logging_normalizes_split_upstream_response_failed() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\",\"sequence_number\":7}\n\n\
                 data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",",
            )),
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "\"message\":\"upstream overloaded\"}}}\n\ndata: [DONE]\n\n",
            )),
        ]);
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let stream = super::super::streaming::stream_with_logging_and_semantic_timeout(
            upstream,
            context,
            log,
            token_tracker,
            Some(Duration::from_secs(30)),
        );

        let chunks = stream
            .map(|item| item.expect("normalized stream item"))
            .collect::<Vec<Bytes>>()
            .await;
        let body = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<String>();
        let failed = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.contains("response.failed"))
            .and_then(|line| serde_json::from_str::<Value>(line).ok())
            .expect("response.failed payload");

        assert_eq!(failed["type"], json!("response.failed"));
        assert_eq!(failed["sequence_number"], json!(8));
        assert!(failed["response"]["created_at"].as_i64().is_some());
        assert_eq!(failed["response"]["model"], json!("unit-model"));
        assert_eq!(
            failed["response"]["error"]["message"],
            json!("upstream overloaded")
        );
        assert!(body.contains("data: [DONE]"));
    });
}

#[test]
fn stream_with_logging_preserves_non_sse_responses_body() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
            r#"{"error":{"message":"unexpected JSON body"}}"#,
        ))]);
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunks =
            super::super::streaming::stream_with_logging(upstream, context, log, token_tracker)
                .map(|item| item.expect("stream item"))
                .collect::<Vec<_>>()
                .await;

        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].as_ref(),
            br#"{"error":{"message":"unexpected JSON body"}}"#
        );
    });
}

#[test]
fn stream_with_model_override_closes_after_responses_terminal_without_upstream_close() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::unfold(0usize, |index| async move {
            if index == 0 {
                return Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(
                        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"actual-model\"}}\n\n",
                    )),
                    1,
                ));
            }
            future::pending::<Option<(Result<Bytes, std::io::Error>, usize)>>().await
        })
        .boxed();
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let stream =
            super::super::streaming::stream_with_logging_and_model_override_semantic_timeout(
                upstream,
                context,
                log,
                "visible-model".to_string(),
                token_tracker,
                Some(Duration::from_secs(30)),
            );

        let chunks = tokio::time::timeout(
            Duration::from_millis(200),
            stream
                .map(|item| item.expect("stream item"))
                .collect::<Vec<Bytes>>(),
        )
        .await
        .expect("model override stream should end after terminal event");
        let body = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<String>();

        assert!(
            body.contains("\"model\":\"visible-model\""),
            "chunks: {body}"
        );
        assert!(body.contains("data: [DONE]"), "chunks: {body}");
    });
}

#[test]
fn stream_with_model_override_semantic_timeout_emits_response_failed_and_done() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;
        let upstream = futures_util::stream::unfold(0usize, |index| async move {
            if index == 0 {
                return Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from(
                        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
                    )),
                    1,
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((Ok::<Bytes, std::io::Error>(Bytes::from(":\n\n")), index + 1))
        })
        .boxed();
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let stream =
            super::super::streaming::stream_with_logging_and_model_override_semantic_timeout(
                upstream,
                context,
                log,
                "visible-model".to_string(),
                token_tracker,
                Some(Duration::from_millis(40)),
            );

        let chunks = tokio::time::timeout(
            Duration::from_millis(500),
            stream
                .map(|item| item.expect("semantic timeout should be an SSE event"))
                .collect::<Vec<Bytes>>(),
        )
        .await
        .expect("heartbeat-only model override stream should not hang");
        let body = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect::<String>();

        assert!(body.contains("event: response.failed"), "chunks: {body}");
        assert!(
            body.contains("\"type\":\"response.failed\""),
            "chunks: {body}"
        );
        assert!(body.contains("\"status\":\"failed\""), "chunks: {body}");
        assert!(body.contains("semantic timeout"), "chunks: {body}");
        assert!(body.contains("data: [DONE]"), "chunks: {body}");
        let failed = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.contains("response.failed"))
            .and_then(|line| serde_json::from_str::<Value>(line).ok())
            .expect("response.failed payload");
        assert!(failed["response"]["created_at"].as_i64().is_some());
        assert_eq!(failed["response"]["model"], json!("visible-model"));
    });
}

#[test]
fn stream_gemini_to_anthropic_emits_single_input_json_delta_for_tool_calls() {
    super::run_async(async {
        let context = LogContext {
            client_ip: None,
            path: "/v1/messages".to_string(),
            provider: "gemini".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let gemini_event = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "Task",
                                    "args": {
                                        "description": "explore",
                                        "prompt": "scan repo",
                                        "subagent_type": "Explore"
                                    }
                                }
                            }
                        ]
                    },
                    "finishReason": "STOP"
                }
            ]
        });
        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(format!(
                "data: {}\n\n",
                gemini_event.to_string()
            ))),
            Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker_1 = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chat_stream = crate::proxy::gemini_compat::stream_gemini_to_chat(
            upstream,
            context.clone(),
            Arc::new(LogWriter::new(None)),
            token_tracker_1,
        )
        .boxed();

        let token_tracker_2 = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let responses_stream = super::super::chat_to_responses::stream_chat_to_responses(
            chat_stream,
            context.clone(),
            Arc::new(LogWriter::new(None)),
            token_tracker_2,
        )
        .boxed();

        let token_tracker_3 = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let anthropic_stream = super::super::responses_to_anthropic::stream_responses_to_anthropic(
            responses_stream,
            context,
            Arc::new(LogWriter::new(None)),
            token_tracker_3,
        );

        let chunks: Vec<Bytes> = anthropic_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        let mut input_json_deltas: Vec<String> = Vec::new();
        for chunk in &chunks {
            let Some((event_type, data)) = super::parse_anthropic_sse(chunk) else {
                continue;
            };
            if event_type != "content_block_delta" {
                continue;
            }
            if data
                .get("delta")
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                != Some("input_json_delta")
            {
                continue;
            }
            let Some(partial) = data
                .get("delta")
                .and_then(|value| value.get("partial_json"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            input_json_deltas.push(partial.to_string());
        }

        // If we emit both `.delta` fragments and the final `.done` full arguments, clients will
        // concatenate them and end up with invalid JSON (tool input becomes `{}`).
        assert_eq!(input_json_deltas.len(), 1);
        assert!(input_json_deltas[0].contains("\"description\""));
        assert!(input_json_deltas[0].contains("\"prompt\""));
        assert!(input_json_deltas[0].contains("\"subagent_type\""));
    });
}

#[test]
fn stream_responses_to_chat_persists_log_when_client_drops_stream_early() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
        let context = LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };
        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);
        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        {
            let stream = super::super::responses_to_chat::stream_responses_to_chat(
                upstream,
                context,
                log,
                token_tracker,
            );
            futures_util::pin_mut!(stream);
            let first = stream
                .next()
                .await
                .expect("first stream item")
                .expect("stream ok");
            assert!(!first.is_empty());
        }
        let count = super::wait_for_log_rows(&sqlite_pool, 1).await;
        assert!(
            count >= 1,
            "responses_to_chat stream dropped early should still persist request log row, got {count}"
        );
    });
}

#[test]
fn stream_responses_to_anthropic_emits_thinking_from_reasoning_summary_events() {
    super::run_async(async {
        let context = LogContext {
            client_ip: None,
            path: "/v1/messages".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"rs_1\",\"type\":\"reasoning\"}}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs_1\",\"delta\":\"think step by step\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"id\":\"rs_1\",\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"think step by step\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let anthropic_stream = super::super::responses_to_anthropic::stream_responses_to_anthropic(
            upstream,
            context,
            Arc::new(LogWriter::new(None)),
            token_tracker,
        );

        let chunks: Vec<Bytes> = anthropic_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        let mut saw_thinking_start = false;
        let mut saw_thinking_delta = false;
        for chunk in &chunks {
            let Some((event_type, data)) = super::parse_anthropic_sse(chunk) else {
                continue;
            };
            if event_type == "content_block_start"
                && data["content_block"]["type"] == json!("thinking")
            {
                saw_thinking_start = true;
            }
            if event_type == "content_block_delta"
                && data["delta"]["type"] == json!("thinking_delta")
                && data["delta"]["thinking"] == json!("think step by step")
            {
                saw_thinking_delta = true;
            }
        }

        assert!(saw_thinking_start, "missing thinking content_block_start");
        assert!(
            saw_thinking_delta,
            "missing thinking_delta from reasoning summary"
        );
    });
}

#[test]
fn stream_responses_to_anthropic_emits_redacted_thinking_from_encrypted_reasoning() {
    super::run_async(async {
        let context = LogContext {
            client_ip: None,
            path: "/v1/messages".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"id\":\"rs_1\",\"type\":\"reasoning\",\"encrypted_content\":\"ENC999\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let anthropic_stream = super::super::responses_to_anthropic::stream_responses_to_anthropic(
            upstream,
            context,
            Arc::new(LogWriter::new(None)),
            token_tracker,
        );

        let chunks: Vec<Bytes> = anthropic_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        assert!(
            chunks
                .iter()
                .filter_map(super::parse_anthropic_sse)
                .any(|(event_type, data)| {
                    event_type == "content_block_start"
                        && data["content_block"]["type"] == json!("redacted_thinking")
                        && data["content_block"]["data"] == json!("ENC999")
                }),
            "missing redacted_thinking content block"
        );
    });
}

#[test]
fn stream_responses_to_chat_emits_reasoning_from_reasoning_summary_events() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"rs_1\",\"type\":\"reasoning\"}}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs_1\",\"delta\":\"think step by step\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"id\":\"rs_1\",\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"think step by step\"}]}]}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert_eq!(
            payloads[0]["choices"][0]["delta"]["role"],
            json!("assistant")
        );
        assert_eq!(
            payloads[1]["choices"][0]["delta"]["reasoning_content"],
            json!("think step by step")
        );
        assert_eq!(payloads[2]["choices"][0]["finish_reason"], json!("stop"));
    });
}

#[test]
fn stream_responses_to_chat_emits_usage_chunk_from_terminal_event_usage() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.completed\",\"usage\":{\"input_tokens\":8,\"output_tokens\":3,\"total_tokens\":11,\"input_tokens_details\":{\"cached_tokens\":5,\"cached_creation_tokens\":1,\"audio_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":4,\"audio_tokens\":6,\"accepted_prediction_tokens\":7,\"rejected_prediction_tokens\":8}},\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;
        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();
        let usage = payloads
            .iter()
            .find(|payload| payload["choices"] == json!([]))
            .expect("usage chunk");

        assert_eq!(usage["usage"]["prompt_tokens"], json!(8));
        assert_eq!(usage["usage"]["completion_tokens"], json!(3));
        assert_eq!(usage["usage"]["total_tokens"], json!(11));
        assert_eq!(
            usage["usage"]["prompt_tokens_details"]["cached_tokens"],
            json!(5)
        );
        assert_eq!(
            usage["usage"]["prompt_tokens_details"]["audio_tokens"],
            json!(2)
        );
        assert_eq!(
            usage["usage"]["prompt_tokens_details"]["cached_creation_tokens"],
            json!(1)
        );
        assert_eq!(
            usage["usage"]["completion_tokens_details"]["reasoning_tokens"],
            json!(4)
        );
        assert_eq!(
            usage["usage"]["completion_tokens_details"]["audio_tokens"],
            json!(6)
        );
        assert_eq!(
            usage["usage"]["completion_tokens_details"]["accepted_prediction_tokens"],
            json!(7)
        );
        assert_eq!(
            usage["usage"]["completion_tokens_details"]["rejected_prediction_tokens"],
            json!(8)
        );
    });
}

#[test]
fn stream_responses_to_chat_emits_thinking_blocks_from_reasoning_summary_events() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs_1\",\"delta\":\"think step by step\"}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert_eq!(
            payloads[1]["choices"][0]["delta"]["thinking_blocks"][0],
            json!({
                "type": "thinking",
                "thinking": "think step by step"
            })
        );
        assert_eq!(payloads[2]["choices"][0]["finish_reason"], json!("stop"));
    });
}

#[test]
fn stream_responses_to_chat_emits_redacted_thinking_blocks_from_reasoning_snapshot() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"id\":\"rs_1\",\"type\":\"reasoning\",\"encrypted_content\":\"ENC999\"}]}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert_eq!(
            payloads[1]["choices"][0]["delta"]["thinking_blocks"][0],
            json!({
                "type": "redacted_thinking",
                "data": "ENC999"
            })
        );
        assert_eq!(payloads[2]["choices"][0]["finish_reason"], json!("stop"));
    });
}

#[test]
fn stream_responses_to_chat_emits_thinking_blocks_from_reasoning_text_snapshot_parts() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"reasoning_text\",\"text\":\"think from snapshot\"}]}]}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert_eq!(
            payloads[1]["choices"][0]["delta"]["thinking_blocks"][0],
            json!({
                "type": "thinking",
                "thinking": "think from snapshot"
            })
        );
        assert_eq!(
            payloads[1]["choices"][0]["delta"]["reasoning_content"],
            json!("think from snapshot")
        );
        assert_eq!(payloads[2]["choices"][0]["finish_reason"], json!("stop"));
    });
}

#[test]
fn stream_responses_to_chat_emits_audio_delta_from_output_audio_snapshot() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_audio\",\"audio\":{\"data\":\"UklGRg==\",\"transcript\":\"spoken\"}}]}]}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert_eq!(
            payloads[0]["choices"][0]["delta"]["role"],
            json!("assistant")
        );
        assert_eq!(
            payloads[1]["choices"][0]["delta"]["audio"]["data"],
            json!("UklGRg==")
        );
        assert_eq!(
            payloads[1]["choices"][0]["delta"]["audio"]["transcript"],
            json!("spoken")
        );
        assert_eq!(payloads[2]["choices"][0]["finish_reason"], json!("stop"));
    });
}

#[test]
fn stream_responses_to_chat_emits_chat_error_for_response_failed() {
    super::run_async(async {
        let (log, context, _sqlite_pool) = super::setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(
            Bytes::from(
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"model overloaded\",\"type\":\"server_error\",\"code\":\"server_overloaded\"}}}\n\n",
            ),
        )]);

        let chunks = super::collect_responses_to_chat_chunks(upstream, context, log).await;
        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["error"]["message"], json!("model overloaded"));
        assert_eq!(payloads[0]["error"]["type"], json!("server_error"));
        assert_eq!(payloads[0]["error"]["code"], json!("server_overloaded"));
    });
}

#[test]
fn stream_responses_to_anthropic_emits_error_without_message_stop() {
    super::run_async(async {
        let cases = [
            (
                "response.failed",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"model overloaded\",\"type\":\"server_error\",\"code\":\"server_overloaded\"}}}\n\n",
                "model overloaded",
                "server_error",
                json!("server_overloaded"),
                503_i64,
            ),
            (
                "response.error",
                "data: {\"type\":\"response.error\",\"error\":{\"message\":\"request rejected\",\"type\":\"invalid_request_error\",\"code\":\"bad_request\"}}\n\n",
                "request rejected",
                "invalid_request_error",
                json!("bad_request"),
                400_i64,
            ),
            (
                "error",
                "data: {\"type\":\"error\",\"error\":{\"message\":\"upstream unavailable\",\"type\":\"api_error\",\"code\":503}}\n\n",
                "upstream unavailable",
                "api_error",
                json!(503),
                503_i64,
            ),
        ];

        for (name, event, expected_message, expected_type, expected_code, expected_status) in cases
        {
            let (log, context, sqlite_pool) = super::setup_responses_stream().await;
            let upstream =
                futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(event))]);
            let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let anthropic_stream =
                super::super::responses_to_anthropic::stream_responses_to_anthropic(
                    upstream,
                    context,
                    log,
                    token_tracker,
                );
            let chunks = anthropic_stream
                .map(|item| item.expect("stream item"))
                .collect::<Vec<_>>()
                .await;
            let events = chunks
                .iter()
                .filter_map(super::parse_anthropic_sse)
                .collect::<Vec<_>>();

            assert_eq!(events.len(), 1, "case={name}: {events:?}");
            assert_eq!(events[0].0, "error", "case={name}");
            assert_eq!(events[0].1["type"], json!("error"), "case={name}");
            assert_eq!(
                events[0].1["error"]["message"],
                json!(expected_message),
                "case={name}"
            );
            assert_eq!(
                events[0].1["error"]["type"],
                json!(expected_type),
                "case={name}"
            );
            assert_eq!(events[0].1["error"]["code"], expected_code, "case={name}");
            assert!(
                events
                    .iter()
                    .all(|(event_type, _)| event_type != "message_stop"),
                "case={name}: {events:?}"
            );

            assert_eq!(super::wait_for_log_rows(&sqlite_pool, 1).await, 1);
            let row =
                sqlx::query("SELECT status, response_error FROM request_logs ORDER BY id LIMIT 1")
                    .fetch_one(&sqlite_pool)
                    .await
                    .expect("request log");
            assert_eq!(
                row.try_get::<i64, _>("status").expect("status"),
                expected_status,
                "case={name}"
            );
            assert_eq!(
                row.try_get::<Option<String>, _>("response_error")
                    .expect("response_error")
                    .as_deref(),
                Some(expected_message),
                "case={name}"
            );
        }
    });
}

#[test]
fn stream_responses_to_gemini_emits_error_without_stop_candidate() {
    super::run_async(async {
        let cases = [
            (
                "response.failed",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"model overloaded\",\"type\":\"server_error\",\"code\":\"server_overloaded\"}}}\n\n",
                "model overloaded",
                503_i64,
                "UNAVAILABLE",
            ),
            (
                "response.error",
                "data: {\"type\":\"response.error\",\"error\":{\"message\":\"request rejected\",\"type\":\"invalid_request_error\",\"code\":\"bad_request\"}}\n\n",
                "request rejected",
                400_i64,
                "INVALID_ARGUMENT",
            ),
            (
                "error",
                "data: {\"type\":\"error\",\"error\":{\"message\":\"upstream unavailable\",\"type\":\"api_error\",\"code\":503}}\n\n",
                "upstream unavailable",
                503_i64,
                "UNAVAILABLE",
            ),
        ];

        for (name, event, expected_message, expected_code, expected_status) in cases {
            let (log, context, sqlite_pool) = super::setup_responses_stream().await;
            let upstream =
                futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(event))]);
            let intermediate_tracker = crate::proxy::token_rate::RequestTokenTracker::disabled();
            let chat_stream = super::super::responses_to_chat::stream_responses_to_chat(
                upstream,
                context.clone(),
                Arc::new(LogWriter::new(None)),
                intermediate_tracker,
            )
            .boxed();
            let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let gemini_stream = crate::proxy::gemini_compat::stream_chat_to_gemini(
                chat_stream,
                context,
                log,
                token_tracker,
            );
            let chunks = gemini_stream
                .map(|item| item.expect("stream item"))
                .collect::<Vec<_>>()
                .await;
            let payloads = chunks
                .iter()
                .filter_map(super::parse_sse_json)
                .collect::<Vec<_>>();

            assert_eq!(payloads.len(), 1, "case={name}: {payloads:?}");
            assert_eq!(
                payloads[0]["error"]["message"],
                json!(expected_message),
                "case={name}"
            );
            assert_eq!(
                payloads[0]["error"]["code"],
                json!(expected_code),
                "case={name}"
            );
            assert_eq!(
                payloads[0]["error"]["status"],
                json!(expected_status),
                "case={name}"
            );
            assert!(payloads[0].get("candidates").is_none(), "case={name}");

            assert_eq!(super::wait_for_log_rows(&sqlite_pool, 1).await, 1);
            let row =
                sqlx::query("SELECT status, response_error FROM request_logs ORDER BY id LIMIT 1")
                    .fetch_one(&sqlite_pool)
                    .await
                    .expect("request log");
            assert_eq!(
                row.try_get::<i64, _>("status").expect("status"),
                expected_code,
                "case={name}"
            );
            assert_eq!(
                row.try_get::<Option<String>, _>("response_error")
                    .expect("response_error")
                    .as_deref(),
                Some(expected_message),
                "case={name}"
            );
        }
    });
}

#[test]
fn stream_anthropic_to_responses_emits_reasoning_summary_events_and_snapshot() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
        let context = LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "anthropic".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("claude-3-7-sonnet".to_string()),
            mapped_model: Some("claude-3-7-sonnet".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-3-7-sonnet\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"analyze first\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"final answer\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            )),
            Ok(Bytes::from(
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let responses_stream = super::super::anthropic_to_responses::stream_anthropic_to_responses(
            upstream,
            context,
            log,
            token_tracker,
        );

        let chunks: Vec<Bytes> = responses_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        assert!(
            payloads.iter().any(|payload| {
                payload["type"] == json!("response.output_item.added")
                    && payload["item"]["type"] == json!("reasoning")
            }),
            "missing reasoning output_item.added event"
        );
        assert!(
            payloads.iter().any(|payload| {
                payload["type"] == json!("response.reasoning_summary_text.delta")
                    && payload["delta"] == json!("analyze first")
            }),
            "missing reasoning_summary_text.delta event"
        );

        let completed = payloads
            .iter()
            .find(|payload| payload["type"] == json!("response.completed"))
            .expect("completed event");
        assert_eq!(
            completed["response"]["output"][0]["type"],
            json!("reasoning")
        );
        assert_eq!(
            completed["response"]["output"][0]["summary"][0],
            json!({ "type": "summary_text", "text": "analyze first" })
        );
        assert_eq!(completed["response"]["output"][1]["type"], json!("message"));
        assert_eq!(
            completed["response"]["output"][1]["content"][0]["text"],
            json!("final answer")
        );
    });
}

#[test]
fn stream_anthropic_to_responses_adds_cache_tokens_to_openai_input_usage() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool)));
        let context = LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "anthropic".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("claude-3-7-sonnet".to_string()),
            mapped_model: Some("claude-3-7-sonnet".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-3-7-sonnet\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":6}}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"final answer\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":3},\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}}\n\n",
            )),
            Ok(Bytes::from(
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let responses_stream = super::super::anthropic_to_responses::stream_anthropic_to_responses(
            upstream,
            context,
            log,
            token_tracker,
        );

        let chunks: Vec<Bytes> = responses_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;
        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();
        let completed = payloads
            .iter()
            .find(|payload| payload["type"] == json!("response.completed"))
            .expect("completed event");
        let usage = &completed["response"]["usage"];

        assert_eq!(usage["input_tokens"], json!(20));
        assert_eq!(usage["output_tokens"], json!(3));
        assert_eq!(usage["total_tokens"], json!(23));
        assert_eq!(usage["input_tokens_details"]["cached_tokens"], json!(4));
        assert_eq!(
            usage["input_tokens_details"]["cached_creation_tokens"],
            json!(6)
        );
    });
}

#[test]
fn stream_anthropic_to_responses_maps_redacted_thinking_to_encrypted_reasoning() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
        let context = LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "anthropic".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("claude-3-7-sonnet".to_string()),
            mapped_model: Some("claude-3-7-sonnet".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-3-7-sonnet\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"ENC789\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from(
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let responses_stream = super::super::anthropic_to_responses::stream_anthropic_to_responses(
            upstream,
            context,
            log,
            token_tracker,
        );

        let chunks: Vec<Bytes> = responses_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        let completed = payloads
            .iter()
            .find(|payload| payload["type"] == json!("response.completed"))
            .expect("completed event");
        assert_eq!(
            completed["response"]["output"][0]["type"],
            json!("reasoning")
        );
        assert_eq!(
            completed["response"]["output"][0]["encrypted_content"],
            json!("ENC789")
        );
    });
}

#[test]
fn stream_anthropic_to_responses_maps_max_tokens_to_incomplete_event() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
        let context = LogContext {
            client_ip: None,
            path: "/v1/responses".to_string(),
            provider: "anthropic".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("claude-3-7-sonnet".to_string()),
            mapped_model: Some("claude-3-7-sonnet".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-3-7-sonnet\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial answer\"}}\n\n",
            )),
            Ok(Bytes::from(
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\",\"stop_sequence\":null}}\n\n",
            )),
            Ok(Bytes::from(
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let responses_stream = super::super::anthropic_to_responses::stream_anthropic_to_responses(
            upstream,
            context,
            log,
            token_tracker,
        );

        let chunks: Vec<Bytes> = responses_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        let incomplete = payloads
            .iter()
            .find(|payload| payload["type"] == json!("response.incomplete"))
            .expect("incomplete event");
        assert_eq!(incomplete["response"]["status"], json!("incomplete"));
        assert_eq!(
            incomplete["response"]["incomplete_details"]["reason"],
            json!("max_tokens")
        );
        assert_eq!(
            incomplete["response"]["output"][0]["type"],
            json!("message")
        );
        assert_eq!(
            incomplete["response"]["output"][0]["status"],
            json!("incomplete")
        );
        assert_eq!(
            incomplete["response"]["output"][0]["content"][0]["text"],
            json!("partial answer")
        );
    });
}

#[test]
fn stream_chat_to_gemini_waits_for_complete_tool_call_arguments() {
    super::run_async(async {
        let context = LogContext {
            client_ip: None,
            path: "/v1/messages".to_string(),
            provider: "openai".to_string(),
            upstream_id: "unit-test".to_string(),
            account_id: None,
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings: Default::default(),
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::from(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Paris\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let gemini_stream = crate::proxy::gemini_compat::stream_chat_to_gemini(
            upstream,
            context,
            Arc::new(LogWriter::new(None)),
            token_tracker,
        );

        let chunks: Vec<Bytes> = gemini_stream
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        let payloads = chunks
            .iter()
            .filter_map(super::parse_sse_json)
            .collect::<Vec<_>>();

        let function_calls = payloads
            .iter()
            .flat_map(|payload| {
                payload["candidates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|candidate| candidate["content"]["parts"].as_array())
                    .flatten()
                    .filter_map(|part| part.get("functionCall"))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        assert_eq!(function_calls.len(), 1);
        assert_eq!(function_calls[0]["name"], json!("get_weather"));
        assert_eq!(function_calls[0]["args"]["city"], json!("Paris"));
    });
}

#[test]
fn stream_with_logging_records_terminal_error_flushed_only_at_eof() {
    super::run_async(async {
        let cases = [
            (
                "response.failed",
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"model overloaded\",\"type\":\"server_error\",\"code\":\"server_overloaded\"}}}",
                "model overloaded",
                503_i64,
            ),
            (
                "response.error",
                "data: {\"type\":\"response.error\",\"error\":{\"message\":\"request rejected\",\"type\":\"invalid_request_error\",\"code\":\"bad_request\"}}",
                "request rejected",
                400_i64,
            ),
            (
                "error",
                "data: {\"type\":\"error\",\"error\":{\"message\":\"upstream unavailable\",\"type\":\"api_error\",\"code\":503}}",
                "upstream unavailable",
                503_i64,
            ),
        ];

        for (name, event, expected_error, expected_status) in cases {
            let (log, context, sqlite_pool) = super::setup_responses_stream().await;
            // 故意不带尾部换行，确保 terminal event 只能由 parser.finish() 识别。
            let upstream =
                futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(event))]);
            let token_tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let chunks =
                super::super::streaming::stream_with_logging(upstream, context, log, token_tracker)
                    .map(|item| item.expect("stream item"))
                    .collect::<Vec<_>>()
                    .await;

            assert_eq!(chunks.len(), 1, "case={name}");
            assert_eq!(super::wait_for_log_rows(&sqlite_pool, 1).await, 1);
            let row =
                sqlx::query("SELECT status, response_error FROM request_logs ORDER BY id LIMIT 1")
                    .fetch_one(&sqlite_pool)
                    .await
                    .expect("request log");
            assert_eq!(
                row.try_get::<i64, _>("status").expect("status"),
                expected_status,
                "case={name}"
            );
            assert_eq!(
                row.try_get::<Option<String>, _>("response_error")
                    .expect("response_error")
                    .as_deref(),
                Some(expected_error),
                "case={name}"
            );
        }
    });
}
