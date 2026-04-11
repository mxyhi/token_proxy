use super::kiro_http::{build_client, read_request_json, refresh_kiro_account};
use super::AttemptOutcome;
use crate::kiro::KiroTokenRecord;
use crate::proxy::http;
use crate::proxy::kiro::{
    build_payload_from_claude, build_payload_from_responses, determine_agentic_mode,
    map_model_to_kiro, select_endpoints, BuildPayloadResult, KiroEndpointConfig,
};
use crate::proxy::openai_compat::FormatTransform;
use crate::proxy::request_body::ReplayableBody;
use crate::proxy::{config::UpstreamRuntime, request_detail::RequestDetailSnapshot};
use crate::proxy::{ProxyState, RequestMeta};
use axum::http::{HeaderMap, Method, StatusCode};
use serde_json::Value;

pub(super) struct KiroContext<'a> {
    pub(super) state: &'a ProxyState,
    pub(super) method: Method,
    pub(super) upstream: &'a UpstreamRuntime,
    pub(super) inbound_path: &'a str,
    pub(super) headers: &'a HeaderMap,
    pub(super) response_transform: FormatTransform,
    pub(super) request_detail: Option<RequestDetailSnapshot>,
    pub(super) mapped_meta: RequestMeta,
    pub(super) request_value: Value,
    pub(super) account_id: String,
    pub(super) record: KiroTokenRecord,
    pub(super) profile_arn: Option<String>,
    pub(super) endpoints: Vec<KiroEndpointConfig>,
    pub(super) is_idc: bool,
    pub(super) model_id: String,
    pub(super) is_agentic: bool,
    pub(super) is_chat_only: bool,
    pub(super) source_format: KiroSourceFormat,
    pub(super) client: reqwest::Client,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum KiroSourceFormat {
    Responses,
    Anthropic,
}

pub(super) async fn prepare_kiro_context<'a>(
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
    let ordered_account_ids = if upstream
        .kiro_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        None
    } else {
        Some(super::ordered_runtime_account_ids(state, "kiro").await)
    };
    let (account_id, record) = state
        .kiro_accounts
        .resolve_account_record_with_order(
            upstream.kiro_account_id.as_deref(),
            ordered_account_ids.as_deref(),
        )
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err))
        })?;
    let is_idc = record.auth_method.trim().eq_ignore_ascii_case("idc");
    let profile_arn = resolve_profile_arn(&record);
    let endpoints = resolve_endpoints(state, upstream, is_idc);
    let (model_id, is_agentic, is_chat_only) = resolve_model(&mapped_meta);
    let source_format = resolve_source_format(response_transform);
    let client_proxy_url = record
        .proxy_url
        .clone()
        .or_else(|| upstream.proxy_url.clone())
        .or(state.kiro_accounts.app_proxy_url().await);
    let client = build_client(state, client_proxy_url.as_deref())?;

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
        profile_arn,
        endpoints,
        is_idc,
        model_id,
        is_agentic,
        is_chat_only,
        source_format,
        client,
    })
}

pub(super) async fn build_endpoint_payload(
    context: &KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
) -> Result<BuildPayloadResult, AttemptOutcome> {
    let payload = match context.source_format {
        KiroSourceFormat::Anthropic => build_payload_from_anthropic(context, endpoint.origin).await,
        KiroSourceFormat::Responses => build_payload_from_responses(
            &context.request_value,
            &context.model_id,
            context.profile_arn.as_deref(),
            endpoint.origin,
            context.is_agentic,
            context.is_chat_only,
            context.headers,
        ),
    };
    payload.map_err(|message| {
        AttemptOutcome::Fatal(http::error_response(StatusCode::BAD_REQUEST, message))
    })
}

pub(super) async fn refresh_and_rebuild_payload(
    context: &mut KiroContext<'_>,
    endpoint: &KiroEndpointConfig,
) -> Result<BuildPayloadResult, AttemptOutcome> {
    refresh_kiro_account(context.state, &context.account_id).await?;
    context.record = load_account_record(context.state, &context.account_id).await?;
    let was_idc = context.is_idc;
    context.is_idc = context
        .record
        .auth_method
        .trim()
        .eq_ignore_ascii_case("idc");
    if context.is_idc != was_idc {
        context.endpoints = resolve_endpoints(context.state, context.upstream, context.is_idc);
    }
    build_endpoint_payload(context, endpoint).await
}

async fn build_payload_from_anthropic(
    context: &KiroContext<'_>,
    origin: &str,
) -> Result<BuildPayloadResult, String> {
    build_payload_from_claude(
        &context.request_value,
        &context.model_id,
        context.profile_arn.as_deref(),
        origin,
        context.is_agentic,
        context.is_chat_only,
        context.headers,
    )
}

fn resolve_profile_arn(record: &KiroTokenRecord) -> Option<String> {
    match record.auth_method.as_str() {
        "builder-id" | "idc" => None,
        _ => record.profile_arn.clone(),
    }
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
    select_endpoints(preferred, is_idc, Some(upstream.base_url.as_str()))
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

fn resolve_source_format(transform: FormatTransform) -> KiroSourceFormat {
    match transform {
        FormatTransform::KiroToAnthropic => KiroSourceFormat::Anthropic,
        _ => KiroSourceFormat::Responses,
    }
}
