use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use super::super::http::RequestAuth;
use super::super::{
    codex_compat::supported_codex_model_ids,
    config::{expand_model_ids_with_mappings, UpstreamRuntime},
    http,
    model_discovery::{UpstreamModelProbe, UpstreamModelProbeStatus},
    request_body::ReplayableBody,
    ProxyState, RequestMeta,
};
use super::{utils::sanitize_upstream_error, AttemptOutcome};

const MODEL_DISCOVERY_MAX_PARALLEL: usize = 8;

#[derive(Clone)]
struct ModelDiscoveryJob {
    provider: String,
    upstream: UpstreamRuntime,
    account_id: Option<String>,
}

pub(super) async fn aggregate_model_catalog_request(
    state: Arc<ProxyState>,
    provider: &str,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    request_auth: &RequestAuth,
) -> Response {
    let Some(provider_upstreams) = state.config.provider_upstreams(provider) else {
        return http::error_response(StatusCode::BAD_GATEWAY, "No available upstream configured.");
    };

    let mut sources: Vec<(String, Vec<String>)> = Vec::new();
    let mut successful = 0usize;
    let meta = RequestMeta {
        stream: false,
        original_model: None,
        mapped_model: None,
        reasoning_effort: None,
        estimated_input_tokens: None,
    };
    let empty_body = ReplayableBody::from_bytes(Bytes::new());

    for group in &provider_upstreams.groups {
        for upstream in &group.items {
            let upstream_model_catalog = fetch_upstream_model_catalog(
                state.as_ref(),
                provider,
                upstream,
                inbound_path,
                upstream_path_with_query,
                headers,
                &meta,
                request_auth,
                &empty_body,
            )
            .await;
            let mut models = upstream.advertised_model_ids.clone();
            expand_model_ids_with_mappings(&mut models, &state.config.hot_model_mappings);
            match upstream_model_catalog {
                Ok(fetched_models) => {
                    successful += 1;
                    merge_model_catalog_ids(&mut models, fetched_models);
                    expand_model_ids_with_mappings(&mut models, &state.config.hot_model_mappings);
                    sources.push((upstream.id.clone(), models));
                }
                Err(err) => {
                    if !models.is_empty() {
                        successful += 1;
                        sources.push((upstream.id.clone(), models));
                        continue;
                    }
                    tracing::warn!(
                        provider = %provider,
                        upstream = %upstream.id,
                        error = %err,
                        "failed to fetch upstream model catalog"
                    );
                }
            }
        }
    }

    if successful == 0 {
        return http::error_response(
            StatusCode::BAD_GATEWAY,
            "No upstream model catalog available.",
        );
    }

    let response_body = build_model_catalog_response_body(&sources, state.config.model_list_prefix);
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    http::build_response(
        StatusCode::OK,
        response_headers,
        Body::from(response_body.to_string()),
    )
}

fn merge_model_catalog_ids(target: &mut Vec<String>, extra: Vec<String>) {
    let mut seen = target.iter().cloned().collect::<HashSet<_>>();
    for model in extra {
        if seen.insert(model.clone()) {
            target.push(model);
        }
    }
}

pub(super) async fn refresh_model_discovery(state: Arc<ProxyState>) {
    let jobs = collect_model_discovery_jobs(&state);
    let pending = jobs
        .iter()
        .map(|job| {
            UpstreamModelProbe::pending(
                job.upstream.id.as_str(),
                job.provider.as_str(),
                job.account_id.clone(),
            )
        })
        .collect();
    state.model_discovery.replace_all(pending).await;

    let completed = stream::iter(jobs.into_iter().enumerate())
        .map(|(index, job)| {
            let state = state.clone();
            async move {
                let probe = refresh_model_discovery_job(state.as_ref(), job).await;
                (index, probe)
            }
        })
        .buffer_unordered(MODEL_DISCOVERY_MAX_PARALLEL)
        .collect::<Vec<_>>()
        .await;

    for (index, probe) in completed {
        state.model_discovery.replace_at(index, probe).await;
    }
}

fn collect_model_discovery_jobs(state: &ProxyState) -> Vec<ModelDiscoveryJob> {
    let mut jobs = Vec::new();
    for (provider, provider_upstreams) in &state.config.upstreams {
        for group in &provider_upstreams.groups {
            for upstream in &group.items {
                jobs.push(ModelDiscoveryJob {
                    provider: provider.clone(),
                    upstream: upstream.clone(),
                    account_id: probe_account_id(upstream),
                });
            }
        }
    }
    jobs.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.upstream.id.cmp(&right.upstream.id))
            .then_with(|| left.account_id.cmp(&right.account_id))
    });
    jobs
}

fn probe_account_id(upstream: &UpstreamRuntime) -> Option<String> {
    upstream
        .kiro_account_id
        .clone()
        .or_else(|| upstream.codex_account_id.clone())
        .or_else(|| (upstream.selector_key != upstream.id).then(|| upstream.selector_key.clone()))
}

async fn refresh_model_discovery_job(
    state: &ProxyState,
    job: ModelDiscoveryJob,
) -> UpstreamModelProbe {
    let mut models = job.upstream.advertised_model_ids.clone();
    merge_model_catalog_ids(&mut models, builtin_model_ids(job.provider.as_str()));
    expand_model_ids_with_mappings(&mut models, &state.config.hot_model_mappings);

    let Some((inbound_path, upstream_path)) = model_catalog_probe_paths(job.provider.as_str())
    else {
        let (status, error) = if models.is_empty() {
            (
                UpstreamModelProbeStatus::Unsupported,
                Some("Model list endpoint is not supported for this provider.".to_string()),
            )
        } else {
            (UpstreamModelProbeStatus::Ok, None)
        };
        return UpstreamModelProbe::completed(
            job.upstream.id.as_str(),
            job.provider.as_str(),
            job.account_id,
            status,
            error,
            models,
        );
    };

    let meta = RequestMeta {
        stream: false,
        original_model: None,
        mapped_model: None,
        reasoning_effort: None,
        estimated_input_tokens: None,
    };
    let headers = HeaderMap::new();
    let request_auth = RequestAuth::default();
    let empty_body = ReplayableBody::from_bytes(Bytes::new());

    match fetch_upstream_model_catalog(
        state,
        job.provider.as_str(),
        &job.upstream,
        inbound_path,
        upstream_path,
        &headers,
        &meta,
        &request_auth,
        &empty_body,
    )
    .await
    {
        Ok(fetched_models) => {
            merge_model_catalog_ids(&mut models, fetched_models);
            expand_model_ids_with_mappings(&mut models, &state.config.hot_model_mappings);
            UpstreamModelProbe::completed(
                job.upstream.id.as_str(),
                job.provider.as_str(),
                job.account_id,
                UpstreamModelProbeStatus::Ok,
                None,
                models,
            )
        }
        Err(error) => UpstreamModelProbe::completed(
            job.upstream.id.as_str(),
            job.provider.as_str(),
            job.account_id,
            UpstreamModelProbeStatus::Failed,
            Some(error),
            models,
        ),
    }
}

fn builtin_model_ids(provider: &str) -> Vec<String> {
    match provider {
        "codex" => supported_codex_model_ids(),
        _ => Vec::new(),
    }
}

fn model_catalog_probe_paths(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "openai" | "openai-response" | "anthropic" => Some(("/v1/models", "/v1/models")),
        "gemini" => Some(("/v1beta/models", "/v1beta/models")),
        _ => None,
    }
}

async fn fetch_upstream_model_catalog(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
    headers: &HeaderMap,
    meta: &RequestMeta,
    request_auth: &RequestAuth,
    body: &ReplayableBody,
) -> Result<Vec<String>, String> {
    let prepared = super::prepare_upstream_request(
        state,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        headers,
        meta,
        request_auth,
        &crate::proxy::cooldown_scope::CooldownScope::Global,
    )
    .await
    .map_err(model_catalog_prepare_error)?;

    let client = state
        .http_clients
        .client_for_proxy_url(prepared.proxy_url.as_deref())?;
    let request_body = body
        .to_reqwest_body()
        .await
        .map_err(|err| format!("Failed to build upstream request body: {err}"))?;
    let request = client
        .request(Method::GET, &prepared.upstream_url)
        .headers(prepared.request_headers)
        .body(request_body);
    let response = tokio::time::timeout(state.config.upstream_no_data_timeout, request.send())
        .await
        .map_err(|_| "Timed out fetching upstream model catalog.".to_string())?
        .map_err(|err| {
            format!(
                "Failed to fetch upstream model catalog: {}",
                sanitize_upstream_error(provider, &err)
            )
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "Upstream model catalog returned status {}.",
            response.status()
        ));
    }

    let value = response
        .json::<Value>()
        .await
        .map_err(|err| format!("Failed to parse upstream model catalog JSON: {err}"))?;
    Ok(extract_model_ids_from_catalog(&value))
}

fn model_catalog_prepare_error(outcome: AttemptOutcome) -> String {
    match outcome {
        AttemptOutcome::SkippedAuth => {
            "No API key available for upstream model catalog.".to_string()
        }
        AttemptOutcome::Retryable { message, .. } => message,
        AttemptOutcome::Fatal(response) => {
            format!(
                "Failed to prepare upstream model catalog request: status {}.",
                response.status()
            )
        }
        AttemptOutcome::Success(_) => {
            "Unexpected upstream model catalog preparation result.".to_string()
        }
    }
}

fn extract_model_ids_from_catalog(value: &Value) -> Vec<String> {
    if let Some(items) = value.get("data").and_then(Value::as_array) {
        return items
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("name").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches("models/").to_string())
        .collect()
}

fn build_model_catalog_response_body(
    sources: &[(String, Vec<String>)],
    include_prefixed: bool,
) -> Value {
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut upstreams_by_model: HashMap<String, Vec<String>> = HashMap::new();
    let mut base_order = Vec::new();

    for (upstream_id, models) in sources {
        let mut seen = HashSet::new();
        for model in models {
            let trimmed = model.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                continue;
            }
            if !upstreams_by_model.contains_key(trimmed) {
                base_order.push(trimmed.to_string());
            }
            upstreams_by_model
                .entry(trimmed.to_string())
                .or_default()
                .push(upstream_id.clone());
        }
    }

    let mut data = Vec::new();
    for model in base_order {
        let Some(upstream_ids) = upstreams_by_model.get(&model) else {
            continue;
        };
        if include_prefixed {
            if upstream_ids.len() > 1 {
                data.push(model_catalog_item(model.as_str(), model.as_str(), created));
            }
            for upstream_id in upstream_ids {
                let prefixed = format!("{upstream_id}/{model}");
                data.push(model_catalog_item(&prefixed, upstream_id.as_str(), created));
            }
            continue;
        }
        data.push(model_catalog_item(model.as_str(), "token_proxy", created));
    }

    json!({
        "object": "list",
        "data": data,
    })
}

fn model_catalog_item(id: &str, owned_by: &str, created: i64) -> Value {
    json!({
        "id": id,
        "object": "model",
        "created": created,
        "owned_by": owned_by,
    })
}
