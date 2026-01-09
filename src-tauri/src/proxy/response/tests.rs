use super::*;
use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use super::super::log::{LogContext, LogWriter};

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(future)
}

fn unique_log_path(prefix: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    PathBuf::from(format!("target/test-logs/{prefix}_{now}.log"))
}

fn parse_sse_json(bytes: &Bytes) -> Option<Value> {
    let text = String::from_utf8_lossy(bytes);
    let Some(data) = text.strip_prefix("data: ") else {
        panic!("unexpected SSE chunk: {text:?}");
    };
    let data = data.trim();
    if data == "[DONE]" {
        return None;
    }
    Some(serde_json::from_str::<Value>(data).expect("parse SSE JSON"))
}

#[test]
fn stream_responses_to_chat_emits_role_delta_and_done_and_logs_usage() {
    run_async(async {
        let log_path = unique_log_path("responses_to_chat");
        let log = Arc::new(
            LogWriter::new(&log_path, None)
                .await
                .expect("create log writer"),
        );
        let context = LogContext {
            path: "/v1/responses".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "unit-test".to_string(),
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\n",
            )),
            // Usage can appear on a different event; collector should still pick it up.
            Ok(Bytes::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker = super::super::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunks: Vec<Bytes> =
            super::responses_to_chat::stream_responses_to_chat(
                upstream,
                context,
                log.clone(),
                token_tracker,
            )
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        assert_eq!(chunks.len(), 5);

        let first = parse_sse_json(&chunks[0]).expect("json");
        let id = first["id"].as_str().expect("id");
        assert!(id.starts_with("chatcmpl_proxy_"));
        assert_eq!(first["model"], json!("unit-model"));
        assert_eq!(first["choices"][0]["delta"]["role"], json!("assistant"));
        assert_eq!(first["choices"][0]["delta"]["content"], json!(""));

        let second = parse_sse_json(&chunks[1]).expect("json");
        assert_eq!(second["id"], json!(id));
        assert_eq!(second["choices"][0]["delta"]["content"], json!("Hello"));

        let third = parse_sse_json(&chunks[2]).expect("json");
        assert_eq!(third["id"], json!(id));
        assert_eq!(third["choices"][0]["delta"]["content"], json!(" world"));

        let done = parse_sse_json(&chunks[3]).expect("json");
        assert_eq!(done["id"], json!(id));
        assert_eq!(done["choices"][0]["finish_reason"], json!("stop"));

        assert_eq!(String::from_utf8_lossy(&chunks[4]), "data: [DONE]\n\n");

        let contents = tokio::fs::read_to_string(&log_path)
            .await
            .expect("read log");
        let line = contents.lines().next().expect("log line");
        let entry: Value = serde_json::from_str(line).expect("parse log entry");
        assert_eq!(entry["usage"]["input_tokens"], json!(1));
        assert_eq!(entry["usage"]["output_tokens"], json!(2));
        assert_eq!(entry["usage"]["total_tokens"], json!(3));
    });
}

#[test]
fn stream_chat_to_responses_handles_chunk_boundaries_and_emits_created_delta_done_and_logs_usage() {
    run_async(async {
        let log_path = unique_log_path("chat_to_responses");
        let log = Arc::new(
            LogWriter::new(&log_path, None)
                .await
                .expect("create log writer"),
        );
        let context = LogContext {
            path: "/v1/chat/completions".to_string(),
            provider: "openai".to_string(),
            upstream_id: "unit-test".to_string(),
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            start: Instant::now(),
        };

        let first_event = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";
        let (first_a, first_b) = first_event.split_at(12);

        let upstream = futures_util::stream::iter(vec![
            Ok(Bytes::from(first_a.to_string())),
            Ok(Bytes::from(first_b.to_string())),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n",
            )),
            // Chat usage format.
            Ok(Bytes::from(
                "data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker = super::super::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunks: Vec<Bytes> =
            stream_chat_to_responses(upstream, context, log.clone(), token_tracker)
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        assert_eq!(chunks.len(), 10);

        let created = parse_sse_json(&chunks[0]).expect("json");
        assert_eq!(created["type"], json!("response.created"));
        let response_id = created["response"]["id"].as_str().expect("response.id");
        assert!(response_id.starts_with("resp_"));

        let output_item_added = parse_sse_json(&chunks[1]).expect("json");
        assert_eq!(output_item_added["type"], json!("response.output_item.added"));
        assert_eq!(output_item_added["output_index"], json!(0));
        let item_id = output_item_added["item"]["id"].as_str().expect("item.id");
        assert!(item_id.starts_with("msg_"));

        let content_part_added = parse_sse_json(&chunks[2]).expect("json");
        assert_eq!(content_part_added["type"], json!("response.content_part.added"));
        assert_eq!(content_part_added["item_id"], json!(item_id));
        assert_eq!(content_part_added["output_index"], json!(0));
        assert_eq!(content_part_added["content_index"], json!(0));
        assert_eq!(content_part_added["part"]["type"], json!("output_text"));
        assert_eq!(content_part_added["part"]["text"], json!(""));

        let delta_1 = parse_sse_json(&chunks[3]).expect("json");
        assert_eq!(delta_1["type"], json!("response.output_text.delta"));
        assert_eq!(delta_1["item_id"], json!(item_id));
        assert_eq!(delta_1["delta"], json!("Hi"));
        assert_eq!(delta_1["sequence_number"], json!(3));

        let delta_2 = parse_sse_json(&chunks[4]).expect("json");
        assert_eq!(delta_2["type"], json!("response.output_text.delta"));
        assert_eq!(delta_2["item_id"], json!(item_id));
        assert_eq!(delta_2["delta"], json!("!"));
        assert_eq!(delta_2["sequence_number"], json!(4));

        let output_text_done = parse_sse_json(&chunks[5]).expect("json");
        assert_eq!(output_text_done["type"], json!("response.output_text.done"));
        assert_eq!(output_text_done["item_id"], json!(item_id));
        assert_eq!(output_text_done["text"], json!("Hi!"));

        let content_part_done = parse_sse_json(&chunks[6]).expect("json");
        assert_eq!(content_part_done["type"], json!("response.content_part.done"));
        assert_eq!(content_part_done["item_id"], json!(item_id));
        assert_eq!(content_part_done["part"]["text"], json!("Hi!"));

        let output_item_done = parse_sse_json(&chunks[7]).expect("json");
        assert_eq!(output_item_done["type"], json!("response.output_item.done"));
        assert_eq!(output_item_done["output_index"], json!(0));
        assert_eq!(output_item_done["item"]["id"], json!(item_id));
        assert_eq!(output_item_done["item"]["content"][0]["type"], json!("output_text"));
        assert_eq!(output_item_done["item"]["content"][0]["text"], json!("Hi!"));

        let completed = parse_sse_json(&chunks[8]).expect("json");
        assert_eq!(completed["type"], json!("response.completed"));
        assert_eq!(completed["response"]["id"], json!(response_id));
        assert_eq!(completed["response"]["output"][0]["id"], json!(item_id));
        assert_eq!(completed["response"]["output"][0]["content"][0]["text"], json!("Hi!"));
        assert_eq!(completed["response"]["usage"]["input_tokens"], json!(1));
        assert_eq!(completed["response"]["usage"]["output_tokens"], json!(2));
        assert_eq!(completed["response"]["usage"]["total_tokens"], json!(3));

        assert_eq!(String::from_utf8_lossy(&chunks[9]), "data: [DONE]\n\n");

        let contents = tokio::fs::read_to_string(&log_path)
            .await
            .expect("read log");
        let line = contents.lines().next().expect("log line");
        let entry: Value = serde_json::from_str(line).expect("parse log entry");
        assert_eq!(entry["usage"]["input_tokens"], json!(1));
        assert_eq!(entry["usage"]["output_tokens"], json!(2));
        assert_eq!(entry["usage"]["total_tokens"], json!(3));
    });
}

#[test]
fn stream_chat_to_responses_emits_function_call_events_and_includes_them_in_completed_response() {
    run_async(async {
        let log_path = unique_log_path("chat_to_responses_tool_calls");
        let log = Arc::new(
            LogWriter::new(&log_path, None)
                .await
                .expect("create log writer"),
        );
        let context = LogContext {
            path: "/v1/chat/completions".to_string(),
            provider: "openai".to_string(),
            upstream_id: "unit-test".to_string(),
            model: Some("unit-model".to_string()),
            mapped_model: Some("unit-model".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            start: Instant::now(),
        };

        let upstream = futures_util::stream::iter(vec![
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_foo\",\"type\":\"function\",\"function\":{\"name\":\"getRandomNumber\",\"arguments\":\"{\\\"a\\\":\\\"0\\\"\"}}]}}]}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\",\\\"b\\\":\\\"100\\\"}\"}}]}}]}\n\n",
            )),
            // Chat usage format.
            Ok(Bytes::from(
                "data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let token_tracker = super::super::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunks: Vec<Bytes> =
            stream_chat_to_responses(upstream, context, log.clone(), token_tracker)
            .map(|item| item.expect("stream item"))
            .collect()
            .await;

        assert_eq!(chunks.len(), 8);

        let created = parse_sse_json(&chunks[0]).expect("json");
        assert_eq!(created["type"], json!("response.created"));
        let response_id = created["response"]["id"].as_str().expect("response.id");
        assert!(response_id.starts_with("resp_"));

        let output_item_added = parse_sse_json(&chunks[1]).expect("json");
        assert_eq!(output_item_added["type"], json!("response.output_item.added"));
        assert_eq!(output_item_added["output_index"], json!(0));
        assert_eq!(output_item_added["item"]["type"], json!("function_call"));
        assert_eq!(output_item_added["item"]["call_id"], json!("call_foo"));
        assert_eq!(output_item_added["item"]["name"], json!("getRandomNumber"));
        let item_id = output_item_added["item"]["id"].as_str().expect("item.id");
        assert!(item_id.starts_with("fc_"));

        let delta_1 = parse_sse_json(&chunks[2]).expect("json");
        assert_eq!(delta_1["type"], json!("response.function_call_arguments.delta"));
        assert_eq!(delta_1["item_id"], json!(item_id));
        assert_eq!(delta_1["output_index"], json!(0));
        assert_eq!(delta_1["delta"], json!("{\"a\":\"0\""));

        let delta_2 = parse_sse_json(&chunks[3]).expect("json");
        assert_eq!(delta_2["type"], json!("response.function_call_arguments.delta"));
        assert_eq!(delta_2["item_id"], json!(item_id));
        assert_eq!(delta_2["output_index"], json!(0));
        assert_eq!(delta_2["delta"], json!(",\"b\":\"100\"}"));

        let args_done = parse_sse_json(&chunks[4]).expect("json");
        assert_eq!(args_done["type"], json!("response.function_call_arguments.done"));
        assert_eq!(args_done["item_id"], json!(item_id));
        assert_eq!(args_done["name"], json!("getRandomNumber"));
        assert_eq!(args_done["arguments"], json!("{\"a\":\"0\",\"b\":\"100\"}"));

        let item_done = parse_sse_json(&chunks[5]).expect("json");
        assert_eq!(item_done["type"], json!("response.output_item.done"));
        assert_eq!(item_done["item"]["id"], json!(item_id));
        assert_eq!(item_done["item"]["status"], json!("completed"));
        assert_eq!(item_done["item"]["type"], json!("function_call"));
        assert_eq!(item_done["item"]["call_id"], json!("call_foo"));
        assert_eq!(item_done["item"]["name"], json!("getRandomNumber"));
        assert_eq!(item_done["item"]["arguments"], json!("{\"a\":\"0\",\"b\":\"100\"}"));

        let completed = parse_sse_json(&chunks[6]).expect("json");
        assert_eq!(completed["type"], json!("response.completed"));
        assert_eq!(completed["response"]["id"], json!(response_id));
        assert_eq!(completed["response"]["output"][0]["type"], json!("function_call"));
        assert_eq!(completed["response"]["output"][0]["call_id"], json!("call_foo"));
        assert_eq!(completed["response"]["output"][0]["name"], json!("getRandomNumber"));
        assert_eq!(
            completed["response"]["output"][0]["arguments"],
            json!("{\"a\":\"0\",\"b\":\"100\"}")
        );
        assert_eq!(completed["response"]["usage"]["input_tokens"], json!(1));
        assert_eq!(completed["response"]["usage"]["output_tokens"], json!(2));
        assert_eq!(completed["response"]["usage"]["total_tokens"], json!(3));

        assert_eq!(String::from_utf8_lossy(&chunks[7]), "data: [DONE]\n\n");

        let contents = tokio::fs::read_to_string(&log_path)
            .await
            .expect("read log");
        let line = contents.lines().next().expect("log line");
        let entry: Value = serde_json::from_str(line).expect("parse log entry");
        assert_eq!(entry["usage"]["input_tokens"], json!(1));
        assert_eq!(entry["usage"]["output_tokens"], json!(2));
        assert_eq!(entry["usage"]["total_tokens"], json!(3));
    });
}
