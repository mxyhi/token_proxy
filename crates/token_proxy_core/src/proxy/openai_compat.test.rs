use super::*;
use axum::body::Bytes;
use serde_json::{json, Value};

use crate::proxy::http_client::ProxyHttpClients;

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(future)
}

fn bytes_from_json(value: Value) -> Bytes {
    Bytes::from(serde_json::to_vec(&value).expect("serialize JSON"))
}

fn json_from_bytes(bytes: Bytes) -> Value {
    serde_json::from_slice(&bytes).expect("parse JSON")
}

fn transform_request_value(
    transform: FormatTransform,
    input: Value,
    http_clients: &ProxyHttpClients,
    model_hint: Option<&str>,
) -> Value {
    let bytes = bytes_from_json(input);
    let output = run_async(async {
        transform_request_body(transform, &bytes, http_clients, model_hint)
            .await
            .expect("transform")
    });
    json_from_bytes(output)
}

fn transform_response_value(
    transform: FormatTransform,
    input: Value,
    model_hint: Option<&str>,
) -> Value {
    let bytes = bytes_from_json(input);
    let output = transform_response_body(transform, &bytes, model_hint).expect("transform");
    json_from_bytes(output)
}

// Split the test suite to keep each file below the project's line limit.
#[path = "openai_compat.test.part1.rs"]
mod part1;
#[path = "openai_compat.test.part2.rs"]
mod part2;
