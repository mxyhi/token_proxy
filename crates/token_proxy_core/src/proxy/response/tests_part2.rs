use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

use crate::proxy::log::{LogContext, LogWriter};

#[test]
fn stream_gemini_to_anthropic_emits_single_input_json_delta_for_tool_calls() {
    super::run_async(async {
        let context = LogContext {
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
fn stream_anthropic_to_responses_emits_reasoning_summary_events_and_snapshot() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
        let context = LogContext {
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
fn stream_anthropic_to_responses_maps_redacted_thinking_to_encrypted_reasoning() {
    super::run_async(async {
        let sqlite_pool = super::create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
        let context = LogContext {
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
