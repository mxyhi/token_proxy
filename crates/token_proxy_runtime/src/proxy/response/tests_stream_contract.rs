use super::*;

fn event(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

fn payloads(chunks: Vec<Bytes>) -> Vec<Value> {
    let mut parser = crate::proxy::sse::SseEventParser::new();
    let mut values = Vec::new();
    for chunk in chunks {
        parser
            .push_chunk(&chunk, |data| {
                if let Ok(value) = serde_json::from_str(&data) {
                    values.push(value);
                }
            })
            .expect("parse fixture SSE");
    }
    values
}

async fn converted(chunks: Vec<Bytes>, anthropic: bool) -> (Vec<Value>, SqlitePool) {
    let (log, mut context, pool) = setup_responses_stream().await;
    context.provider = if anthropic { "anthropic" } else { "openai" }.to_string();
    let tracker = crate::proxy::token_rate::TokenRateTracker::new()
        .register(None, None)
        .await;
    let upstream = futures_util::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
    let stream = if anthropic {
        super::super::anthropic_to_responses::stream_anthropic_to_responses(
            upstream, context, log, tracker,
        )
        .boxed()
    } else {
        super::super::chat_to_responses::stream_chat_to_responses(upstream, context, log, tracker)
            .boxed()
    };
    let chunks = stream
        .map(|item| item.expect("protocol event"))
        .collect()
        .await;
    (payloads(chunks), pool)
}

fn assert_reasoning_lifecycle(values: &[Value], expected: usize, incomplete: bool) {
    let parts = values
        .iter()
        .filter(|v| v["type"] == "response.reasoning_summary_part.added")
        .collect::<Vec<_>>();
    assert_eq!(
        parts.len(),
        expected,
        "missing summary part creation: {values:?}"
    );
    let sequences = values
        .iter()
        .filter_map(|v| v["sequence_number"].as_u64())
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    for part in parts {
        let item_id = &part["item_id"];
        let matching = values
            .iter()
            .filter(|v| &v["item_id"] == item_id || &v["item"]["id"] == item_id)
            .collect::<Vec<_>>();
        let names = matching
            .iter()
            .filter_map(|v| v["type"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "response.output_item.added",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_text.delta",
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_item.done"
            ]
        );
        for value in &matching {
            assert_eq!(value["output_index"], part["output_index"]);
            if value["type"].as_str().unwrap().contains("summary") {
                assert_eq!(value["summary_index"], 0);
            }
        }
        let done = matching
            .iter()
            .find(|v| v["type"] == "response.reasoning_summary_part.done")
            .unwrap();
        if incomplete {
            assert_eq!(done["status"], "incomplete");
        } else {
            assert!(done.get("status").is_none());
        }
    }
}

#[test]
fn cpa_chat_eof_and_done_without_finish_fail_and_log() {
    run_async(async {
        for done in [false, true] {
            for delta in [
                json!({"content":"partial"}),
                json!({"tool_calls":[{"index":0,"id":"call_1","function":{"name":"run","arguments":"{\"unfinished\":"}}]}),
            ] {
                let mut chunks = vec![event(
                    json!({"choices":[{"delta":delta,"finish_reason":null}]}),
                )];
                if done {
                    chunks.push(Bytes::from("data: [DONE]\n\n"));
                }
                let (values, pool) = converted(chunks, false).await;
                let terminal = values
                    .iter()
                    .find(|v| v["type"] == "response.failed")
                    .expect("missing finish_reason must fail");
                assert_eq!(terminal["response"]["status"], "failed");
                assert!(terminal["response"]["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("finish_reason"));
                assert!(!values.iter().any(|v| v["type"] == "response.completed"));
                wait_for_log_rows(&pool, 1).await;
                let row = sqlx::query("SELECT status,response_error FROM request_logs")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(row.get::<i64, _>("status"), 502);
                assert!(row
                    .get::<String, _>("response_error")
                    .contains("finish_reason"));
            }
        }
    });
}

#[test]
fn cpa_raw_chat_truncation_is_protocol_error_with_and_without_model_override() {
    run_async(async {
        for override_model in [false, true] {
            for done in [false, true] {
                let (log, mut context, pool) = setup_responses_stream().await;
                context.path = "/v1/chat/completions".into();
                context.provider = "openai".into();
                let tracker = crate::proxy::token_rate::TokenRateTracker::new()
                    .register(None, None)
                    .await;
                let mut chunks = vec![event(
                    json!({"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}),
                )];
                if done {
                    chunks.extend([Bytes::from("data: [DO"), Bytes::from("NE]\n\n")]);
                }
                let upstream =
                    futures_util::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
                let stream = if override_model {
                    super::super::streaming::stream_with_logging_and_model_override_semantic_timeout(upstream, context, log, "alias".into(), tracker, None).boxed()
                } else {
                    super::super::streaming::stream_with_logging(upstream, context, log, tracker)
                        .boxed()
                };
                let values = payloads(
                    stream
                        .map(|item| item.expect("error SSE frame"))
                        .collect()
                        .await,
                );
                assert!(
                    values.iter().any(|v| v["error"]["message"]
                        .as_str()
                        .is_some_and(|m| m.contains("finish_reason"))),
                    "{values:?}"
                );
                wait_for_log_rows(&pool, 1).await;
                let row = sqlx::query("SELECT status,response_error FROM request_logs")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(row.get::<i64, _>("status"), 502);
                assert!(row
                    .get::<String, _>("response_error")
                    .contains("finish_reason"));
            }
        }
    });
}

#[test]
fn cpa_chat_normal_eof_retains_tail_usage() {
    run_async(async {
        let (values, pool) = converted(vec![
            event(json!({"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]})),
            event(json!({"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}})),
        ], false).await;
        let terminal = values
            .iter()
            .find(|v| v["type"] == "response.completed")
            .unwrap();
        assert_eq!(terminal["response"]["usage"]["total_tokens"], 10);
        assert_eq!(
            read_first_usage_tokens(&pool).await,
            (Some(7), Some(3), Some(10))
        );
    });
}

#[test]
fn cpa_raw_chat_normal_eof_retains_tail_usage() {
    run_async(async {
        let (log, mut context, pool) = setup_responses_stream().await;
        context.path = "/v1/chat/completions".into();
        context.provider = "openai".into();
        let tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let upstream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(event(
                json!({"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}),
            )),
            Ok(event(
                json!({"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}),
            )),
        ]);
        let values = payloads(
            super::super::streaming::stream_with_logging(upstream, context, log, tracker)
                .map(|item| item.unwrap())
                .collect()
                .await,
        );
        assert!(!values.iter().any(|value| value.get("error").is_some()));
        assert_eq!(values.last().unwrap()["usage"]["total_tokens"], 10);
        assert_eq!(
            read_first_usage_tokens(&pool).await,
            (Some(7), Some(3), Some(10))
        );
    });
}

#[test]
fn cpa_chat_explicit_upstream_error_is_never_replaced_by_truncation() {
    run_async(async {
        for route in 0..3 {
            let (log, mut context, pool) = setup_responses_stream().await;
            context.provider = "openai".into();
            context.path = if route == 0 {
                "/v1/responses"
            } else {
                "/v1/chat/completions"
            }
            .into();
            let tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let upstream = futures_util::stream::iter(vec![
                Ok::<_, std::io::Error>(event(
                    json!({"choices":[{"delta":{"content":"partial"}}]}),
                )),
                Ok(event(
                    json!({"error":{"message":"provider quota exhausted","type":"rate_limit_error","code":"rate_limit_exceeded"}}),
                )),
            ]);
            let stream = match route {
                0 => super::super::chat_to_responses::stream_chat_to_responses(upstream,context,log,tracker).boxed(),
                1 => super::super::streaming::stream_with_logging(upstream,context,log,tracker).boxed(),
                _ => super::super::streaming::stream_with_logging_and_model_override_semantic_timeout(upstream,context,log,"alias".into(),tracker,None).boxed(),
            };
            let values = payloads(stream.map(|item| item.unwrap()).collect().await);
            let serialized = serde_json::to_string(&values).unwrap();
            assert!(serialized.contains("provider quota exhausted"));
            assert!(!serialized.contains("truncated"), "{values:?}");
            let terminal = values.last().unwrap();
            let error = if route == 0 {
                &terminal["response"]["error"]
            } else {
                &terminal["error"]
            };
            assert_eq!(error["code"], "rate_limit_exceeded");
            wait_for_log_rows(&pool, 1).await;
            let row = sqlx::query("SELECT status,response_error FROM request_logs")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(row.get::<i64, _>("status"), 429);
            assert!(row
                .get::<String, _>("response_error")
                .contains("provider quota exhausted"));
        }
    });
}

#[test]
fn cpa_reasoning_transport_failure_closes_as_interrupted_before_failed() {
    run_async(async {
        for anthropic in [false, true] {
            let (log, context, _) = setup_responses_stream().await;
            let tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let first = if anthropic {
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"partial"}})
            } else {
                json!({"choices":[{"delta":{"reasoning_content":"partial"},"finish_reason":null}]})
            };
            let upstream = futures_util::stream::iter(vec![
                Ok(event(first)),
                Err(std::io::Error::other("disconnected")),
            ]);
            let stream = if anthropic {
                super::super::anthropic_to_responses::stream_anthropic_to_responses(
                    upstream, context, log, tracker,
                )
                .boxed()
            } else {
                super::super::chat_to_responses::stream_chat_to_responses(
                    upstream, context, log, tracker,
                )
                .boxed()
            };
            let values = payloads(
                stream
                    .map(|item| item.expect("failure event"))
                    .collect()
                    .await,
            );
            assert_reasoning_lifecycle(&values, 1, true);
            assert_eq!(values.last().unwrap()["type"], "response.failed");
            assert!(values.last().unwrap()["response"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("disconnected"));
        }
    });
}

#[test]
fn cpa_chat_reasoning_lifecycle_closes_interrupted_and_signature_only_items() {
    run_async(async {
        for finish in ["stop", "length"] {
            let (values, _) = converted(vec![
                event(json!({"choices":[{"delta":{"reasoning_content":"思考"},"finish_reason":null}]})),
                event(json!({"choices":[{"delta":{},"finish_reason":finish}]})),
                Bytes::from("data: [DONE]\n\ndata: [DONE]\n\n"),
            ], false).await;
            assert_reasoning_lifecycle(&values, 1, finish == "length");
        }
        let (values, _) = converted(vec![event(json!({"choices":[{"delta":{"thinking_blocks":[{"type":"redacted_thinking","data":"signature"}]},"finish_reason":"stop"}]}))], false).await;
        assert_reasoning_lifecycle(&values, 0, false);
        let item = values
            .iter()
            .find(|v| v["type"] == "response.output_item.done")
            .unwrap();
        assert_eq!(item["item"]["summary"], json!([]));
        assert_eq!(item["item"]["encrypted_content"], "signature");
    });
}

#[test]
fn cpa_chat_multiple_reasoning_blocks_keep_distinct_item_identity() {
    run_async(async {
        let (values, _) = converted(vec![
            event(json!({"choices":[{"delta":{"reasoning_content":"first"},"finish_reason":null}]})),
            event(json!({"choices":[{"delta":{"content":"answer"},"finish_reason":null}]})),
            event(json!({"choices":[{"delta":{"reasoning_content":"second"},"finish_reason":"stop"}]})),
        ], false).await;
        assert_reasoning_lifecycle(&values, 2, false);
    });
}

#[test]
fn cpa_anthropic_pause_turn_and_multiple_reasoning_blocks_preserve_incomplete() {
    run_async(async {
        let mut chunks = vec![event(
            json!({"type":"message_start","message":{"usage":{"input_tokens":7}}}),
        )];
        for index in 0..2 {
            chunks.push(event(json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":""}})));
            chunks.push(event(json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":format!("step {index}")}})));
            chunks.push(event(json!({"type":"content_block_stop","index":index})));
        }
        chunks.extend([
            event(json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_1","name":"lookup","input":{"city":"北京"}}})),
            event(json!({"type":"message_delta","delta":{"stop_reason":"pause_turn"},"usage":{"output_tokens":3}})),
            event(json!({"type":"message_delta","delta":{},"usage":{"output_tokens":3}})),
            event(json!({"type":"message_stop"})),
            event(json!({"type":"message_stop"})),
        ]);
        let (values, _) = converted(chunks, true).await;
        let terminal = values
            .iter()
            .find(|v| v["type"] == "response.incomplete")
            .expect("pause_turn is incomplete");
        assert_eq!(terminal["response"]["incomplete_details"], Value::Null);
        assert_eq!(terminal["response"]["usage"]["total_tokens"], 10);
        assert_eq!(
            terminal["response"]["output"][2]["arguments"],
            "{\"city\":\"北京\"}"
        );
        assert_reasoning_lifecycle(&values, 2, false);
        let body = crate::proxy::anthropic_compat::anthropic_response_to_responses(&event_body(json!({"id":"msg_1","model":"claude","stop_reason":"pause_turn","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{"city":"北京"}}],"usage":{"input_tokens":7,"output_tokens":3}}))).unwrap();
        let response: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(response["status"], "incomplete");
        assert_eq!(response["incomplete_details"], Value::Null);
        assert_eq!(response["output"][0]["call_id"], "call_1");
    });
}

fn event_body(value: Value) -> Bytes {
    Bytes::from(value.to_string())
}

#[test]
fn cpa_chat_cancel_is_not_upstream_truncation() {
    run_async(async {
        let (log, mut context, pool) = setup_responses_stream().await;
        context.provider = "openai".into();
        let tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let upstream = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
        let mut stream = super::super::chat_to_responses::stream_chat_to_responses(
            upstream, context, log, tracker,
        )
        .boxed();
        stream
            .next()
            .await
            .expect("created event")
            .expect("created frame");
        drop(stream);
        wait_for_log_rows(&pool, 1).await;
        let row = sqlx::query("SELECT response_error FROM request_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row.get::<String, _>("response_error"),
            super::super::streaming::STREAM_DROPPED_ERROR
        );
    });
}

#[test]
fn cpa_anthropic_interrupted_reasoning_and_signature_only_have_correct_parts() {
    run_async(async {
        let (values, _) = converted(vec![
            event(json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),
            event(json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"partial"}})),
            event(json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}})),
            event(json!({"type":"message_stop"})),
        ], true).await;
        assert_reasoning_lifecycle(&values, 1, true);
        let (values, _) = converted(vec![
            event(json!({"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"signature"}})),
            event(json!({"type":"content_block_stop","index":0})),
            event(json!({"type":"message_stop"})),
        ], true).await;
        assert_reasoning_lifecycle(&values, 0, false);
        assert!(values
            .iter()
            .filter(|v| v["type"] == "response.output_item.done")
            .all(|v| v["item"]["summary"] == json!([])));
    });
}
