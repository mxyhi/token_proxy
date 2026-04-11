use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};

use super::super::http::RequestAuth;
use super::super::{
    config::{ProviderUpstreams, UpstreamRuntime},
    gemini, http, ProxyState, RequestMeta,
};
use super::{request, AttemptOutcome, PreparedUpstreamRequest, ResolvedUpstreamAuth};

pub(super) async fn prepare_upstream_request(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
) -> Result<PreparedUpstreamRequest, AttemptOutcome> {
    let mapped_meta = build_mapped_meta(meta, upstream);
    let upstream_path_with_query =
        resolve_upstream_path_with_query(provider, upstream_path_with_query, &mapped_meta);
    let upstream_url = upstream.upstream_url(&upstream_path_with_query);
    let resolved = resolve_upstream_auth(
        state,
        provider,
        upstream,
        request_auth,
        &upstream_path_with_query,
        &upstream_url,
    )
    .await?;
    let ResolvedUpstreamAuth {
        upstream_url,
        auth,
        extra_headers,
        proxy_url,
        selected_account_id,
    } = resolved;
    let request_headers = request::build_request_headers(
        provider,
        inbound_path,
        headers,
        auth,
        extra_headers.as_ref(),
        upstream.header_overrides.as_deref(),
    );
    Ok(PreparedUpstreamRequest {
        upstream_path_with_query,
        upstream_url,
        request_headers,
        proxy_url,
        selected_account_id,
        meta: mapped_meta,
    })
}

async fn resolve_upstream_auth(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    request_auth: &RequestAuth,
    upstream_path_with_query: &str,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    if provider == "gemini" {
        let (upstream_url, auth) = request::resolve_gemini_upstream(
            upstream,
            request_auth,
            upstream_path_with_query,
            upstream_url,
        )?;
        return Ok(ResolvedUpstreamAuth {
            upstream_url,
            auth,
            extra_headers: None,
            proxy_url: upstream.proxy_url.clone(),
            selected_account_id: None,
        });
    }
    if provider == "kiro" {
        return resolve_kiro_upstream(state, upstream, upstream_url).await;
    }
    if provider == "codex" {
        return resolve_codex_upstream(state, upstream, upstream_url).await;
    }
    let auth = match http::resolve_upstream_auth(provider, upstream, request_auth) {
        Ok(Some(auth)) => auth,
        Ok(None) => return Err(AttemptOutcome::SkippedAuth),
        Err(response) => return Err(AttemptOutcome::Fatal(response)),
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth,
        extra_headers: None,
        proxy_url: upstream.proxy_url.clone(),
        selected_account_id: None,
    })
}

async fn resolve_kiro_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let ordered_account_ids = if upstream
        .kiro_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        None
    } else {
        Some(ordered_runtime_account_ids(state, "kiro").await)
    };
    let (selected_account_id, record) = state
        .kiro_accounts
        .resolve_account_record_with_order(
            upstream.kiro_account_id.as_deref(),
            ordered_account_ids.as_deref(),
        )
        .await
        .map_err(|err| {
            AttemptOutcome::Fatal(http::error_response(StatusCode::UNAUTHORIZED, err))
        })?;
    let global_proxy_url = state.kiro_accounts.app_proxy_url().await;
    let proxy_url = record
        .proxy_url
        .clone()
        .or_else(|| upstream.proxy_url.clone())
        .or(global_proxy_url);
    let value = http::bearer_header(&record.access_token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers: None,
        proxy_url,
        selected_account_id: Some(selected_account_id),
    })
}

async fn resolve_codex_upstream(
    state: &ProxyState,
    upstream: &UpstreamRuntime,
    upstream_url: &str,
) -> Result<ResolvedUpstreamAuth, AttemptOutcome> {
    let has_pinned_account = upstream
        .codex_account_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let ordered_account_ids = if has_pinned_account {
        None
    } else {
        Some(ordered_runtime_account_ids(state, "codex").await)
    };
    let (selected_account_id, record) = state
        .codex_accounts
        .resolve_account_record_with_order(
            upstream.codex_account_id.as_deref(),
            ordered_account_ids.as_deref(),
        )
        .await
        .map_err(|err| codex_account_resolution_outcome(has_pinned_account, err))?;
    let global_proxy_url = state.codex_accounts.app_proxy_url().await;
    let proxy_url = record
        .proxy_url
        .clone()
        .or_else(|| upstream.proxy_url.clone())
        .or(global_proxy_url);
    let value = http::bearer_header(&record.access_token).ok_or_else(|| {
        AttemptOutcome::Fatal(http::error_response(
            StatusCode::UNAUTHORIZED,
            "Upstream access token contains invalid characters.",
        ))
    })?;
    let mut extra_headers = HeaderMap::new();
    if let Some(account_id) = record.account_id.as_deref() {
        if let Ok(value) = axum::http::HeaderValue::from_str(account_id) {
            extra_headers.insert(
                axum::http::HeaderName::from_static("chatgpt-account-id"),
                value,
            );
        }
    }
    let extra_headers = if extra_headers.is_empty() {
        None
    } else {
        Some(extra_headers)
    };
    Ok(ResolvedUpstreamAuth {
        upstream_url: upstream_url.to_string(),
        auth: http::UpstreamAuthHeader {
            name: AUTHORIZATION,
            value,
        },
        extra_headers,
        proxy_url,
        selected_account_id: Some(selected_account_id),
    })
}

fn codex_account_resolution_outcome(has_pinned_account: bool, err: String) -> AttemptOutcome {
    let response = http::error_response(StatusCode::UNAUTHORIZED, err.clone());
    if has_pinned_account {
        return AttemptOutcome::Fatal(response);
    }
    AttemptOutcome::Retryable {
        message: err,
        response: Some(response),
        is_timeout: false,
        should_cooldown: false,
    }
}

pub(super) async fn ordered_runtime_account_ids(state: &ProxyState, provider: &str) -> Vec<String> {
    let account_ids = match provider {
        "kiro" => state.kiro_accounts.list_accounts().await.map(|items| {
            items
                .into_iter()
                .map(|item| item.account_id)
                .collect::<Vec<_>>()
        }),
        "codex" => state.codex_accounts.list_accounts().await.map(|items| {
            items
                .into_iter()
                .map(|item| item.account_id)
                .collect::<Vec<_>>()
        }),
        _ => Ok(Vec::new()),
    }
    .unwrap_or_default();
    state
        .account_selector
        .order_accounts(provider, &account_ids)
}

pub(super) fn build_mapped_meta(meta: &RequestMeta, upstream: &UpstreamRuntime) -> RequestMeta {
    let upstream_input_model = meta.original_model.as_deref().map(|original| {
        strip_target_upstream_prefix(original, upstream.id.as_str())
            .unwrap_or_else(|| original.to_string())
    });
    let mapped_model = upstream_input_model
        .as_deref()
        .and_then(|original| upstream.map_model(original))
        .or_else(|| {
            let mapped_input = upstream_input_model.as_deref()?;
            let original = meta.original_model.as_deref()?;
            (mapped_input != original).then(|| mapped_input.to_string())
        });
    let (mapped_model, reasoning_effort) =
        normalize_mapped_model_reasoning_suffix(mapped_model, meta.reasoning_effort.clone());
    RequestMeta {
        stream: meta.stream,
        original_model: meta.original_model.clone(),
        mapped_model,
        reasoning_effort,
        estimated_input_tokens: meta.estimated_input_tokens,
    }
}

pub(super) fn requested_target_upstream_id(
    upstreams: &ProviderUpstreams,
    original_model: Option<&str>,
) -> Option<String> {
    let original_model = original_model?.trim();
    let (prefix, rest) = original_model.split_once('/')?;
    if prefix.trim().is_empty() || rest.trim().is_empty() {
        return None;
    }
    upstreams
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .find(|upstream| upstream.id == prefix)
        .map(|upstream| upstream.id.clone())
}

fn strip_target_upstream_prefix(model: &str, upstream_id: &str) -> Option<String> {
    let (prefix, rest) = model.split_once('/')?;
    if prefix != upstream_id || rest.trim().is_empty() {
        return None;
    }
    Some(rest.to_string())
}

pub(super) fn normalize_mapped_model_reasoning_suffix(
    mapped_model: Option<String>,
    reasoning_effort: Option<String>,
) -> (Option<String>, Option<String>) {
    let Some(mapped_model) = mapped_model else {
        return (None, reasoning_effort);
    };
    let Some((base_model, mapped_effort)) =
        super::super::server_helpers::parse_openai_reasoning_effort_from_model_suffix(
            &mapped_model,
        )
    else {
        return (Some(mapped_model), reasoning_effort);
    };

    let reasoning_effort = reasoning_effort.or(Some(mapped_effort));
    (Some(base_model), reasoning_effort)
}

fn resolve_upstream_path_with_query(
    provider: &str,
    upstream_path_with_query: &str,
    meta: &RequestMeta,
) -> String {
    if provider != "gemini" || meta.model_override().is_none() {
        return upstream_path_with_query.to_string();
    }
    let Some(mapped_model) = meta.mapped_model.as_deref() else {
        return upstream_path_with_query.to_string();
    };
    let (path, query) = request::split_path_query(upstream_path_with_query);
    let replaced = gemini::replace_gemini_model_in_path(path, mapped_model)
        .unwrap_or_else(|| path.to_string());
    match query {
        Some(query) => format!("{replaced}?{query}"),
        None => replaced,
    }
}
