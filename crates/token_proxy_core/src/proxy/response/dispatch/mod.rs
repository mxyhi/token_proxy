mod buffered;
mod stream;

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use std::sync::Arc;
use std::time::Duration;

use super::super::log::{LogContext, LogWriter};
use super::super::openai_compat::FormatTransform;
use super::super::token_rate::RequestTokenTracker;

pub(super) async fn build_stream_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    upstream_no_data_timeout: Duration,
) -> Response {
    stream::build_stream_response(
        status,
        upstream_res,
        headers,
        context,
        log,
        request_tracker,
        response_transform,
        model_override,
        estimated_input_tokens,
        upstream_no_data_timeout,
    )
    .await
}

pub(super) async fn build_buffered_response(
    status: StatusCode,
    upstream_res: reqwest::Response,
    headers: HeaderMap,
    context: LogContext,
    log: Arc<LogWriter>,
    request_tracker: RequestTokenTracker,
    response_transform: FormatTransform,
    model_override: Option<&str>,
    estimated_input_tokens: Option<u64>,
    upstream_no_data_timeout: Duration,
) -> Response {
    buffered::build_buffered_response(
        status,
        upstream_res,
        headers,
        context,
        log,
        request_tracker,
        response_transform,
        model_override,
        estimated_input_tokens,
        upstream_no_data_timeout,
    )
    .await
}

#[cfg(test)]
#[path = "buffered.test.rs"]
mod buffered_tests;
