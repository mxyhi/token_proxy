use super::*;
use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::super::log::{LogContext, LogWriter};
use tokio::time::{sleep, Instant as TokioInstant};

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(future)
}

async fn create_test_sqlite_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    crate::proxy::sqlite::init_schema(&pool)
        .await
        .expect("init sqlite schema");
    pool
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

async fn setup_responses_stream() -> (Arc<LogWriter>, LogContext, SqlitePool) {
    let sqlite_pool = create_test_sqlite_pool().await;
    let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
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
    (log, context, sqlite_pool)
}

async fn collect_responses_to_chat_chunks(
    upstream: impl futures_util::stream::Stream<Item = Result<Bytes, reqwest::Error>>
        + Unpin
        + Send
        + 'static,
    context: LogContext,
    log: Arc<LogWriter>,
) -> Vec<Bytes> {
    let token_tracker = super::super::token_rate::TokenRateTracker::new()
        .register(None, None)
        .await;
    super::responses_to_chat::stream_responses_to_chat(upstream, context, log, token_tracker)
        .map(|item| item.expect("stream item"))
        .collect()
        .await
}

async fn read_first_usage_tokens(
    pool: &SqlitePool,
) -> (Option<i64>, Option<i64>, Option<i64>) {
    let deadline = TokioInstant::now() + std::time::Duration::from_secs(2);
    loop {
        let row = sqlx::query(
            "SELECT input_tokens, output_tokens, total_tokens FROM request_logs ORDER BY id LIMIT 1",
        )
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if let Some(row) = row {
            let input_tokens = row.try_get::<Option<i64>, _>("input_tokens").unwrap_or_default();
            let output_tokens = row
                .try_get::<Option<i64>, _>("output_tokens")
                .unwrap_or_default();
            let total_tokens = row.try_get::<Option<i64>, _>("total_tokens").unwrap_or_default();
            return (input_tokens, output_tokens, total_tokens);
        }
        if TokioInstant::now() >= deadline {
            panic!("log entry");
        }
        sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn stream_responses_to_chat_emits_role_delta_and_done_and_logs_usage() {
    run_async(async {
        let (log, context, sqlite_pool) = setup_responses_stream().await;

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

        let chunks = collect_responses_to_chat_chunks(upstream, context, log.clone()).await;

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

        let (input_tokens, output_tokens, total_tokens) =
            read_first_usage_tokens(&sqlite_pool).await;
        assert_eq!(input_tokens, Some(1));
        assert_eq!(output_tokens, Some(2));
        assert_eq!(total_tokens, Some(3));
    });
}

#[test]
fn stream_responses_to_chat_emits_tool_call_deltas_and_finish_reason() {
    run_async(async {
        let (log, context, _sqlite_pool) = setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"status\":\"in_progress\",\"call_id\":\"call_foo\",\"name\":\"getRandomNumber\",\"arguments\":\"\"}}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\"{\\\"a\\\":\\\"0\\\"\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":0,\"delta\":\",\\\"b\\\":\\\"100\\\"}\"}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = collect_responses_to_chat_chunks(upstream, context, log.clone()).await;

        assert_eq!(chunks.len(), 6);

        let first = parse_sse_json(&chunks[0]).expect("json");
        assert_eq!(first["choices"][0]["delta"]["role"], json!("assistant"));

        let initial = parse_sse_json(&chunks[1]).expect("json");
        assert_eq!(initial["choices"][0]["delta"]["tool_calls"][0]["id"], json!("call_foo"));
        assert_eq!(
            initial["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            json!("getRandomNumber")
        );
        assert_eq!(
            initial["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("")
        );

        let delta_1 = parse_sse_json(&chunks[2]).expect("json");
        assert_eq!(
            delta_1["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("{\"a\":\"0\"")
        );

        let delta_2 = parse_sse_json(&chunks[3]).expect("json");
        assert_eq!(
            delta_2["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!(",\"b\":\"100\"}")
        );

        let done = parse_sse_json(&chunks[4]).expect("json");
        assert_eq!(done["choices"][0]["finish_reason"], json!("tool_calls"));

        assert_eq!(String::from_utf8_lossy(&chunks[5]), "data: [DONE]\n\n");
    });
}

#[test]
fn stream_responses_to_chat_emits_content_parts_for_non_text() {
    run_async(async {
        let (log, context, _sqlite_pool) = setup_responses_stream().await;

        let upstream = futures_util::stream::iter(vec![
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"msg_1\",\"type\":\"message\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\",\"annotations\":[]},{\"type\":\"output_image\",\"image_url\":{\"url\":\"https://example.com/a.png\"}}]}}\n\n",
            )),
            Ok(Bytes::from("data: [DONE]\n\n")),
        ]);

        let chunks = collect_responses_to_chat_chunks(upstream, context, log.clone()).await;

        assert_eq!(chunks.len(), 5);

        let first = parse_sse_json(&chunks[0]).expect("json");
        assert_eq!(first["choices"][0]["delta"]["role"], json!("assistant"));

        let text_delta = parse_sse_json(&chunks[1]).expect("json");
        assert_eq!(text_delta["choices"][0]["delta"]["content"], json!("Hello"));

        let parts_delta = parse_sse_json(&chunks[2]).expect("json");
        assert_eq!(parts_delta["choices"][0]["delta"]["content_parts"][0]["type"], json!("output_text"));
        assert_eq!(parts_delta["choices"][0]["delta"]["content_parts"][1]["type"], json!("output_image"));

        let done = parse_sse_json(&chunks[3]).expect("json");
        assert_eq!(done["choices"][0]["finish_reason"], json!("stop"));

        assert_eq!(String::from_utf8_lossy(&chunks[4]), "data: [DONE]\n\n");
    });
}

#[test]
fn stream_chat_to_responses_handles_chunk_boundaries_and_emits_created_delta_done_and_logs_usage() {
    run_async(async {
        let sqlite_pool = create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
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

        let (input_tokens, output_tokens, total_tokens) =
            read_first_usage_tokens(&sqlite_pool).await;
        assert_eq!(input_tokens, Some(1));
        assert_eq!(output_tokens, Some(2));
        assert_eq!(total_tokens, Some(3));
    });
}

#[test]
fn stream_chat_to_responses_emits_function_call_events_and_includes_them_in_completed_response() {
    run_async(async {
        let sqlite_pool = create_test_sqlite_pool().await;
        let log = Arc::new(LogWriter::new(Some(sqlite_pool.clone())));
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

        let (input_tokens, output_tokens, total_tokens) =
            read_first_usage_tokens(&sqlite_pool).await;
        assert_eq!(input_tokens, Some(1));
        assert_eq!(output_tokens, Some(2));
        assert_eq!(total_tokens, Some(3));
    });
}
