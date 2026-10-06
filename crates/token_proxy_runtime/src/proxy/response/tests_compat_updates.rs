//! 上游兼容修复的实际流式出口回归，检查客户端收到的内容及内容块生命周期。
use super::*;
use crate::proxy::{codex_compat, gemini_compat, token_rate::TokenRateTracker};
use futures_util::stream::BoxStream;

#[derive(Clone, Copy, Debug)]
enum Route {
    Chat,
    Codex,
    Anthropic,
    ChatResponses,
    ChatAnthropic,
    GeminiChat,
    GeminiResponses,
}

async fn convert(events: Vec<Value>, route: Route) -> Vec<Value> {
    let (log, context, _) = setup_responses_stream().await;
    let tracker = TokenRateTracker::new().register(None, None).await;
    // 将 UTF-8 和 SSE 边界分散到网络 chunk，避免回归只覆盖整帧输入。
    let bytes = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let chunks = bytes
        .as_bytes()
        .chunks(7)
        .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let upstream = futures_util::stream::iter(chunks);
    let converted: BoxStream<'static, Result<Bytes, std::io::Error>> = match route {
        Route::Chat => {
            responses_to_chat::stream_responses_to_chat(upstream, context, log, tracker).boxed()
        }
        Route::Codex => codex_compat::stream_codex_to_chat(upstream, context, log, tracker).boxed(),
        Route::Anthropic => {
            responses_to_anthropic::stream_responses_to_anthropic(upstream, context, log, tracker)
                .boxed()
        }
        Route::ChatResponses => stream_chat_to_responses(upstream, context, log, tracker).boxed(),
        Route::ChatAnthropic => {
            let responses = stream_chat_to_responses(
                upstream,
                context.clone(),
                Arc::new(LogWriter::new(None)),
                crate::proxy::token_rate::RequestTokenTracker::disabled(),
            )
            .boxed();
            responses_to_anthropic::stream_responses_to_anthropic(responses, context, log, tracker)
                .boxed()
        }
        Route::GeminiChat => {
            gemini_compat::stream_gemini_to_chat(upstream, context, log, tracker).boxed()
        }
        Route::GeminiResponses => {
            let chat = gemini_compat::stream_gemini_to_chat(
                upstream,
                context.clone(),
                Arc::new(LogWriter::new(None)),
                crate::proxy::token_rate::RequestTokenTracker::disabled(),
            )
            .boxed();
            stream_chat_to_responses(chat, context, log, tracker).boxed()
        }
    };
    let mut parser = crate::proxy::sse::SseEventParser::new();
    let mut result = Vec::new();
    for chunk in converted.collect::<Vec<_>>().await {
        parser.push_chunk(&chunk.expect("converted stream"), |data| {
            if data != "[DONE]" {
                result.push(serde_json::from_str::<Value>(&data).unwrap());
            }
        });
    }
    result
}

fn message(text: &str) -> Value {
    json!({"id":"m","type":"message","role":"assistant","content":[{"type":"output_text","text":text}]})
}

#[test]
fn p1_text_recovery_across_all_three_stream_exits() {
    run_async(async {
        let cases = vec![
            (
                vec![json!({"type":"response.output_text.done","item_id":"m","text":"你好"})],
                "你好",
            ),
            (
                vec![
                    json!({"type":"response.output_item.done","output_index":0,"item":message("你好")}),
                ],
                "你好",
            ),
            (
                vec![
                    json!({"type":"response.completed","response":{"status":"completed","output":[message("你好")]}}),
                ],
                "你好",
            ),
            (
                vec![
                    json!({"type":"response.output_text.delta","output_index":0,"delta":"你"}),
                    json!({"type":"response.output_text.done","item_id":"m","output_index":0,"text":"你好"}),
                    json!({"type":"response.output_text.done","item_id":"m","output_index":0,"text":"你好"}),
                    json!({"type":"response.completed","response":{"status":"completed","output":[{"id":"m","type":"message","role":"assistant","content":[{"type":"output_text","text":"你好"},{"type":"output_text","text":"世界"}]}]}}),
                    json!({"type":"response.output_text.delta","item_id":"m","delta":"late"}),
                ],
                "你好世界",
            ),
            (
                vec![
                    json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[message("部分")]}}),
                ],
                "部分",
            ),
        ];
        for route in [Route::Chat, Route::Codex, Route::Anthropic] {
            for (events, expected) in &cases {
                let payloads = convert(events.clone(), route).await;
                let text = payloads
                    .iter()
                    .filter_map(|event| match route {
                        Route::Anthropic => event.pointer("/delta/text").and_then(Value::as_str),
                        _ => event
                            .pointer("/choices/0/delta/content")
                            .and_then(Value::as_str),
                    })
                    .collect::<String>();
                assert_eq!(&text, expected, "route {route:?}");
                if *expected == "部分" {
                    assert!(
                        payloads
                            .iter()
                            .any(|event| event.pointer("/delta/stop_reason")
                                == Some(&json!("max_tokens"))
                                || event.pointer("/choices/0/finish_reason")
                                    == Some(&json!("length"))),
                        "{route:?}: {payloads:?}"
                    );
                }
            }
        }
    });
}

fn tool(id: &str, arguments: &str) -> Value {
    json!({"id":id,"type":"function_call","call_id":format!("call_{id}"),"name":format!("tool_{id}"),"arguments":arguments})
}

// 每个块必须 start -> delta* -> stop；不能重开索引或向关闭的块追加。
fn assert_blocks(events: &[Value]) -> Vec<(String, String)> {
    let mut active = None;
    let mut seen = std::collections::HashSet::new();
    let mut tools = Vec::<(String, String)>::new();
    let mut active_tool = None;
    for event in events {
        let index = event["index"].as_u64();
        match event["type"].as_str() {
            Some("content_block_start") => {
                assert!(active.is_none(), "overlapping blocks: {event}");
                assert!(seen.insert(index.unwrap()), "reopened block: {event}");
                active = index;
                active_tool = if event["content_block"]["type"] == "tool_use" {
                    tools.push((
                        event["content_block"]["id"].as_str().unwrap().to_string(),
                        String::new(),
                    ));
                    Some(tools.len() - 1)
                } else {
                    None
                };
            }
            Some("content_block_delta") => {
                assert!(active.is_some());
                assert_eq!(active, index, "delta after closed block: {event}");
                if let Some(partial) = event["delta"]["partial_json"].as_str() {
                    tools[active_tool.expect("tool start")].1.push_str(partial);
                }
            }
            Some("content_block_stop") => {
                assert!(active.is_some());
                assert_eq!(active.take(), index, "stop without start: {event}");
                active_tool = None;
            }
            _ => {}
        }
    }
    assert!(active.is_none());
    tools
}

#[test]
fn p1_interleaved_tools_keep_complete_arguments_and_sequential_blocks() {
    run_async(async {
        for ending in ["eof", "done", "terminal"] {
            let mut events = vec![
                json!({"type":"response.output_item.added","item":tool("a", "")}),
                json!({"type":"response.function_call_arguments.delta","item_id":"a","delta":"{\"a\":"}),
                json!({"type":"response.output_item.added","item":tool("b", "")}),
                json!({"type":"response.function_call_arguments.delta","item_id":"b","delta":"{\"b\":2}"}),
                json!({"type":"response.output_text.done","item_id":"m","text":"正文"}),
                json!({"type":"response.reasoning_summary_text.delta","delta":"推理"}),
                json!({"type":"response.function_call_arguments.delta","item_id":"a","delta":"1}"}),
            ];
            if ending == "done" {
                events.push(json!({"type":"response.function_call_arguments.done","item_id":"a","arguments":"{\"a\":1}"}));
                events.push(json!({"type":"response.function_call_arguments.done","item_id":"b","arguments":"{\"b\":2}"}));
            }
            if ending == "terminal" {
                events.push(json!({"type":"response.completed","response":{"status":"completed","output":[tool("a", "{\"a\":1}"),tool("b", "{\"b\":2}")]}}));
            } // EOF 也应顺序排空已收完整参数。
            let payloads = convert(events, Route::Anthropic).await;
            assert_eq!(
                assert_blocks(&payloads),
                vec![
                    ("call_a".into(), "{\"a\":1}".into()),
                    ("call_b".into(), "{\"b\":2}".into())
                ]
            );
            assert!(payloads
                .iter()
                .any(|event| event["delta"]["text"] == "正文"));
            assert!(payloads
                .iter()
                .any(|event| event["delta"]["thinking"] == "推理"));
            assert!(payloads
                .iter()
                .any(|event| event["delta"]["stop_reason"] == "tool_use"));
        }
    });
}

#[test]
fn p1_tool_done_snapshots_recover_suffix_once_and_ignore_late_deltas() {
    run_async(async {
        let events = vec![
            json!({"type":"response.output_item.added","item":tool("a", "")}),
            json!({"type":"response.function_call_arguments.delta","item_id":"a","delta":"{\"a\":"}),
            json!({"type":"response.function_call_arguments.done","item_id":"a","arguments":"{\"a\":1}"}),
            json!({"type":"response.function_call_arguments.delta","item_id":"a","delta":"late"}),
            json!({"type":"response.output_item.done","item":tool("a", "{\"a\":1}")}),
            json!({"type":"response.output_item.done","item":tool("b", "{}")}),
        ];
        assert_eq!(
            assert_blocks(&convert(events, Route::Anthropic).await),
            vec![
                ("call_a".into(), "{\"a\":1}".into()),
                ("call_b".into(), "{}".into())
            ]
        );
    });
}

#[test]
fn p1_truncated_or_non_object_tool_arguments_never_finish_as_complete() {
    run_async(async {
        for args in ["{\"x\":", "[]", "42", " ", "{}", ""] {
            for finish in [
                None,
                Some("tool_calls"),
                Some("stop"),
                Some("length"),
                Some("content_filter"),
            ] {
                let events = vec![
                    json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"tool_a","arguments":args}}]},"finish_reason":finish}]}),
                ];
                let valid = args == "{}"
                    || (args.is_empty() && matches!(finish, Some("tool_calls" | "stop")));
                let incomplete = !valid || matches!(finish, Some("length" | "content_filter"));
                let responses = convert(events.clone(), Route::ChatResponses).await;
                assert!(
                    responses.iter().any(|event| event["type"]
                        == if finish.is_none() {
                            "response.failed"
                        } else if incomplete {
                            "response.incomplete"
                        } else {
                            "response.completed"
                        }),
                    "args={args:?}, finish={finish:?}: {responses:?}"
                );
                let anthropic = convert(events, Route::ChatAnthropic).await;
                assert_blocks(&anthropic);
                if finish.is_none() {
                    assert!(anthropic.iter().any(|event| event["type"] == "error"));
                    continue;
                }
                let expected = if finish == Some("content_filter") {
                    "refusal"
                } else if incomplete {
                    "max_tokens"
                } else {
                    "tool_use"
                };
                assert!(
                    anthropic
                        .iter()
                        .any(|event| event["delta"]["stop_reason"] == expected),
                    "args={args:?}, finish={finish:?}: {anthropic:?}"
                );
            }
            let native = convert(
                vec![json!({"type":"response.output_item.done","item":tool("a", args)})],
                Route::Anthropic,
            )
            .await;
            assert_blocks(&native);
            let expected = if args == "{}" || args.is_empty() {
                "tool_use"
            } else {
                "max_tokens"
            };
            assert!(native
                .iter()
                .any(|event| event["delta"]["stop_reason"] == expected));
        }
    });
}

#[test]
fn p1_gemini_usage_after_finish_survives_chat_and_responses_bridges() {
    run_async(async {
        let events = vec![
            json!({"candidates":[{"content":{"parts":[{"text":"answer"}]},"finishReason":"STOP"}]}),
            json!({"usageMetadata":{"promptTokenCount":16,"candidatesTokenCount":5,"thoughtsTokenCount":42,"totalTokenCount":63,"cachedContentTokenCount":3}}),
        ];
        for route in [Route::GeminiChat, Route::GeminiResponses] {
            let payloads = convert(events.clone(), route).await;
            let usage = payloads
                .iter()
                .find_map(|event| {
                    event
                        .get("usage")
                        .or_else(|| event.pointer("/response/usage"))
                        .filter(|value| value.is_object())
                })
                .unwrap();
            match route {
                Route::GeminiChat => {
                    assert_eq!(usage["completion_tokens"], 47);
                    assert_eq!(usage["completion_tokens_details"]["reasoning_tokens"], 42);
                    assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 3);
                }
                Route::GeminiResponses => {
                    assert_eq!(usage["output_tokens"], 47);
                    assert_eq!(usage["output_tokens_details"]["reasoning_tokens"], 42);
                    assert_eq!(usage["input_tokens_details"]["cached_tokens"], 3);
                }
                _ => unreachable!(),
            }
            assert_eq!(usage["total_tokens"], 63);
        }
    });
}

#[test]
fn gemini_partial_usage_matches_client_and_persistence() {
    run_async(async {
        for zero_tail in [false, true] {
            for responses in [false, true] {
                let (log, context, pool) = setup_responses_stream().await;
                let mut events = vec![
                    json!({"candidates":[{"content":{"parts":[{"text":"answer"}]},"finishReason":"STOP"}]}),
                    json!({"usageMetadata":{"promptTokenCount":16,"cachedContentTokenCount":4}}),
                    json!({"usageMetadata":{"candidatesTokenCount":5}}),
                    json!({"usageMetadata":{"thoughtsTokenCount":42,"totalTokenCount":63}}),
                    json!({"usageMetadata":{"thoughtsTokenCount":42,"totalTokenCount":63}}),
                ];
                if zero_tail {
                    events.push(json!({"usageMetadata":{
                        "candidatesTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":58
                    }}));
                }
                let upstream = futures_util::stream::iter(events.into_iter().map(|event| {
                    Ok::<_, std::io::Error>(Bytes::from(format!("data: {event}\n\n")))
                }));
                let chat = gemini_compat::stream_gemini_to_chat(
                    upstream,
                    context.clone(),
                    if responses {
                        Arc::new(LogWriter::new(None))
                    } else {
                        log.clone()
                    },
                    crate::proxy::token_rate::RequestTokenTracker::disabled(),
                )
                .boxed();
                let converted: BoxStream<'static, Result<Bytes, std::io::Error>> = if responses {
                    stream_chat_to_responses(
                        chat,
                        context,
                        log,
                        crate::proxy::token_rate::RequestTokenTracker::disabled(),
                    )
                    .boxed()
                } else {
                    chat
                };
                let events = converted
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .filter_map(|chunk| parse_sse_json(&chunk.expect("converted stream")))
                    .collect::<Vec<_>>();
                let usage = events
                    .iter()
                    .find_map(|event| {
                        event
                            .get("usage")
                            .or_else(|| event.pointer("/response/usage"))
                            .filter(|usage| usage.is_object())
                    })
                    .expect("final client usage");
                let output = if zero_tail { 42 } else { 47 };
                let total = if zero_tail { 58 } else { 63 };
                let cached = if zero_tail { 0 } else { 4 };
                if responses {
                    assert_eq!(usage["input_tokens"], 16);
                    assert_eq!(usage["output_tokens"], output);
                    assert_eq!(usage["input_tokens_details"]["cached_tokens"], cached);
                    assert_eq!(usage["output_tokens_details"]["reasoning_tokens"], 42);
                } else {
                    assert_eq!(usage["prompt_tokens"], 16);
                    assert_eq!(usage["completion_tokens"], output);
                    assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], cached);
                    assert_eq!(usage["completion_tokens_details"]["reasoning_tokens"], 42);
                }
                assert_eq!(usage["total_tokens"], total);
                assert_eq!(
                    read_first_usage_tokens(&pool).await,
                    (Some(16), Some(output), Some(total))
                );
            }
        }
    });
}

#[test]
fn p1_active_tool_delta_is_forwarded_before_upstream_finishes() {
    run_async(async {
        let (log, context, _) = setup_responses_stream().await;
        let events = [
            json!({"type":"response.output_item.added","item":tool("a", "")}),
            json!({"type":"response.function_call_arguments.delta","item_id":"a","delta":"{"}),
        ];
        let upstream = futures_util::stream::iter(
            events
                .into_iter()
                .map(|event| Ok::<_, std::io::Error>(Bytes::from(format!("data: {event}\n\n")))),
        )
        .chain(futures_util::stream::pending());
        let converted = responses_to_anthropic::stream_responses_to_anthropic(
            upstream,
            context,
            log,
            crate::proxy::token_rate::RequestTokenTracker::disabled(),
        );
        let chunks = tokio::time::timeout(
            Duration::from_secs(1),
            converted.take(3).collect::<Vec<_>>(),
        )
        .await
        .expect("must stream before EOF");
        let (_, delta) = parse_anthropic_sse(chunks[2].as_ref().expect("chunk")).expect("event");
        assert_eq!(delta["delta"]["partial_json"], "{");
    });
}

#[test]
fn gemini_stream_keeps_thoughts_out_of_text_and_round_trips_function_signature() {
    run_async(async {
        let events = vec![
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"thinking","thought":true}]}}]}),
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"final"}]}}]}),
            json!({"candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"name":"lookup","args":{"q":"x"}},"thoughtSignature":"sig_fc"}
            ]},"finishReason":"STOP"}]}),
        ];

        let chat = convert(events.clone(), Route::GeminiChat).await;
        let text: String = chat
            .iter()
            .filter_map(|event| event["choices"][0]["delta"]["content"].as_str())
            .collect();
        let reasoning: String = chat
            .iter()
            .filter_map(|event| event["choices"][0]["delta"]["reasoning_content"].as_str())
            .collect();
        assert_eq!(text, "final");
        assert_eq!(reasoning, "thinking");

        let responses = convert(events, Route::GeminiResponses).await;
        let call = responses
            .iter()
            .find(|event| {
                event["type"] == json!("response.output_item.done")
                    && event["item"]["type"] == json!("function_call")
            })
            .map(|event| event["item"].clone())
            .expect("function call item");
        assert_eq!(
            call["provider_specific_fields"],
            json!({"provider":"gemini","thought_signature":"sig_fc"})
        );

        // 下一轮客户端回放该调用时，签名必须回到 Gemini functionCall part。
        let replay = Bytes::from(
            json!({
                "model": "gemini-3-pro",
                "input": [
                    {"type":"message","role":"user","content":"hi"},
                    call,
                    {"type":"function_call_output","call_id":call["call_id"],"output":"ok"}
                ]
            })
            .to_string(),
        );
        let gemini = crate::proxy::openai_compat::transform_request_body(
            crate::proxy::openai_compat::FormatTransform::ResponsesToGemini,
            &replay,
            &crate::proxy::http_client::ProxyHttpClients::new().expect("http clients"),
            None,
        )
        .await
        .expect("responses to gemini");
        let gemini: Value = serde_json::from_slice(&gemini).unwrap();
        let signed = gemini["contents"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|content| content["parts"].as_array().into_iter().flatten())
            .find(|part| part.get("functionCall").is_some())
            .expect("functionCall part");
        assert_eq!(signed["thoughtSignature"], json!("sig_fc"));
    });
}
