use axum::body::Bytes;
use futures_util::{future, StreamExt};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::super::{
    log::{LogContext, LogWriter},
    token_rate::TokenRateTracker,
};
use super::stream::is_codex_business_output_event;
use super::tool_names::shorten_name_if_needed;
use super::{
    chat_request_to_codex, codex_response_to_chat, codex_response_to_responses,
    responses_compact_request_to_codex, responses_request_to_codex,
    responses_request_to_codex_with_prompt_cache_key, stream_codex_to_chat,
    stream_codex_to_responses, stream_codex_to_responses_with_semantic_timeout,
    supported_codex_model_ids,
};

#[test]
fn chat_request_to_codex_sets_model_and_stream() {
    let input = json!({
        "model": "gpt-5",
        "stream": true,
        "messages": [
            { "role": "user", "content": "hi" }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = chat_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(value["model"], "gpt-5-codex");
    assert_eq!(value["stream"], true);
    assert_eq!(value["input"][0]["type"], "message");
    assert!(
        value["prompt_cache_key"]
            .as_str()
            .is_some_and(|key| !key.is_empty()),
        "prompt_cache_key should be present for Codex responses"
    );
}

#[test]
fn chat_request_to_codex_forces_upstream_streaming() {
    let input = json!({
        "model": "gpt-5",
        "stream": false,
        "messages": [
            { "role": "user", "content": "hi" }
        ]
    });

    let output = chat_request_to_codex(&Bytes::from(input.to_string()), Some("gpt-5-codex"))
        .expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["stream"], true);
}

#[test]
fn chat_request_to_codex_accepts_responses_shaped_body() {
    let input = json!({
        "model": "openai/gpt 5.5",
        "stream": false,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "hi" }]
            }
        ],
        "metadata": { "client": "cursor" },
        "stream_options": { "include_usage": true },
        "prompt_cache_retention": "24h",
        "safety_identifier": "sid_1"
    });

    let output = chat_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses-shaped chat request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["model"], "gpt-5.5");
    assert_eq!(value["stream"], true);
    assert_eq!(value["store"], false);
    assert_eq!(
        value["instructions"],
        "You are GPT-5.1 running in the Codex CLI, a terminal-based coding assistant."
    );
    assert_eq!(value["input"][0]["role"], "user");
    assert!(value.get("messages").is_none());
    assert!(value.get("metadata").is_none());
    assert!(value.get("stream_options").is_none());
    assert!(value.get("prompt_cache_retention").is_none());
    assert!(value.get("safety_identifier").is_none());
}

#[test]
fn chat_request_to_codex_rejects_missing_messages_without_input() {
    let input = json!({ "model": "gpt-5" });
    let error = chat_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect_err("should reject malformed chat request");

    assert!(error.contains("messages"), "error: {error}");
}

#[test]
fn responses_request_to_codex_normalizes_gpt_5_5_and_sanitizes_oauth_payload() {
    let input = json!({
        "model": "openai/gpt 5.5",
        "stream": false,
        "store": true,
        "frequency_penalty": 0.2,
        "presence_penalty": 0.3,
        "prompt_cache_retention": "24h",
        "input": [
            {
                "type": "message",
                "role": "system",
                "content": [
                    { "type": "input_text", "text": "system rules" }
                ]
            },
            {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "hi" }
                ]
            }
        ]
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["model"], "gpt-5.5");
    assert_eq!(value["stream"], true);
    assert_eq!(value["store"], false);
    assert_eq!(value["instructions"], "system rules");
    assert_eq!(value["input"].as_array().expect("input").len(), 2);
    assert_eq!(value["input"][0]["role"], "developer");
    assert_eq!(value["input"][1]["role"], "user");
    assert!(
        value["prompt_cache_key"]
            .as_str()
            .is_some_and(|key| !key.is_empty()),
        "prompt_cache_key should be present for Codex responses"
    );
    assert!(value.get("frequency_penalty").is_none());
    assert!(value.get("presence_penalty").is_none());
    assert!(value.get("prompt_cache_retention").is_none());
}

#[test]
fn responses_request_to_codex_preserves_gpt_5_5_pro_model() {
    for incoming_model in ["openai/gpt5.5-pro", "gpt-5.5-pro-high"] {
        let input = json!({
            "model": incoming_model,
            "input": "hi"
        });

        let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
            .expect("convert responses request");
        let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

        assert_eq!(value["model"], "gpt-5.5-pro", "model={incoming_model}");
    }
}

#[test]
fn responses_request_to_codex_prefers_model_hint_over_body_model() {
    let input = json!({
        "model": "claude-3-5-sonnet",
        "input": "hi"
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), Some("gpt-5.5"))
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["model"], "gpt-5.5");
}

#[test]
fn responses_request_to_codex_preserves_gpt_5_6_models() {
    let cases = [
        ("gpt-5.6", "gpt-5.6-sol"),
        ("gpt-5.6-max", "gpt-5.6-sol"),
        ("gpt-5.6-high", "gpt-5.6-sol"),
        ("openai/gpt-5.6-sol", "gpt-5.6-sol"),
        ("gpt-5.6-terra-high", "gpt-5.6-terra"),
        ("gpt-5.6-luna-xhigh", "gpt-5.6-luna"),
    ];

    for (incoming_model, expected_model) in cases {
        let input = json!({
            "model": incoming_model,
            "input": "hi"
        });

        let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
            .expect("convert responses request");
        let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

        assert_eq!(value["model"], expected_model, "model={incoming_model}");
    }
}

#[test]
fn supported_codex_models_include_current_codex_families() {
    let models = supported_codex_model_ids();

    assert!(models.contains(&"gpt-5.6".to_string()));
    assert!(models.contains(&"gpt-5.6-max".to_string()));
    assert!(models.contains(&"gpt-5.6-sol".to_string()));
    assert!(models.contains(&"gpt-5.6-sol-high".to_string()));
    assert!(models.contains(&"gpt-5.6-terra".to_string()));
    assert!(models.contains(&"gpt-5.6-terra-high".to_string()));
    assert!(models.contains(&"gpt-5.6-luna".to_string()));
    assert!(models.contains(&"gpt-5.6-luna-xhigh".to_string()));
    assert!(models.contains(&"gpt-5.5-pro".to_string()));
    assert!(models.contains(&"gpt-5.5-pro-high".to_string()));
}

#[test]
fn responses_request_to_codex_preserves_system_input_as_developer() {
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            {
                "type": "message",
                "role": "system",
                "content": [
                    { "type": "input_text", "text": "system rules" }
                ]
            },
            {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "hi" }
                ]
            }
        ]
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["instructions"], "system rules");
    assert_eq!(value["input"].as_array().expect("input").len(), 2);
    assert_eq!(value["input"][0]["role"], "developer");
    assert_eq!(value["input"][1]["role"], "user");
}

#[test]
fn responses_request_to_codex_uses_model_aware_default_instructions() {
    for (model, expected) in [
        (
            "gpt-5.2",
            "You are GPT-5.2 running in the Codex CLI, a terminal-based coding assistant.",
        ),
        (
            "gpt-5",
            "You are GPT-5.1 running in the Codex CLI, a terminal-based coding assistant.",
        ),
        (
            "gpt-5-codex",
            "You are Codex, based on GPT-5. You are running as a coding agent in the Codex CLI on a user's computer.",
        ),
    ] {
        let input = json!({
            "model": model,
            "input": "hi"
        });

        let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
            .expect("convert responses request");
        let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

        assert_eq!(value["instructions"], expected, "model={model}");
    }
}

#[test]
fn responses_request_to_codex_preserves_prompt_cache_key() {
    let input = json!({
        "model": "gpt-5.5",
        "prompt_cache_key": "thread-from-client",
        "input": "hi"
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["prompt_cache_key"], "thread-from-client");
}

#[test]
fn responses_request_to_codex_uses_prompt_cache_key_hint_when_missing() {
    let input = json!({
        "model": "gpt-5.5",
        "input": "hi"
    });

    let output = responses_request_to_codex_with_prompt_cache_key(
        &Bytes::from(input.to_string()),
        None,
        Some("thread-from-header"),
    )
    .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["prompt_cache_key"], "thread-from-header");
}

#[test]
fn responses_compact_request_to_codex_normalizes_gpt_5_5_and_removes_stream_store() {
    let input = json!({
        "model": "gpt-5.5-medium",
        "stream": true,
        "store": true,
        "input": "hi"
    });

    let output = responses_compact_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert compact responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["model"], "gpt-5.5");
    assert_eq!(
        value["instructions"],
        "You are GPT-5.1 running in the Codex CLI, a terminal-based coding assistant."
    );
    assert_eq!(value["stream"], true);
    assert_eq!(value["store"], false);
    assert!(value.get("include").is_none());
}

#[test]
fn responses_request_to_codex_preserves_gpt_5_6_max_effort() {
    let input = json!({
        "model": "gpt-5.6-max",
        "input": "hi"
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["model"], "gpt-5.6-sol");
    assert_eq!(value["reasoning"]["effort"], "max");
}

#[test]
fn responses_compact_request_to_codex_downgrades_gpt_5_6_max_effort() {
    let input = json!({
        "model": "gpt-5.6-sol",
        "reasoning": { "effort": "max", "summary": "auto" },
        "input": "hi"
    });

    let output = responses_compact_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert compact responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["model"], "gpt-5.6-sol");
    assert_eq!(value["reasoning"]["effort"], "xhigh");
    assert_eq!(value["reasoning"]["summary"], "auto");
}

#[test]
fn responses_compact_request_to_codex_normalizes_openai_message_input() {
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            { "role": "user", "content": "hi" }
        ]
    });

    let output = responses_compact_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert compact responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["input"][0]["type"], "message");
    assert_eq!(value["input"][0]["role"], "user");
    assert_eq!(value["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(value["input"][0]["content"][0]["text"], "hi");
}

#[test]
fn codex_response_to_chat_restores_tool_name() {
    let original = "mcp__very_long_tool_name_for_codex_restoration_check_v1_tool_extra_long_suffix";
    let short = shorten_name_if_needed(original);
    assert!(short.len() <= 64);
    assert_ne!(short, original);

    let request_body = json!({
        "tools": [
            { "type": "function", "function": { "name": original } }
        ]
    })
    .to_string();

    let response = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_1",
            "created_at": 123,
            "model": "gpt-5",
            "status": "completed",
            "output": [
                { "type": "function_call", "call_id": "call_1", "name": short, "arguments": "{}" }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 2, "total_tokens": 3 }
        }
    });
    let bytes = Bytes::from(response.to_string());
    let output = codex_response_to_chat(&bytes, Some(&request_body)).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let name = value["choices"][0]["message"]["tool_calls"][0]["function"]["name"]
        .as_str()
        .expect("tool name");
    assert_eq!(name, original);
}

#[test]
fn codex_response_to_chat_restores_custom_tool_name_and_input() {
    let original = "custom_tool_name_for_codex_restoration_check_with_a_very_long_suffix";
    let short = shorten_name_if_needed(original);
    assert_ne!(short, original);

    let request_body = json!({
        "tools": [{ "type": "custom", "name": original }]
    })
    .to_string();
    let response = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_custom",
            "created_at": 123,
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [{
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": short,
                "input": "*** Begin Patch\n*** End Patch"
            }]
        }
    });

    let output = codex_response_to_chat(
        &Bytes::from(response.to_string()),
        Some(request_body.as_str()),
    )
    .expect("convert custom tool response");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let tool_call = &value["choices"][0]["message"]["tool_calls"][0];

    assert_eq!(tool_call["type"], json!("function"));
    assert_eq!(tool_call["function"]["name"], json!(original));
    assert_eq!(
        tool_call["function"]["arguments"],
        json!("*** Begin Patch\n*** End Patch")
    );
    assert_eq!(value["choices"][0]["finish_reason"], json!("tool_calls"));
}

#[test]
fn codex_response_to_chat_preserves_image_generation_call_result() {
    let response = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_img",
            "created_at": 123,
            "model": "gpt-5.5",
            "status": "completed",
            "output": [
                {
                    "type": "image_generation_call",
                    "id": "img_1",
                    "status": "completed",
                    "result": "BASE64PNG"
                }
            ]
        }
    });
    let bytes = Bytes::from(response.to_string());
    let output = codex_response_to_chat(&bytes, None).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let content = value["choices"][0]["message"]["content"]
        .as_str()
        .expect("content");

    assert!(content.contains("data:image/png;base64,BASE64PNG"));
}

#[test]
fn codex_response_to_responses_rejects_json_error_payload() {
    let bytes = Bytes::from(
        json!({
            "error": {
                "message": "rate limit exceeded",
                "type": "rate_limit_error"
            }
        })
        .to_string(),
    );

    let error = codex_response_to_responses(&bytes, None).expect_err("should fail");
    assert!(
        error.contains("rate limit exceeded"),
        "unexpected error: {error}"
    );
    assert!(error.contains("error payload"), "unexpected error: {error}");
}

#[test]
fn codex_response_to_responses_rejects_non_json_success_payload_with_raw_text() {
    let bytes = Bytes::from("upstream gateway said nope");

    let error = codex_response_to_responses(&bytes, None).expect_err("should fail");
    assert!(
        error.contains("non-JSON success payload"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("upstream gateway said nope"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn stream_codex_to_responses_emits_error_event_for_invalid_json_event() {
    let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        "data: not-json\n\n",
    ))]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(text.contains("event: response.failed"), "chunks: {text}");
    assert!(
        text.contains("\"type\":\"response.failed\""),
        "chunks: {text}"
    );
    assert!(text.contains("invalid JSON stream event"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
    assert!(
        text.contains("\"sequence_number\":0"),
        "first terminal event must start the Responses sequence: {text}"
    );
    let failed = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| line.contains("response.failed"))
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .expect("response.failed payload");
    assert!(failed["response"]["created_at"].as_i64().is_some());
    assert_eq!(failed["response"]["model"], json!("unknown"));
}

#[tokio::test]
async fn stream_codex_to_responses_emits_compatible_terminal_event_when_upstream_ends_early() {
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_early\",\"model\":\"gpt-5.4\"}}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial output\"}\n\n",
        )),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(
        text.contains("\"type\":\"response.completed\""),
        "chunks: {text}"
    );
    assert!(text.contains("\"status\":\"incomplete\""), "chunks: {text}");
    assert!(
        text.contains("\"incomplete_details\":{\"reason\":\"error\"}"),
        "chunks: {text}"
    );
    assert!(text.contains("partial output"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_responses_counts_reasoning_summary_delta_for_token_rate() {
    let (first_chunk, output_tokens) = first_codex_responses_output_tokens(
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"thinking out loud\"}\n\n",
    )
    .await;

    assert!(
        String::from_utf8_lossy(&first_chunk).contains("response.reasoning_summary_text.delta"),
        "chunk: {:?}",
        first_chunk
    );
    assert!(output_tokens > 0, "output_tokens: {output_tokens}");
}

#[tokio::test]
async fn stream_codex_to_responses_counts_official_output_delta_events_for_token_rate() {
    for event in [
        "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"private reasoning\"}\n\n",
        "data: {\"type\":\"response.refusal.delta\",\"delta\":\"cannot comply\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"city\\\":\"}\n\n",
        "data: {\"type\":\"response.mcp_call_arguments.delta\",\"item_id\":\"mcp_1\",\"output_index\":0,\"delta\":\"{\\\"query\\\":\"}\n\n",
        "data: {\"type\":\"response.custom_tool_call_input.delta\",\"item_id\":\"ctc_1\",\"output_index\":0,\"delta\":\"partial input\"}\n\n",
        "data: {\"type\":\"response.code_interpreter_call_code.delta\",\"item_id\":\"ci_1\",\"output_index\":0,\"delta\":\"print(1)\"}\n\n",
    ] {
        let (_, output_tokens) = first_codex_responses_output_tokens(event).await;

        assert!(output_tokens > 0, "event should count output tokens: {event}");
    }
}

#[tokio::test]
async fn stream_codex_to_responses_does_not_count_final_snapshots_for_token_rate() {
    for event in [
        "data: {\"type\":\"response.output_text.done\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"final text\"}\n\n",
        "data: {\"type\":\"response.reasoning_text.done\",\"item_id\":\"rs_1\",\"output_index\":0,\"content_index\":0,\"text\":\"final reasoning\"}\n\n",
        "data: {\"type\":\"response.custom_tool_call_input.done\",\"item_id\":\"ctc_1\",\"output_index\":0,\"input\":\"final input\"}\n\n",
        "data: {\"type\":\"response.code_interpreter_call_code.done\",\"item_id\":\"ci_1\",\"output_index\":0,\"code\":\"print(1)\"}\n\n",
        "data: {\"type\":\"response.mcp_call_arguments.done\",\"item_id\":\"mcp_1\",\"output_index\":0,\"arguments\":\"{\\\"query\\\":\\\"repo\\\"}\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_done\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"final text\"}]}]}}\n\n",
    ] {
        let (_, output_tokens) = first_codex_responses_output_tokens(event).await;

        assert_eq!(
            output_tokens, 0,
            "final snapshot should not count realtime output tokens: {event}"
        );
    }
}

#[tokio::test]
async fn stream_codex_to_responses_ignores_empty_delta_for_token_rate() {
    let (_, output_tokens) = first_codex_responses_output_tokens(
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"\"}\n\n",
    )
    .await;

    assert_eq!(output_tokens, 0);
}

#[tokio::test]
async fn stream_codex_to_responses_closes_after_terminal_without_upstream_close() {
    let upstream = futures_util::stream::unfold(0usize, |index| async move {
        if index == 0 {
            return Some((
                Ok::<Bytes, std::io::Error>(Bytes::from(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_done\",\"model\":\"gpt-5.4\",\"status\":\"completed\"}}\n\n",
                )),
                1,
            ));
        }
        future::pending::<Option<(Result<Bytes, std::io::Error>, usize)>>().await
    })
    .boxed();
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = tokio::time::timeout(
        Duration::from_millis(200),
        stream_codex_to_responses(upstream, context, log, tracker).collect::<Vec<_>>(),
    )
    .await
    .expect("stream should end after terminal Responses event");
    let text = join_stream_chunks(&chunks);

    assert!(
        text.contains("\"type\":\"response.completed\""),
        "chunks: {text}"
    );
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_responses_semantic_timeout_ignores_heartbeat_comments() {
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
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = tokio::time::timeout(
        Duration::from_millis(500),
        stream_codex_to_responses_with_semantic_timeout(
            upstream,
            context,
            log,
            tracker,
            Some(Duration::from_millis(40)),
        )
        .collect::<Vec<_>>(),
    )
    .await
    .expect("heartbeat-only Codex stream should not hang");
    let text = join_stream_chunks(&chunks);

    assert!(text.contains("event: response.failed"), "chunks: {text}");
    assert!(
        text.contains("\"type\":\"response.failed\""),
        "chunks: {text}"
    );
    assert!(text.contains("\"status\":\"failed\""), "chunks: {text}");
    assert!(text.contains("semantic timeout"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_responses_upstream_error_emits_response_failed_after_stream_started() {
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\",\"sequence_number\":7}\n\n",
        )),
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "codex stream reset",
        )),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(text.contains("event: response.failed"), "chunks: {text}");
    assert!(
        text.contains("\"type\":\"response.failed\""),
        "chunks: {text}"
    );
    assert!(text.contains("codex stream reset"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
    assert!(
        text.contains("\"sequence_number\":8"),
        "terminal event must continue the Responses sequence: {text}"
    );
}

#[tokio::test]
async fn stream_codex_to_responses_normalizes_upstream_response_failed() {
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\",\"sequence_number\":7}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"codex overloaded\"}}}\n\n",
        )),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);
    let failed = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| line.contains("response.failed"))
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .expect("response.failed payload");

    assert_eq!(failed["sequence_number"], json!(8));
    assert!(failed["response"]["created_at"].as_i64().is_some());
    assert_eq!(failed["response"]["model"], json!("gpt-5-codex"));
    assert_eq!(
        failed["response"]["error"]["message"],
        json!("codex overloaded")
    );
    assert!(text.contains("data: [DONE]"));
}

#[tokio::test]
async fn stream_codex_to_responses_accepts_canceled_terminal_event() {
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_cancel\",\"model\":\"gpt-5.5\"}}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.canceled\",\"response\":{\"id\":\"resp_cancel\",\"status\":\"cancelled\",\"model\":\"gpt-5.5\"}}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n")),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(
        text.contains("\"type\":\"response.canceled\""),
        "chunks: {text}"
    );
    assert!(text.contains("\"status\":\"cancelled\""), "chunks: {text}");
    assert!(
        !text.contains("Codex upstream stream disconnected before response.completed"),
        "chunks: {text}"
    );
    assert!(
        !text.contains("\"incomplete_details\":{\"reason\":\"error\"}"),
        "chunks: {text}"
    );
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_responses_restores_custom_tool_name() {
    let original = "custom_tool_name_for_responses_stream_restoration_with_long_suffix";
    let short = shorten_name_if_needed(original);
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(format!(
            "data: {}\n\n",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "custom_tool_call",
                    "call_id": "call_custom",
                    "name": short,
                    "input": "freeform input"
                }
            })
        ))),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_custom\",\"status\":\"completed\"}}\n\n",
        )),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let mut context = test_log_context();
    context.request_body =
        Some(json!({ "tools": [{ "type": "custom", "name": original }] }).to_string());
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_responses(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);
    let tool_event = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["type"] == "response.output_item.done")
        .expect("custom tool event");

    assert_eq!(tool_event["item"]["name"], json!(original));
}

#[tokio::test]
async fn stream_codex_to_chat_emits_error_event_for_invalid_json_event() {
    let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        "data: not-json\n\n",
    ))]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_chat(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(text.contains("\"error\":{"), "chunks: {text}");
    assert!(text.contains("invalid JSON stream event"), "chunks: {text}");
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_chat_emits_image_generation_call_result() {
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img\",\"model\":\"gpt-5.5\"}}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"id\":\"img_1\",\"status\":\"completed\",\"result\":\"BASE64PNG\"}}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img\",\"status\":\"completed\"}}\n\n",
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n")),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_chat(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(
        text.contains("data:image/png;base64,BASE64PNG"),
        "chunks: {text}"
    );
    assert!(text.contains("data: [DONE]"), "chunks: {text}");
}

#[tokio::test]
async fn stream_codex_to_chat_emits_custom_tool_call() {
    let original = "custom_tool_name_for_codex_stream_restoration_with_a_very_long_suffix";
    let short = shorten_name_if_needed(original);
    let upstream = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(format!(
            "data: {}\n\n",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "custom_tool_call",
                    "call_id": "call_custom",
                    "name": short,
                    "input": "freeform input"
                }
            })
        ))),
        Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_custom\",\"status\":\"completed\"}}\n\n",
        )),
    ]);
    let tracker = TokenRateTracker::new().register(None, None).await;
    let mut context = test_log_context();
    context.request_body =
        Some(json!({ "tools": [{ "type": "custom", "name": original }] }).to_string());
    let log = Arc::new(LogWriter::new(None));

    let chunks = stream_codex_to_chat(upstream, context, log, tracker)
        .collect::<Vec<_>>()
        .await;
    let text = join_stream_chunks(&chunks);

    assert!(text.contains(original), "chunks: {text}");
    assert!(text.contains("freeform input"), "chunks: {text}");
    assert!(
        text.contains("\"finish_reason\":\"tool_calls\""),
        "chunks: {text}"
    );
}

#[test]
fn custom_tool_call_is_codex_business_output() {
    assert!(is_codex_business_output_event(&json!({
        "type": "response.output_item.done",
        "item": { "type": "custom_tool_call" }
    })));
}

fn test_log_context() -> LogContext {
    LogContext {
        client_ip: None,
        path: "/v1/responses".to_string(),
        provider: "codex".to_string(),
        upstream_id: "test".to_string(),
        account_id: None,
        model: Some("gpt-5-codex".to_string()),
        mapped_model: None,
        stream: true,
        status: 200,
        upstream_request_id: None,
        request_headers: None,
        request_body: None,
        ttfb_ms: None,
        timings: Default::default(),
        start: Instant::now(),
    }
}

fn join_stream_chunks(chunks: &[Result<Bytes, std::io::Error>]) -> String {
    chunks
        .iter()
        .map(|chunk| chunk.as_ref().expect("stream chunk"))
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>()
        .join("")
}

async fn first_codex_responses_output_tokens(event: &str) -> (Bytes, u64) {
    let upstream = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        event.to_string(),
    ))]);
    let token_rate = TokenRateTracker::new();
    let tracker = token_rate.register(Some("gpt-5.5".to_string()), None).await;
    let context = test_log_context();
    let log = Arc::new(LogWriter::new(None));
    let stream = stream_codex_to_responses(upstream, context, log, tracker);
    futures_util::pin_mut!(stream);
    let first_chunk = stream.next().await.expect("stream item").expect("chunk");
    let snapshot = token_rate.snapshot().await;

    (first_chunk, snapshot.output)
}

#[test]
fn chat_request_to_codex_skips_missing_tool_names() {
    let input = json!({
        "model": "gpt-5",
        "messages": [
            { "role": "user", "content": "hi" }
        ],
        "tools": [
            { "type": "function", "function": { "description": "noop", "parameters": {} } }
        ],
        "tool_choice": { "type": "function", "function": {} }
    });
    let bytes = Bytes::from(input.to_string());
    let output = chat_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let tools = value["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);
    assert!(tools[0].get("name").is_none());
    let tool_choice = value["tool_choice"].as_object().expect("tool_choice");
    assert_eq!(
        tool_choice.get("type").and_then(serde_json::Value::as_str),
        Some("function")
    );
    assert!(tool_choice.get("name").is_none());
}

#[test]
fn chat_request_to_codex_rejects_spark_image_url() {
    let input = json!({
        "model": "gpt-5.3-codex-spark",
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }
                ]
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let error = chat_request_to_codex(&bytes, None).expect_err("should reject image input");

    assert!(error.contains("gpt-5.3-codex-spark"), "error: {error}");
    assert!(error.contains("text-only"), "error: {error}");
}

#[test]
fn responses_request_to_codex_uses_top_level_tool_name() {
    let input = json!({
        "model": "gpt-5",
        "input": "hi",
        "tools": [
            { "type": "function", "name": "demo_tool", "description": "noop", "parameters": {} }
        ],
        "tool_choice": { "type": "function", "name": "demo_tool" }
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let tools = value["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "demo_tool");
    assert_eq!(tools[0]["description"], "noop");
    assert!(tools[0]["parameters"].is_object());
    assert_eq!(
        value["tool_choice"]
            .get("name")
            .and_then(serde_json::Value::as_str),
        Some("demo_tool")
    );
}

#[test]
fn responses_request_to_codex_replaces_explicit_null_tool_schema_types() {
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            {
                "type": "additional_tools",
                "tools": [
                    {
                        "type": "namespace",
                        "tools": [
                            { "type": "function", "name": "nested", "parameters": { "type": null } }
                        ]
                    },
                    {
                        "type": "function",
                        "function": { "name": "legacy", "parameters": { "type": null } }
                    }
                ]
            },
            { "type": "message", "role": "user", "content": "hi" }
        ],
        "tools": [
            { "type": "function", "name": "root", "parameters": { "type": null } },
            { "type": "function", "function": { "name": "chat", "parameters": { "type": null } } },
            { "type": "function", "name": "unconstrained", "parameters": { "properties": {} } }
        ]
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["tools"][0]["parameters"]["type"], "object");
    assert_eq!(value["tools"][1]["parameters"]["type"], "object");
    assert!(value["tools"][2]["parameters"].get("type").is_none());
    assert_eq!(
        value["input"][0]["tools"][0]["tools"][0]["parameters"]["type"],
        "object"
    );
    assert_eq!(
        value["input"][0]["tools"][1]["function"]["parameters"]["type"],
        "object"
    );
}

#[test]
fn responses_request_to_codex_flattens_namespace_declaration_history_and_choice() {
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            { "type": "function_call", "call_id": "call_1", "name": "spawn_agent", "namespace": "collaboration", "arguments": "{}" },
            { "type": "message", "role": "user", "content": "hi", "name": "spawn_agent", "namespace": "collaboration" }
        ],
        "tools": [
            { "type": "function", "name": "plain", "description": "keep", "parameters": {} },
            {
                "type": "namespace",
                "name": "collaboration",
                "tools": [
                    { "type": "function", "name": "spawn_agent", "description": "spawn", "parameters": { "type": "object" } }
                ]
            },
            {
                "type": "namespace",
                "name": "image_gen",
                "tools": [{ "type": "function", "name": "imagegen" }]
            }
        ],
        "tool_choice": { "type": "function", "name": "spawn_agent", "namespace": "collaboration" }
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert namespace request");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let tools = value["tools"].as_array().expect("tools");

    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["name"], "plain");
    assert_eq!(tools[1]["type"], "function");
    assert_eq!(tools[1]["name"], "collaboration__spawn_agent");
    assert_eq!(tools[1]["description"], "spawn");
    assert_eq!(tools[2]["type"], "namespace");
    assert_eq!(tools[2]["name"], "image_gen");
    assert_eq!(value["tool_choice"]["name"], "collaboration__spawn_agent");
    assert!(value["tool_choice"].get("namespace").is_none());
    assert_eq!(value["input"][0]["name"], "collaboration__spawn_agent");
    assert!(value["input"][0].get("namespace").is_none());
    assert_eq!(value["input"][1]["name"], "spawn_agent");
    assert_eq!(value["input"][1]["namespace"], "collaboration");
}

#[test]
fn responses_request_to_codex_rejects_namespace_flat_name_collisions() {
    for tools in [
        json!([
            { "type": "function", "name": "collaboration__spawn_agent" },
            { "type": "namespace", "name": "collaboration", "tools": [
                { "type": "function", "name": "spawn_agent" }
            ] }
        ]),
        json!([
            { "type": "namespace", "name": "a", "tools": [
                { "type": "function", "name": "b__c" }
            ] },
            { "type": "namespace", "name": "a__b", "tools": [
                { "type": "function", "name": "c" }
            ] }
        ]),
    ] {
        let input = json!({ "model": "gpt-5.5", "input": "hi", "tools": tools });
        let error = responses_request_to_codex(&Bytes::from(input.to_string()), None)
            .expect_err("ambiguous namespace tools must be rejected");

        assert!(error.contains("conflict"), "error: {error}");
    }
}

#[test]
fn codex_response_to_responses_restores_namespace_function_calls_only() {
    let request_body = json!({
        "tools": [{
            "type": "namespace",
            "name": "collaboration",
            "tools": [{ "type": "function", "name": "spawn_agent" }]
        }]
    })
    .to_string();
    let response = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_namespace",
            "object": "response",
            "status": "completed",
            "output": [
                { "type": "function_call", "call_id": "call_1", "name": "collaboration__spawn_agent", "arguments": "{}" },
                { "type": "message", "name": "collaboration__spawn_agent", "content": [] }
            ]
        }
    });

    let output =
        codex_response_to_responses(&Bytes::from(response.to_string()), Some(&request_body))
            .expect("convert response");
    let value: Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["output"][0]["name"], "spawn_agent");
    assert_eq!(value["output"][0]["namespace"], "collaboration");
    assert_eq!(value["output"][1]["name"], "collaboration__spawn_agent");
    assert!(value["output"][1].get("namespace").is_none());
}

#[test]
fn codex_response_to_responses_restores_custom_tool_name() {
    let original = "custom_tool_name_for_responses_restoration_with_a_very_long_suffix";
    let short = shorten_name_if_needed(original);
    let request_body = json!({
        "tools": [{ "type": "custom", "name": original }]
    })
    .to_string();
    let response = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_custom",
            "object": "response",
            "status": "completed",
            "output": [{
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": short,
                "input": "freeform input"
            }]
        }
    });

    let output = codex_response_to_responses(
        &Bytes::from(response.to_string()),
        Some(request_body.as_str()),
    )
    .expect("convert custom tool response");
    let value: Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["output"][0]["name"], json!(original));
    assert_eq!(value["output"][0]["input"], json!("freeform input"));
}

#[test]
fn responses_request_to_codex_strips_prompt_cache_retention() {
    let input = json!({
        "model": "gpt-5",
        "input": "hi",
        "prompt_cache_retention": "24h",
        "previous_response_id": "resp_123",
        "safety_identifier": "sid_1"
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    assert!(value.get("prompt_cache_retention").is_none());
    assert!(value.get("previous_response_id").is_none());
    assert!(value.get("safety_identifier").is_none());
}

#[test]
fn responses_request_to_codex_preserves_parallel_tool_calls_false() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "hi" }]
            }
        ],
        "parallel_tool_calls": false
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(value["parallel_tool_calls"], json!(false));
}

#[test]
fn responses_request_to_codex_strips_output_parts_from_function_call_output() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "ok",
                "output_parts": [
                    { "type": "text", "text": "ok" }
                ]
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");
    assert_eq!(input_items.len(), 1);
    assert!(input_items[0].get("output_parts").is_none());
}

#[test]
fn responses_request_to_codex_strips_output_parts_from_new_tool_output_types() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "tool_search_output",
                "call_id": "call_search",
                "output": "search ok",
                "output_parts": [{ "type": "text", "text": "search ok" }]
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_custom",
                "output": "custom ok",
                "output_parts": [{ "type": "text", "text": "custom ok" }]
            },
            {
                "type": "mcp_tool_call_output",
                "call_id": "call_mcp",
                "output": "mcp ok",
                "output_parts": [{ "type": "text", "text": "mcp ok" }]
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");

    assert_eq!(input_items.len(), 3);
    assert_eq!(input_items[0]["type"], "tool_search_output");
    assert_eq!(input_items[1]["type"], "custom_tool_call_output");
    assert_eq!(input_items[2]["type"], "mcp_tool_call_output");
    assert!(input_items
        .iter()
        .all(|item| item.get("output_parts").is_none()));
}

#[test]
fn responses_request_to_codex_sanitizes_store_false_reasoning_context() {
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            {
                "type": "reasoning",
                "id": "rs_replayed_null_summary",
                "encrypted_content": null,
                "content": [{ "type": "reasoning_text", "text": "private context" }],
                "summary": null
            },
            {
                "type": "reasoning",
                "id": "rs_replayed_missing_summary",
                "encrypted_content": "",
                "content": []
            },
            {
                "type": "reasoning",
                "id": "reasoning_local_context",
                "encrypted_content": "encrypted-existing",
                "content": [],
                "summary": [{ "type": "summary_text", "text": "existing summary" }]
            },
            {
                "type": "reasoning",
                "id": "rs_existing_context",
                "encrypted_content": "encrypted-prefixed",
                "content": [],
                "summary": []
            }
        ]
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");

    assert_eq!(input_items.len(), 4);
    assert!(input_items[..2].iter().all(|item| item.get("id").is_none()));
    assert!(input_items[0]["encrypted_content"].is_null());
    assert_eq!(
        input_items[0]["content"],
        json!([{ "type": "reasoning_text", "text": "private context" }])
    );
    assert_eq!(input_items[0]["summary"], json!([]));
    assert_eq!(input_items[1]["encrypted_content"], "");
    assert_eq!(input_items[1]["content"], json!([]));
    assert_eq!(input_items[1]["summary"], json!([]));
    assert_eq!(input_items[2]["id"], "rs_reasoning_local_context");
    assert_eq!(
        input_items[2]["summary"],
        json!([{ "type": "summary_text", "text": "existing summary" }])
    );
    assert_eq!(input_items[3]["id"], "rs_existing_context");
}

#[test]
fn responses_request_to_codex_strips_invalid_call_input_ids_only() {
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            { "type": "function_call", "id": "item_function", "call_id": "call_function", "name": "function", "arguments": "{}" },
            { "type": "tool_call", "id": "item_tool", "call_id": "call_tool" },
            { "type": "local_shell_call", "id": "item_shell", "call_id": "call_shell", "tool_name": "shell" },
            { "type": "tool_search_call", "id": "item_search", "call_id": "call_search", "function": { "name": "search" } },
            { "type": "custom_tool_call", "id": "item_custom", "call_id": "call_custom", "name": "custom" },
            { "type": "mcp_tool_call", "id": "item_mcp", "call_id": "call_mcp", "name": "mcp" },
            { "type": "function_call", "id": "fc_keep", "call_id": "call_keep", "name": "keep", "arguments": "{}" },
            { "type": "function_call_output", "id": "item_output", "call_id": "call_output", "output": "ok" },
            { "type": "message", "id": "item_message", "role": "user", "content": [{ "type": "input_text", "text": "continue" }] }
        ]
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");

    assert_eq!(input_items[0]["id"], "fc_item_function");
    for (item, expected_call_id) in input_items[1..6].iter().zip([
        "call_tool",
        "call_shell",
        "call_search",
        "call_custom",
        "call_mcp",
    ]) {
        assert!(item.get("id").is_none(), "item={item}");
        assert_eq!(item["call_id"], expected_call_id);
    }
    assert_eq!(input_items[0]["call_id"], "call_function");
    assert_eq!(input_items[6]["id"], "fc_keep");
    assert_eq!(input_items[6]["call_id"], "call_keep");
    assert_eq!(input_items[7]["id"], "item_output");
    assert_eq!(input_items[7]["call_id"], "call_output");
    assert_eq!(input_items[8]["id"], "msg_item_message");
    assert_eq!(input_items[8]["content"][0]["text"], "continue");
}

#[test]
fn responses_request_to_codex_bounds_long_call_ids_and_preserves_pairing() {
    let suffix = "z".repeat(62);
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            { "type": "function_call", "call_id": format!("call_{suffix}"), "name": "lookup", "arguments": "{}" },
            { "type": "function_call_output", "call_id": format!("fc_{suffix}"), "output": "done" }
        ]
    });

    let output = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");
    let call_id = input_items[0]["call_id"].as_str().expect("call id");

    assert!(call_id.starts_with("fc_"));
    assert_eq!(call_id.len(), 64);
    assert_eq!(input_items[1]["call_id"], call_id);
}

#[test]
fn responses_request_to_codex_bounds_all_retained_input_item_ids() {
    let call_item_id = format!("fc_{}", "c".repeat(80));
    let output_item_id = format!("output_{}", "o".repeat(80));
    let message_item_id = format!("message_{}", "m".repeat(80));
    let input = json!({
        "model": "gpt-5.5",
        "input": [
            { "type": "function_call", "id": call_item_id, "call_id": "call_1", "name": "lookup", "arguments": "{}" },
            { "type": "function_call_output", "id": output_item_id, "call_id": "call_1", "output": "ok" },
            { "type": "message", "id": message_item_id, "role": "user", "content": [{ "type": "input_text", "text": "continue" }] },
            { "type": "message", "id": message_item_id, "role": "user", "content": [{ "type": "input_text", "text": "repeat" }] },
            { "type": "message", "id": "message_valid", "role": "user", "content": [{ "type": "input_text", "text": "valid" }] },
            { "type": "message", "id": "msg-1", "role": "assistant", "content": [{ "type": "input_text", "text": "already valid" }] },
            { "type": "function_call_output", "id": "output_valid", "call_id": "call_2", "output": "unchanged" }
        ]
    });

    let first = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("convert responses request");
    let second = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("repeat conversion");
    let first: Value = serde_json::from_slice(&first).expect("json");
    let second: Value = serde_json::from_slice(&second).expect("json");
    let items = first["input"].as_array().expect("input array");

    for index in 0..4 {
        let id = items[index]["id"].as_str().expect("retained id");
        assert_eq!(id.chars().count(), 64, "index={index} id={id}");
        assert_eq!(second["input"][index]["id"], id);
    }
    assert_ne!(items[0]["id"], items[1]["id"]);
    assert_eq!(items[2]["id"], items[3]["id"]);
    assert!(items[2]["id"]
        .as_str()
        .expect("message id")
        .starts_with("msg_"));
    assert_eq!(items[4]["id"], "msg_message_valid");
    assert_eq!(items[5]["id"], "msg-1");
    assert_eq!(items[6]["id"], "output_valid");
    assert_eq!(items[0]["call_id"], items[1]["call_id"]);
}

#[test]
fn chat_request_to_codex_bounds_long_call_ids_and_preserves_pairing() {
    let call_id = format!("call_{}", "x".repeat(62));
    let input = json!({
        "model": "gpt-5.5",
        "messages": [
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": { "name": "lookup", "arguments": "{}" }
                }]
            },
            { "role": "tool", "tool_call_id": call_id, "content": "done" }
        ]
    });

    let output =
        chat_request_to_codex(&Bytes::from(input.to_string()), None).expect("convert chat request");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");
    let function_call = input_items
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("function call");
    let function_output = input_items
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .expect("function output");
    let call_id = function_call["call_id"].as_str().expect("call id");

    assert!(call_id.starts_with("fc_"));
    assert_eq!(call_id.len(), 64);
    assert_eq!(function_output["call_id"], call_id);
}

#[test]
fn chat_request_to_codex_preserves_custom_tool_call_family() {
    let input = json!({
        "model": "gpt-5.6",
        "messages": [
            { "role": "assistant", "content": null, "tool_calls": [{
                "id": "call_patch",
                "type": "custom",
                "custom": { "name": "apply_patch", "input": "*** Begin Patch" }
            }] },
            { "role": "tool", "tool_call_id": "call_patch", "content": "done" }
        ]
    });
    let output =
        chat_request_to_codex(&Bytes::from(input.to_string()), None).expect("convert chat request");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let items = value["input"].as_array().expect("input array");
    let call = items
        .iter()
        .find(|item| item["type"] == "custom_tool_call")
        .expect("custom call");
    let result = items
        .iter()
        .find(|item| item["type"] == "custom_tool_call_output")
        .expect("custom output");

    assert_eq!(call["name"], "apply_patch");
    assert_eq!(call["input"], "*** Begin Patch");
    assert_eq!(result["call_id"], call["call_id"]);
    assert_eq!(result["output"], "done");
}

#[test]
fn chat_request_to_codex_pairs_missing_ids_and_drops_orphan_outputs() {
    let input = json!({
        "model": "gpt-5.6",
        "messages": [
            { "role": "tool", "content": "orphan" },
            { "role": "assistant", "content": null, "tool_calls": [{
                "type": "function",
                "function": { "name": "lookup", "arguments": "{}" }
            }] },
            { "role": "tool", "content": "found" },
            { "role": "tool", "content": "duplicate" }
        ]
    });
    let output =
        chat_request_to_codex(&Bytes::from(input.to_string()), None).expect("convert chat request");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let items = value["input"].as_array().expect("input array");
    let call = items
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("function call");
    let outputs = items
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();

    assert!(!call["call_id"].as_str().unwrap_or_default().is_empty());
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0]["call_id"], call["call_id"]);
    assert_eq!(outputs[0]["output"], "found");
}

#[test]
fn chat_request_to_codex_drops_ambiguous_duplicate_call_ids() {
    let input = json!({
        "model": "gpt-5.6",
        "messages": [
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "call_duplicate", "type": "function", "function": { "name": "lookup", "arguments": "{}" } },
                { "id": "call_duplicate", "type": "custom", "custom": { "name": "apply_patch", "input": "patch" } }
            ] },
            { "role": "tool", "tool_call_id": "call_duplicate", "content": "ambiguous" }
        ]
    });
    let output =
        chat_request_to_codex(&Bytes::from(input.to_string()), None).expect("convert chat request");
    let value: Value = serde_json::from_slice(&output).expect("json");
    let items = value["input"].as_array().expect("input array");

    assert!(items.iter().all(|item| {
        !matches!(
            item["type"].as_str(),
            Some(
                "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
            )
        )
    }));
}

#[test]
fn responses_request_to_codex_preserves_name_less_tool_call_context_items() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            { "type": "tool_call", "call_id": "call_tool" },
            { "type": "local_shell_call", "call_id": "call_shell", "tool_name": "shell" },
            { "type": "tool_search_call", "call_id": "call_search", "function": { "name": "search" } }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let input_items = value["input"].as_array().expect("input array");

    assert!(input_items.iter().all(|item| item.get("name").is_none()));
}

#[test]
fn responses_request_to_codex_rejects_spark_input_image() {
    let input = json!({
        "model": "gpt-5.3-codex-spark",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "describe" },
                    { "type": "input_image", "image_url": "data:image/png;base64,AAAA" }
                ]
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let error = responses_request_to_codex(&bytes, None).expect_err("should reject image input");

    assert!(error.contains("gpt-5.3-codex-spark"), "error: {error}");
    assert!(error.contains("text-only"), "error: {error}");
}

#[test]
fn responses_request_to_codex_rejects_spark_image_generation_tool() {
    let input = json!({
        "model": "gpt-5.3-codex-spark",
        "input": "draw icon",
        "tools": [
            { "type": "image_generation" }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let error =
        responses_request_to_codex(&bytes, None).expect_err("should reject image generation");

    assert!(error.contains("gpt-5.3-codex-spark"), "error: {error}");
    assert!(error.contains("text-only"), "error: {error}");
}

#[test]
fn responses_request_to_codex_allows_passive_spark_image_generation_namespace() {
    let input = json!({
        "model": "gpt-5.3-codex-spark",
        "input": "draw icon",
        "tools": [
            { "type": "namespace", "name": "image_gen", "description": "Generate an image" }
        ]
    });

    responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("passive namespace declaration is not image generation intent");
}

#[test]
fn responses_request_to_codex_allows_passive_spark_additional_image_generation_namespace() {
    let input = json!({
        "model": "gpt-5.3-codex-spark",
        "input": [
            {
                "type": "additional_tools",
                "tools": [
                    { "type": "namespace", "name": "image_gen" }
                ]
            },
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "draw icon" }]
            }
        ]
    });

    responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect("passive additional_tools declaration is not image generation intent");
}

#[test]
fn responses_request_to_codex_rejects_spark_explicit_image_namespace_choice() {
    let input = json!({
        "model": "gpt-5.3-codex-spark",
        "input": "draw icon",
        "tools": [{ "type": "namespace", "name": "image_gen" }],
        "tool_choice": { "type": "namespace", "name": "image_gen" }
    });

    let error = responses_request_to_codex(&Bytes::from(input.to_string()), None)
        .expect_err("explicit image namespace choice must be rejected");

    assert!(error.contains("gpt-5.3-codex-spark"), "error: {error}");
    assert!(error.contains("text-only"), "error: {error}");
}

#[test]
fn responses_request_to_codex_does_not_overmatch_namespace_tools() {
    for input in [
        json!({
            "model": "gpt-5.3-codex-spark",
            "input": "search docs",
            "tools": [{ "type": "namespace", "name": "code_tools" }]
        }),
        json!({
            "model": "gpt-5.5",
            "input": [{
                "type": "additional_tools",
                "tools": [{ "type": "namespace", "name": "image_gen" }]
            }]
        }),
    ] {
        responses_request_to_codex(&Bytes::from(input.to_string()), None)
            .expect("supported namespace tool request");
    }
}

#[test]
fn responses_request_to_codex_converts_tool_role_message_to_function_call_output() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "ok"
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");
    let item = &value["input"][0];

    assert_eq!(item["type"], "function_call_output");
    assert_eq!(item["call_id"], "call_1");
    assert_eq!(item["output"], "ok");
    assert!(item.get("role").is_none());
}

#[test]
fn responses_request_to_codex_stringifies_non_string_message_content_text() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": ["a", "b"] }
                ]
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["input"][0]["content"][0]["text"], "[\"a\",\"b\"]");
}

#[test]
fn responses_request_to_codex_downgrades_unknown_tool_choice_to_auto() {
    let input = json!({
        "model": "gpt-5",
        "input": "hi",
        "tools": [
            { "type": "function", "name": "shell" }
        ],
        "tool_choice": { "type": "custom" }
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["tool_choice"], "auto");
}

#[test]
fn responses_request_to_codex_adds_fallback_name_for_tool_call_input() {
    let input = json!({
        "model": "gpt-5",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "arguments": "{}"
            }
        ]
    });
    let bytes = Bytes::from(input.to_string());
    let output = responses_request_to_codex(&bytes, Some("gpt-5-codex")).expect("convert");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("json");

    assert_eq!(value["input"][0]["type"], "function_call");
    assert_eq!(value["input"][0]["call_id"], "call_1");
    assert_eq!(value["input"][0]["name"], "tool");
}
