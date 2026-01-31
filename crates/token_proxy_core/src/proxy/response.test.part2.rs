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
            provider: "antigravity".to_string(),
            upstream_id: "unit-test".to_string(),
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
