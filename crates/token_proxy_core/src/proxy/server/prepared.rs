use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::Response,
};
use std::{sync::Arc, time::Instant};
use url::form_urlencoded;

use super::super::{
    config::ProxyConfig,
    http,
    log::LogWriter,
    request_body::ReplayableBody,
    request_detail::RequestDetailSnapshot,
    server_helpers::{
        is_anthropic_path, maybe_force_openai_stream_options_include_usage,
        maybe_transform_request_body,
    },
    RequestMeta,
};
use super::{
    inbound::{log_request_error, InboundRequest},
    resolve_outbound_path, DispatchPlan, ProxyState, LOCAL_UPSTREAM_ID,
};

pub(crate) struct PreparedRequest {
    pub(crate) path: String,
    pub(crate) outbound_path_with_query: String,
    pub(crate) plan: DispatchPlan,
    pub(crate) meta: RequestMeta,
    pub(crate) client_gemini_api_key: Option<String>,
    pub(crate) request_detail: Option<RequestDetailSnapshot>,
    pub(crate) source_body: ReplayableBody,
    pub(crate) outbound_body: ReplayableBody,
    pub(crate) request_auth: http::RequestAuth,
}

pub(super) fn build_outbound_path_with_query(outbound_path: &str, uri: &Uri) -> String {
    let Some(query) = uri.query() else {
        return outbound_path.to_string();
    };
    let outbound_query = sanitize_outbound_query(uri.path(), outbound_path, query);
    if outbound_query.is_empty() {
        return outbound_path.to_string();
    }
    format!("{outbound_path}?{outbound_query}")
}

pub(super) fn resolve_request_auth_or_respond(
    config: &ProxyConfig,
    headers: &HeaderMap,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    provider: &str,
    request_start: Instant,
) -> Result<http::RequestAuth, Response> {
    match http::resolve_request_auth(config, headers, path) {
        Ok(auth) => Ok(auth),
        Err(message) => {
            log_request_error(
                log,
                request_detail,
                path,
                provider,
                LOCAL_UPSTREAM_ID,
                StatusCode::UNAUTHORIZED,
                message.clone(),
                request_start,
            );
            Err(http::error_response(StatusCode::UNAUTHORIZED, message))
        }
    }
}

pub(super) async fn finalize_prepared_request(
    state: &ProxyState,
    headers: &HeaderMap,
    uri: &Uri,
    inbound: InboundRequest,
    request_start: Instant,
) -> Result<PreparedRequest, Response> {
    let source_body = inbound.body.clone();
    let outbound_path = resolve_outbound_path(&inbound.path, &inbound.plan, &inbound.meta);
    let outbound_path_with_query = build_outbound_path_with_query(&outbound_path, uri);
    let client_gemini_api_key =
        http::resolve_client_gemini_api_key(&state.config, headers, &inbound.path, uri.query())
            .map_err(|message| http::error_response(StatusCode::UNAUTHORIZED, message))?;
    let outbound_body = build_outbound_body_or_respond(
        &state.http_clients,
        &state.log,
        inbound.request_detail.clone(),
        &inbound.path,
        &inbound.plan,
        &inbound.meta,
        inbound.body,
        request_start,
    )
    .await?;
    let request_auth = resolve_request_auth_or_respond(
        &state.config,
        headers,
        &state.log,
        inbound.request_detail.clone(),
        &inbound.path,
        inbound.plan.provider,
        request_start,
    )?;
    Ok(PreparedRequest {
        path: inbound.path,
        outbound_path_with_query,
        plan: inbound.plan,
        meta: inbound.meta,
        client_gemini_api_key,
        request_detail: inbound.request_detail,
        source_body,
        outbound_body,
        request_auth,
    })
}

pub(super) async fn build_outbound_body_or_respond(
    http_clients: &super::super::http_client::ProxyHttpClients,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    let body = transform_body_or_respond(
        http_clients,
        log,
        request_detail.clone(),
        path,
        plan,
        meta,
        body,
        request_start,
    )
    .await?;
    apply_openai_stream_options_or_respond(
        log,
        request_detail,
        path,
        plan,
        meta,
        body,
        request_start,
    )
    .await
}

async fn transform_body_or_respond(
    http_clients: &super::super::http_client::ProxyHttpClients,
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match maybe_transform_request_body(
        http_clients,
        plan.provider,
        path,
        plan.request_transform,
        meta.original_model.as_deref(),
        body,
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(err) => {
            log_request_error(
                log,
                request_detail,
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            Err(http::error_response(err.status, err.message))
        }
    }
}

async fn apply_openai_stream_options_or_respond(
    log: &Arc<LogWriter>,
    request_detail: Option<RequestDetailSnapshot>,
    path: &str,
    plan: &DispatchPlan,
    meta: &RequestMeta,
    body: ReplayableBody,
    request_start: Instant,
) -> Result<ReplayableBody, Response> {
    match maybe_force_openai_stream_options_include_usage(
        plan.provider,
        plan.outbound_path.unwrap_or(path),
        meta,
        body,
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(err) => {
            log_request_error(
                log,
                request_detail,
                path,
                plan.provider,
                LOCAL_UPSTREAM_ID,
                err.status,
                err.message.clone(),
                request_start,
            );
            Err(http::error_response(err.status, err.message))
        }
    }
}

fn sanitize_outbound_query(inbound_path: &str, outbound_path: &str, query: &str) -> String {
    if !is_anthropic_path(inbound_path) || is_anthropic_path(outbound_path) {
        return query.to_string();
    }
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key != "beta")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if pairs.is_empty() {
        return String::new();
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(&key, &value);
    }
    serializer.finish()
}
