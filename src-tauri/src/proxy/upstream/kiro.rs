use axum::http::{
    header::{ACCEPT, CONTENT_TYPE, USER_AGENT},
    HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
};
use serde_json::Value;
use std::time::Instant;
use tokio::time::timeout;

use super::request;
use super::{result, AttemptOutcome};
use crate::proxy::http;
use crate::proxy::kiro::{
    build_payload_from_responses, determine_agentic_mode, map_model_to_kiro, select_endpoints,
    KiroEndpointConfig,
};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::{ProxyState, RequestMeta, UPSTREAM_NO_DATA_TIMEOUT};
use crate::proxy::{config::UpstreamRuntime, request_detail::RequestDetailSnapshot};
use crate::kiro::KiroTokenRecord;

const KIRO_REQUEST_CONTENT_TYPE: &str = "application/x-amz-json-1.0";
const KIRO_REQUEST_ACCEPT: &str = "*/*";
const KIRO_AGENT_MODE_IDC: &str = "spec";
const KIRO_AGENT_MODE_DEFAULT: &str = "vibe";
const KIRO_OPT_OUT: &str = "true";
const KIRO_SDK_REQUEST: &str = "attempt=1; max=3";
const KIRO_USER_AGENT_IDC: &str = "aws-sdk-js/1.0.18 ua/2.1 os/darwin#25.0.0 lang/js md/nodejs#20.16.0 api/codewhispererstreaming#1.0.18 m/E KiroIDE-0.2.13-66c23a8c5d15afabec89ef9954ef52a119f10d369df04d548fc6c1eac694b0d1";
const KIRO_USER_AGENT_IDC_AMZ: &str =
    "aws-sdk-js/1.0.18 KiroIDE-0.2.13-66c23a8c5d15afabec89ef9954ef52a119f10d369df04d548fc6c1eac694b0d1";
const KIRO_USER_AGENT_DEFAULT: &str = "aws-sdk-rust/1.3.9 os/macos lang/rust/1.87.0";
const KIRO_USER_AGENT_DEFAULT_AMZ: &str =
    "aws-sdk-rust/1.3.9 ua/2.1 api/ssooidc/1.88.0 os/macos lang/rust/1.87.0 m/E app/AmazonQ-For-CLI";

const HEADER_AMZ_TARGET: HeaderName = HeaderName::from_static("x-amz-target");
const HEADER_AMZ_USER_AGENT: HeaderName = HeaderName::from_static("x-amz-user-agent");
const HEADER_AMZ_SDK_REQUEST: HeaderName = HeaderName::from_static("amz-sdk-request");
const HEADER_AMZ_SDK_INVOCATION_ID: HeaderName = HeaderName::from_static("amz-sdk-invocation-id");
const HEADER_KIRO_AGENT_MODE: HeaderName = HeaderName::from_static("x-amzn-kiro-agent-mode");
const HEADER_KIRO_OPTOUT: HeaderName = HeaderName::from_static("x-amzn-codewhisperer-optout");

pub(super) async fn attempt_kiro_upstream(
    state: &ProxyState,
    method: Method,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    headers: &HeaderMap,
    body: &ReplayableBody,
    meta: &RequestMeta,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> AttemptOutcome {
    let mut context = match prepare_kiro_context(
        state,
        upstream,
        body,
        meta,
        headers,
        method,
        inbound_path,
        response_transform,
        request_detail,
    )
    .await
    {
        Ok(context) => context,
        Err(outcome) => return outcome,
    };
    run_kiro_endpoints(&mut context).await
}

struct KiroContext<'a> {
    state: &'a ProxyState,
    method: Method,
    upstream: &'a UpstreamRuntime,
    inbound_path: &'a str,
    headers: &'a HeaderMap,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    mapped_meta: RequestMeta,
    request_value: Value,
    account_id: String,
    record: KiroTokenRecord,
    endpoints: Vec<KiroEndpointConfig>,
    is_idc: bool,
    model_id: String,
    is_agentic: bool,
    is_chat_only: bool,
    client: reqwest::Client,
}

enum EndpointOutcome {
    Continue,
    Done(AttemptOutcome),
}

async fn prepare_kiro_context<'a>(
    state: &'a ProxyState,
    upstream: &'a UpstreamRuntime,
    body: &ReplayableBody,
    meta: &RequestMeta,
    headers: &'a HeaderMap,
    method: Method,
    inbound_path: &'a str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
) -> Result<KiroContext<'a>, AttemptOutcome> {
    let mapped_meta = super::build_mapped_meta(meta, upstream);
    let request_value = read_request_json(state, body).await?;
    let account_id = resolve_account_id(upstream)?;
    let record = load_account_record(state, &account_id).await?;
    let is_idc = record.auth_method.trim().eq_ignore_ascii_case("idc");
    let endpoints = resolve_endpoints(state, upstream, is_idc);
    let (model_id, is_agentic, is_chat_only) = resolve_model(&mapped_meta);
    let client = build_client(state, upstream)?;

    Ok(KiroContext {
        state,
        method,
        upstream,
        inbound_path,
        headers,
        response_transform,
        request_detail,
        mapped_meta,
        request_value,
        account_id,
        record,
        endpoints,
        is_idc,
        model_id,
        is_agentic,
        is_chat_only,
        client,
    })
}

async fn run_kiro_endpoints(context: &mut KiroContext<'_>) -> AttemptOutcome {
    let endpoints = context.endpoints.clone();
    let total = endpoints.len();
    for (index, endpoint) in endpoints.iter().enumerate() {
        let is_last = index + 1 >= total;
        match attempt_endpoint(context, endpoint, is_last).await {
            EndpointOutcome::Continue => continue,
            EndpointOutcome::Done(outcome) => return outcome,
        }
    }

    AttemptOutcome::Fatal(http::error_response(
        StatusCode::BAD_GATEWAY,
        "Kiro upstream request failed.",
    ))
}

async fn attempt_endpoint(
    context: &mut KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
    is_last: bool,
) -> EndpointOutcome {
    let payload = match build_endpoint_payload(context, endpoint) {
        Ok(payload) => payload,
        Err(outcome) => return EndpointOutcome::Done(outcome),
    };

    let (response, start_time) = match send_endpoint_request(context, endpoint, &payload.payload).await
    {
        Ok(result) => result,
        Err(outcome) => return EndpointOutcome::Done(outcome),
    };

    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return EndpointOutcome::Done(
            handle_auth_response(context, endpoint, payload.payload).await,
        );
    }

    if response.status() == StatusCode::TOO_MANY_REQUESTS && !is_last {
        return EndpointOutcome::Continue;
    }

    let is_forbidden = response.status() == StatusCode::FORBIDDEN;
    EndpointOutcome::Done(
        finalize_response(
            context.state,
            &context.mapped_meta,
            context.upstream,
            context.inbound_path,
            context.response_transform,
            context.request_detail.clone(),
            response,
            is_forbidden,
            start_time,
        )
        .await,
    )
}

fn build_endpoint_payload(
    context: &KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
) -> Result<crate::proxy::kiro::BuildPayloadResult, AttemptOutcome> {
    build_payload_from_responses(
        &context.request_value,
        &context.model_id,
        context.record.profile_arn.as_deref(),
        endpoint.origin,
        context.is_agentic,
        context.is_chat_only,
        context.headers,
    )
    .map_err(|message| {
        AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_REQUEST, message))
    })
}

async fn send_endpoint_request(
    context: &KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
    payload: &[u8],
) -> Result<(reqwest::Response, Instant), AttemptOutcome> {
    let start_time = Instant::now();
    let response = match send_kiro_request(
        &context.client,
        context.method.clone(),
        endpoint.url,
        &context.record.access_token,
        endpoint.amz_target,
        context.is_idc,
        payload,
        context.upstream.header_overrides.as_deref(),
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            let outcome = handle_send_error(
                context.state,
                &context.mapped_meta,
                context.upstream,
                context.inbound_path,
                context.response_transform,
                context.request_detail.clone(),
                err,
                start_time,
            )
            .await;
            return Err(outcome);
        }
    };
    Ok((response, start_time))
}

async fn handle_auth_response(
    context: &mut KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
    payload: Vec<u8>,
) -> AttemptOutcome {
    if let Err(outcome) = refresh_kiro_account(context.state, &context.account_id).await {
        return outcome;
    }
    context.record = match load_account_record(context.state, &context.account_id).await {
        Ok(record) => record,
        Err(outcome) => return outcome,
    };

    let retry_start = Instant::now();
    let retry = match send_kiro_request(
        &context.client,
        context.method.clone(),
        endpoint.url,
        &context.record.access_token,
        endpoint.amz_target,
        context.is_idc,
        &payload,
        context.upstream.header_overrides.as_deref(),
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            return handle_send_error(
                context.state,
                &context.mapped_meta,
                context.upstream,
                context.inbound_path,
                context.response_transform,
                context.request_detail.clone(),
                err,
                retry_start,
            )
            .await
        }
    };

    let is_forbidden = retry.status() == StatusCode::FORBIDDEN;
    finalize_response(
        context.state,
        &context.mapped_meta,
        context.upstream,
        context.inbound_path,
        context.response_transform,
        context.request_detail.clone(),
        retry,
        is_forbidden,
        retry_start,
    )
    .await
}

fn resolve_account_id(upstream: &UpstreamRuntime) -> Result<String, AttemptOutcome> {
    upstream
        .kiro_account_id
        .as_ref()
        .map(|value| value.to_string())
        .ok_or_else(|| {
            AttemptOutcome::Fatal(http::error_response(
                StatusCode::UNAUTHORIZED,
                "Kiro account is not configured.",
            ))
        })
}

async fn load_account_record(
    state: &ProxyState,
    account_id: &str,
) -> Result<KiroTokenRecord, AttemptOutcome> {
    state
        .kiro_accounts
        .get_account_record(account_id)
        .await
        .map_err(|err| AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err)))
}

fn resolve_endpoints(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    is_idc: bool,
) -> Vec<KiroEndpointConfig> {
    let preferred = upstream
        .kiro_preferred_endpoint
        .clone()
        .or(state.config.kiro_preferred_endpoint.clone());
    select_endpoints(preferred, is_idc)
}

fn resolve_model(meta: &RequestMeta) -> (String, bool, bool) {
    let model_source = meta
        .mapped_model
        .as_deref()
        .or(meta.original_model.as_deref())
        .unwrap_or("claude-sonnet-4.5");
    let (is_agentic, is_chat_only) =
        determine_agentic_mode(meta.original_model.as_deref().unwrap_or(model_source));
    (map_model_to_kiro(model_source), is_agentic, is_chat_only)
}

fn build_client(
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

async fn read_request_json(
    state: &ProxyState,
    body: &ReplayableBody,
) -> Result<Value, AttemptOutcome> {
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
    serde_json::from_slice::<Value>(&bytes).map_err(|_| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::BAD_REQUEST,
            "Request body must be JSON.",
        ))
    })
}

enum KiroSendError {
    Timeout,
    Upstream(reqwest::Error),
}

async fn send_kiro_request(
    client: &reqwest::Client,
    method: Method,
    url: &str,
    access_token: &str,
    amz_target: &str,
    is_idc: bool,
    payload: &[u8],
    overrides: Option<&[crate::proxy::config::HeaderOverride]>,
) -> Result<reqwest::Response, KiroSendError> {
    let mut request_headers = build_kiro_headers(access_token, amz_target, is_idc);
    if let Some(overrides) = overrides {
        request::apply_header_overrides(&mut request_headers, overrides);
    }

    let result = timeout(
        UPSTREAM_NO_DATA_TIMEOUT,
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

fn build_kiro_headers(access_token: &str, amz_target: &str, is_idc: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(KIRO_REQUEST_CONTENT_TYPE));
    headers.insert(ACCEPT, HeaderValue::from_static(KIRO_REQUEST_ACCEPT));
    if let Ok(value) = HeaderValue::from_str(amz_target) {
        headers.insert(HEADER_AMZ_TARGET, value);
    }
    headers.insert(HEADER_AMZ_SDK_REQUEST, HeaderValue::from_static(KIRO_SDK_REQUEST));
    if let Ok(value) = HeaderValue::from_str(&crate::proxy::kiro::utils::random_uuid()) {
        headers.insert(HEADER_AMZ_SDK_INVOCATION_ID, value);
    }
    headers.insert(
        HEADER_KIRO_AGENT_MODE,
        HeaderValue::from_static(if is_idc {
            KIRO_AGENT_MODE_IDC
        } else {
            KIRO_AGENT_MODE_DEFAULT
        }),
    );
    headers.insert(HEADER_KIRO_OPTOUT, HeaderValue::from_static(KIRO_OPT_OUT));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(if is_idc {
            KIRO_USER_AGENT_IDC
        } else {
            KIRO_USER_AGENT_DEFAULT
        }),
    );
    headers.insert(
        HEADER_AMZ_USER_AGENT,
        HeaderValue::from_static(if is_idc {
            KIRO_USER_AGENT_IDC_AMZ
        } else {
            KIRO_USER_AGENT_DEFAULT_AMZ
        }),
    );
    if let Some(auth) = http::bearer_header(access_token) {
        headers.insert(axum::http::header::AUTHORIZATION, auth);
    }
    headers
}

async fn refresh_kiro_account(
    state: &ProxyState,
    account_id: &str,
) -> Result<(), AttemptOutcome> {
    state
        .kiro_accounts
        .refresh_account(account_id)
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err))
        })
}

async fn finalize_response(
    state: &ProxyState,
    meta: &RequestMeta,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    response: reqwest::Response,
    force_success: bool,
    start_time: Instant,
) -> AttemptOutcome {
    if force_success {
        let output = crate::proxy::response::build_proxy_response(
            meta,
            "kiro",
            &upstream.id,
            inbound_path,
            response,
            state.log.clone(),
            state.token_rate.clone(),
            start_time,
            response_transform,
            request_detail,
        )
        .await;
        return AttemptOutcome::Success(output);
    }
    result::handle_upstream_result(
        Ok(response),
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

async fn handle_send_error(
    state: &ProxyState,
    meta: &RequestMeta,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    response_transform: FormatTransform,
    request_detail: Option<RequestDetailSnapshot>,
    err: KiroSendError,
    start_time: Instant,
) -> AttemptOutcome {
    match err {
        KiroSendError::Upstream(err) => {
            result::handle_upstream_result(
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
                UPSTREAM_NO_DATA_TIMEOUT.as_secs()
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
            }
        }
    }
}
