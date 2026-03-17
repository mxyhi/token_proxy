use axum::http::{Method, StatusCode};
use std::time::Duration;
use tokio::time::timeout;

use super::kiro_headers::build_kiro_headers;
use super::request;
use super::{result, AttemptOutcome};
use crate::proxy::config::UpstreamRuntime;
use crate::proxy::http;
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::request_detail::RequestDetailSnapshot;
use crate::proxy::{ProxyState, RequestMeta};

pub(super) enum KiroSendError {
    Timeout,
    Upstream(reqwest::Error),
}

pub(super) fn build_client(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
) -> Result<reqwest::Client, AttemptOutcome> {
    state
        .http_clients
        .client_for_proxy_url(upstream.proxy_url.as_deref())
        .map_err(|message| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_GATEWAY, message))
        })
}

pub(super) async fn read_request_json(
    state: &ProxyState,
    body: &ReplayableBody,
) -> Result<serde_json::Value, AttemptOutcome> {
    let Some(bytes) = body
        .read_bytes_if_small(state.config.max_request_body_bytes)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::BAD_REQUEST,
                format!("Failed to read request body: {err}"),
            ))
        })?
    else {
        return Err(AttemptOutcome::Fatal(http::error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request body is too large to transform.",
        )));
    };
    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_REQUEST,
            "Request body must be JSON.",
        ))
    })
}

pub(super) async fn send_kiro_request(
    client: &reqwest::Client,
    method: Method,
    url: &str,
    access_token: &str,
    amz_target: &str,
    is_idc: bool,
    payload: &[u8],
    overrides: Option<&[crate::proxy::config::HeaderOverride]>,
    upstream_no_data_timeout: Duration,
) -> Result<reqwest::Response, KiroSendError> {
    let mut request_headers = build_kiro_headers(access_token, amz_target, is_idc);
    if let Some(overrides) = overrides {
        request::apply_header_overrides(&mut request_headers, overrides);
    }

    let result = timeout(
        upstream_no_data_timeout,
        client
            .request(method, url)
            .headers(request_headers)
            .body(payload.to_vec())
            .send(),
    )
    .await;
    match result {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(err)) => Err(KiroSendError::Upstream(err)),
        Err(_) => Err(KiroSendError::Timeout),
    }
}

pub(super) async fn refresh_kiro_account(
    state: &ProxyState,
    account_id: &str,
) -> Result<(), AttemptOutcome> {
    state
        .kiro_accounts
        .refresh_account(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))
}

pub(super) async fn handle_send_error(
    state: &ProxyState,
    meta: &RequestMeta,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    err: KiroSendError,
    start_time: std::time::Instant,
) -> AttemptOutcome {
    match err {
        KiroSendError::Upstream(err) => {
            result::handle_upstream_result(
                state,
                Err(err),
                meta,
                "kiro",
                &upstream.id,
                inbound_path,
                state.log.clone(),
                state.token_rate.clone(),
                start_time,
                response_transform,
                request_detail,
            )
            .await
        }
        KiroSendError::Timeout => {
            let message = format!(
                "Upstream did not respond within {}s.",
                state.config.upstream_no_data_timeout.as_secs()
            );
            result::log_upstream_error_if_needed(
                &state.log,
                request_detail.as_ref(),
                meta,
                "kiro",
                &upstream.id,
                inbound_path,
                StatusCode::GATEWAY_TIMEOUT,
                message.clone(),
                start_time,
            );
            AttemptOutcome::Retryable {
                message,
                response: None,
                is_timeout: true,
                should_cooldown: true,
            }
        }
    }
}
