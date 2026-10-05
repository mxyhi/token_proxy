use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::super::http::RequestAuth;
use super::super::{
    codex_compat::supported_codex_model_ids,
    config::{expand_model_ids_with_mappings, UpstreamRuntime},
    http,
    model_discovery::{
        ModelCatalogEntry, ModelCatalogKey, UpstreamModelProbe, UpstreamModelProbeStatus,
    },
    ProxyState, RequestMeta,
};
use super::{utils::sanitize_upstream_error, AttemptOutcome};

const MODEL_DISCOVERY_MAX_PARALLEL: usize = 8;
/// 单个上游目录探测上限；与 sync_response_timeout 取较小值，避免一个慢上游拖住整轮刷新。
const MODEL_CATALOG_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

type FetchedModelCatalogs = HashMap<ModelCatalogKey, Vec<ModelCatalogEntry>>;

#[derive(Clone)]
struct ModelDiscoveryJob {
    provider: String,
    upstream: UpstreamRuntime,
    account_id: Option<String>,
}

/// 聚合全部已配置 provider 的模型目录（OpenAI 兼容 `/v1/models` 入口）。
/// 不再按 priority 只选一个 provider，避免模型选择器只露出 1/3 上游。
/// 远端目录只读后台探测缓存，请求路径不访问上游（逐个实时拉取会超过网关超时）。
pub(super) async fn aggregate_all_providers_model_catalog(state: Arc<ProxyState>) -> Response {
    let mut providers: Vec<String> = state.config.upstreams.keys().cloned().collect();
    providers.sort();
    let fetched = state.model_discovery.fetched_catalogs().await;
    tracing::debug!(
        provider_count = providers.len(),
        providers = ?providers,
        cached_catalogs = fetched.len(),
        "aggregating cached model catalog across all providers"
    );

    let mut sources: Vec<(String, Vec<ModelCatalogEntry>)> = Vec::new();
    let mut successful = 0usize;
    for provider in &providers {
        let (count, provider_sources) =
            collect_provider_model_sources(state.as_ref(), provider, &fetched);
        successful += count;
        sources.extend(provider_sources);
    }

    if successful == 0 {
        return http::error_response(
            StatusCode::BAD_GATEWAY,
            "No upstream model catalog available.",
        );
    }

    model_catalog_list_response(&sources, state.config.model_list_prefix)
}

/// 为 Codex App manifest 提供跨 provider 的稳定模型键与展示名。
///
/// manifest 只补充本地目录中不存在的模型；原始 Codex manifest 的字段和顺序由调用方保留。
pub(super) async fn collect_model_catalog_entries_for_manifest(
    state: &ProxyState,
) -> Vec<(String, Option<String>)> {
    let mut providers: Vec<String> = state.config.upstreams.keys().cloned().collect();
    providers.sort();
    let fetched = state.model_discovery.fetched_catalogs().await;
    let mut sources = Vec::new();
    for provider in providers {
        // Codex manifest already owns the GPT catalog returned by ChatGPT; only supplement it
        // with models from other configured providers/local gateways.
        if provider == "codex" {
            continue;
        }
        let (_, provider_sources) = collect_provider_model_sources(state, &provider, &fetched);
        sources.extend(provider_sources);
    }

    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    for (_, models) in sources {
        for model in models {
            if let Some(index) = positions.get(&model.id).copied() {
                if entries[index].1.is_none() {
                    entries[index].1 = model.display_name;
                }
                continue;
            }
            positions.insert(model.id.clone(), entries.len());
            entries.push((model.id, model.display_name));
        }
    }
    tracing::debug!(
        model_count = entries.len(),
        "collected models for Codex manifest augmentation"
    );
    entries
}

/// 按当前配置叠加缓存的远端目录；返回 (可用来源数, [(upstream_id, models)])。
fn collect_provider_model_sources(
    state: &ProxyState,
    provider: &str,
    fetched: &FetchedModelCatalogs,
) -> (usize, Vec<(String, Vec<ModelCatalogEntry>)>) {
    let Some(provider_upstreams) = state.config.provider_upstreams(provider) else {
        return (0, Vec::new());
    };

    let mut sources: Vec<(String, Vec<ModelCatalogEntry>)> = Vec::new();
    let mut successful = 0usize;
    for group in &provider_upstreams.groups {
        for upstream in &group.items {
            let upstream_catalog = fetched.get(&model_catalog_key(provider, upstream));
            let models = resolve_upstream_model_entries(
                state,
                provider,
                upstream,
                upstream_catalog.cloned(),
            );
            // 远端目录拉取成功（即使为空）或本地可广告模型非空，都算可用来源。
            if upstream_catalog.is_some() || !models.is_empty() {
                successful += 1;
                sources.push((upstream.id.clone(), models));
            }
        }
    }

    (successful, sources)
}

/// 本地可广告模型 + 内置目录为底，再叠加远端目录；映射与 available_models 按当前配置实时生效。
fn resolve_upstream_model_entries(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    fetched: Option<Vec<ModelCatalogEntry>>,
) -> Vec<ModelCatalogEntry> {
    let mut models = model_catalog_entries_from_ids(&upstream.advertised_model_ids);
    merge_model_catalog_entries(
        &mut models,
        model_catalog_entries_from_ids(&builtin_model_ids(provider)),
    );
    expand_model_catalog_entries_with_mappings(&mut models, &state.config.hot_model_mappings);
    restrict_model_catalog_entries(upstream, &mut models);
    let Some(fetched) = fetched else {
        return models;
    };
    merge_model_catalog_entries(&mut models, fetched);
    expand_model_catalog_entries_with_mappings(&mut models, &state.config.hot_model_mappings);
    restrict_model_catalog_entries(upstream, &mut models);
    models
}

fn model_catalog_key(provider: &str, upstream: &UpstreamRuntime) -> ModelCatalogKey {
    ModelCatalogKey {
        provider: provider.to_string(),
        upstream_id: upstream.id.clone(),
        account_id: probe_account_id(upstream),
    }
}

fn model_catalog_list_response(
    sources: &[(String, Vec<ModelCatalogEntry>)],
    include_prefixed: bool,
) -> Response {
    let response_body = build_model_catalog_response_body(sources, include_prefixed);
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

fn model_catalog_entries_from_ids(ids: &[String]) -> Vec<ModelCatalogEntry> {
    ids.iter()
        .filter_map(|id| {
            let id = id.trim();
            (!id.is_empty()).then(|| ModelCatalogEntry {
                id: id.to_string(),
                display_name: None,
            })
        })
        .collect()
}

fn merge_model_catalog_entries(target: &mut Vec<ModelCatalogEntry>, extra: Vec<ModelCatalogEntry>) {
    let mut positions = target
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.clone(), index))
        .collect::<HashMap<_, _>>();
    for entry in extra {
        if let Some(index) = positions.get(&entry.id).copied() {
            if target[index].display_name.is_none() {
                target[index].display_name = entry.display_name;
            }
            continue;
        }
        positions.insert(entry.id.clone(), target.len());
        target.push(entry);
    }
}

fn expand_model_catalog_entries_with_mappings(
    entries: &mut Vec<ModelCatalogEntry>,
    mappings: &HashMap<String, String>,
) {
    let ids = entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    let mut expanded_ids = ids.clone();
    expand_model_ids_with_mappings(&mut expanded_ids, mappings);
    merge_model_catalog_entries(entries, model_catalog_entries_from_ids(&expanded_ids));
}

fn restrict_model_catalog_entries(
    upstream: &UpstreamRuntime,
    entries: &mut Vec<ModelCatalogEntry>,
) {
    let mut ids = entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    upstream.restrict_model_catalog(&mut ids);
    let allowed = ids.into_iter().collect::<HashSet<_>>();
    entries.retain(|entry| allowed.contains(&entry.id));
}

struct ModelDiscoveryOutcome {
    probe: UpstreamModelProbe,
    /// 本次成功拉到的远端原始目录；None 表示失败或 provider 无目录接口，缓存保留旧值。
    fetched: Option<Vec<ModelCatalogEntry>>,
    elapsed_ms: u64,
}

/// 刷新模型目录缓存（启动、配置热加载、Dashboard 手动刷新触发）。
/// 并发探测且每个上游完成即写入，慢上游不阻塞其它结果。
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
    state.model_discovery.begin_refresh(pending).await;

    let started = Instant::now();
    let job_count = jobs.len();
    let (failed, slowest) = stream::iter(jobs)
        .map(|job| {
            let state = state.clone();
            async move { refresh_model_discovery_job(state.as_ref(), job).await }
        })
        .buffer_unordered(MODEL_DISCOVERY_MAX_PARALLEL)
        .fold(
            (0usize, None::<(String, u64)>),
            |(failed, slowest), outcome| {
                let state = state.clone();
                async move {
                    let failed = failed
                        + usize::from(outcome.probe.status == UpstreamModelProbeStatus::Failed);
                    let slowest = match slowest {
                        Some(current) if current.1 >= outcome.elapsed_ms => Some(current),
                        _ => Some((outcome.probe.upstream_id.clone(), outcome.elapsed_ms)),
                    };
                    state
                        .model_discovery
                        .complete_probe(outcome.probe, outcome.fetched)
                        .await;
                    (failed, slowest)
                }
            },
        )
        .await;
    let (slowest_upstream, slowest_ms) = slowest.unwrap_or_default();
    tracing::info!(
        jobs = job_count,
        failed,
        slowest_upstream = %slowest_upstream,
        slowest_ms,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "model discovery refresh finished"
    );
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
        .or_else(|| upstream.xai_account_id.clone())
        .or_else(|| (upstream.selector_key != upstream.id).then(|| upstream.selector_key.clone()))
}

async fn refresh_model_discovery_job(
    state: &ProxyState,
    job: ModelDiscoveryJob,
) -> ModelDiscoveryOutcome {
    let provider = job.provider.as_str();
    let completed = |status, error, models: Vec<ModelCatalogEntry>| {
        UpstreamModelProbe::completed(
            job.upstream.id.as_str(),
            provider,
            job.account_id.clone(),
            status,
            error,
            models.into_iter().map(|entry| entry.id).collect(),
        )
    };

    let Some((inbound_path, upstream_path)) = model_catalog_probe_paths(provider) else {
        let models = resolve_upstream_model_entries(state, provider, &job.upstream, None);
        let (status, error) = if models.is_empty() {
            (
                UpstreamModelProbeStatus::Unsupported,
                Some("Model list endpoint is not supported for this provider.".to_string()),
            )
        } else {
            (UpstreamModelProbeStatus::Ok, None)
        };
        return ModelDiscoveryOutcome {
            probe: completed(status, error, models),
            fetched: None,
            elapsed_ms: 0,
        };
    };

    let started = Instant::now();
    let probe_timeout = MODEL_CATALOG_PROBE_TIMEOUT.min(state.config.sync_response_timeout);
    let result = tokio::time::timeout(
        probe_timeout,
        fetch_upstream_model_catalog(state, provider, &job.upstream, inbound_path, upstream_path),
    )
    .await
    .unwrap_or_else(|_| Err("Timed out fetching upstream model catalog.".to_string()));
    let elapsed_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(fetched) => {
            tracing::debug!(
                provider = %provider,
                upstream = %job.upstream.id,
                elapsed_ms,
                model_count = fetched.len(),
                "upstream model catalog probed"
            );
            let models = resolve_upstream_model_entries(
                state,
                provider,
                &job.upstream,
                Some(fetched.clone()),
            );
            ModelDiscoveryOutcome {
                probe: completed(UpstreamModelProbeStatus::Ok, None, models),
                fetched: Some(fetched),
                elapsed_ms,
            }
        }
        Err(error) => {
            tracing::warn!(
                provider = %provider,
                upstream = %job.upstream.id,
                elapsed_ms,
                error = %error,
                "failed to probe upstream model catalog"
            );
            // 失败时展示与 /v1/models 一致：本地模型 + 上次成功的远端目录。
            let last_fetched = state
                .model_discovery
                .fetched_catalog(&model_catalog_key(provider, &job.upstream))
                .await;
            let models =
                resolve_upstream_model_entries(state, provider, &job.upstream, last_fetched);
            ModelDiscoveryOutcome {
                probe: completed(UpstreamModelProbeStatus::Failed, Some(error), models),
                fetched: None,
                elapsed_ms,
            }
        }
    }
}

fn builtin_model_ids(provider: &str) -> Vec<String> {
    match provider {
        "codex" => supported_codex_model_ids(),
        "xai" => token_proxy_account_xai::BUILTIN_MODELS
            .iter()
            .map(|model| (*model).to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn model_catalog_probe_paths(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "openai" | "openai-response" | "anthropic" | "xai" => Some(("/v1/models", "/v1/models")),
        "gemini" => Some(("/v1beta/models", "/v1beta/models")),
        _ => None,
    }
}

/// 后台探测不携带客户端请求头，统一使用上游自身凭证。
async fn fetch_upstream_model_catalog(
    state: &ProxyState,
    provider: &str,
    upstream: &UpstreamRuntime,
    inbound_path: &str,
    upstream_path_with_query: &str,
) -> Result<Vec<ModelCatalogEntry>, String> {
    let meta = RequestMeta {
        client_ip: None,
        stream: false,
        original_model: None,
        mapped_model: None,
        reasoning_effort: None,
        response_format: None,
        estimated_input_tokens: None,
        client_request_body: None,
        billing: Default::default(),
    };
    let prepared = super::prepare_upstream_request(
        state,
        provider,
        upstream,
        inbound_path,
        upstream_path_with_query,
        &HeaderMap::new(),
        &meta,
        &RequestAuth::default(),
        &crate::proxy::cooldown_scope::CooldownScope::Global,
    )
    .await
    .map_err(model_catalog_prepare_error)?;

    let client = if provider == "xai" {
        state
            .http_clients
            .xai_client_for_proxy_url(prepared.proxy_url.as_deref())?
    } else {
        state
            .http_clients
            .client_for_proxy_url(prepared.proxy_url.as_deref())?
    };
    let response = client
        .request(Method::GET, &prepared.upstream_url)
        .headers(prepared.request_headers)
        .send()
        .await
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
    Ok(extract_model_entries_from_catalog(provider, &value))
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

fn extract_model_entries_from_catalog(provider: &str, value: &Value) -> Vec<ModelCatalogEntry> {
    let items = model_catalog_items(provider, value);
    let mut models = items
        .into_iter()
        .filter_map(|item| {
            let id = model_catalog_item_id(provider, item)?;
            let display_name = if provider == "gemini" {
                string_field(item, "displayName").or_else(|| string_field(item, "display_name"))
            } else {
                string_field(item, "display_name").or_else(|| string_field(item, "displayName"))
            }
            .map(str::to_string)
            .filter(|value| value != &id);
            Some(ModelCatalogEntry { id, display_name })
        })
        .collect::<Vec<_>>();
    if provider == "xai" {
        models.sort_by(|left, right| left.id.cmp(&right.id));
        let mut seen = HashSet::new();
        models.retain(|entry| seen.insert(entry.id.clone()));
    }
    models
}

fn model_catalog_items<'a>(provider: &str, value: &'a Value) -> Vec<&'a Value> {
    if let Some(items) = value.as_array() {
        return items.iter().collect();
    }
    if provider != "xai" {
        return value
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| value.get("models").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .collect();
    }
    let mut items = Vec::new();
    if let Some(data) = value.get("data").and_then(Value::as_array) {
        items.extend(data);
    }
    if let Some(models) = value.get("models").and_then(Value::as_array) {
        items.extend(models);
    }
    items
}

fn model_catalog_item_id(provider: &str, item: &Value) -> Option<String> {
    let candidate = if provider == "xai" {
        xai_model_catalog_item_id(item)
    } else {
        string_field(item, "id").or_else(|| string_field(item, "name"))
    }?;
    let model = candidate.trim().trim_start_matches("models/").trim();
    (!model.is_empty()).then(|| model.to_string())
}

fn xai_model_catalog_item_id(item: &Value) -> Option<&str> {
    ["model", "modelId", "model_id", "id"]
        .into_iter()
        .find_map(|field| string_field(item, field))
        .or_else(|| {
            let metadata = item.get("_meta")?;
            ["model", "modelId", "model_id", "id", "name"]
                .into_iter()
                .find_map(|field| string_field(metadata, field))
        })
        // xAI `name` 常是展示名，只在没有协议模型 ID 时兼容兜底。
        .or_else(|| string_field(item, "name"))
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn build_model_catalog_response_body(
    sources: &[(String, Vec<ModelCatalogEntry>)],
    include_prefixed: bool,
) -> Value {
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut upstreams_by_model: HashMap<String, Vec<String>> = HashMap::new();
    let mut display_names_by_model: HashMap<String, String> = HashMap::new();
    let mut base_order = Vec::new();

    for (upstream_id, models) in sources {
        let mut seen = HashSet::new();
        for model in models {
            let trimmed = model.id.trim();
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
            if let Some(display_name) = model
                .display_name
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                display_names_by_model
                    .entry(trimmed.to_string())
                    .or_insert_with(|| display_name.to_string());
            }
        }
    }

    let mut data = Vec::new();
    for model in base_order {
        let Some(upstream_ids) = upstreams_by_model.get(&model) else {
            continue;
        };
        if include_prefixed {
            // model_list_prefix：每条上游出 `upstream_id/model`；同名额外保留裸名供轮询/failover。
            if upstream_ids.len() > 1 {
                data.push(model_catalog_item(
                    model.as_str(),
                    model.as_str(),
                    created,
                    display_names_by_model.get(&model),
                ));
            }
            for upstream_id in upstream_ids {
                let prefixed = format!("{upstream_id}/{model}");
                data.push(model_catalog_item(
                    &prefixed,
                    upstream_id.as_str(),
                    created,
                    display_names_by_model.get(&model),
                ));
            }
            continue;
        }
        // 默认：全上游可广告模型去重并集，仅裸名。
        data.push(model_catalog_item(
            model.as_str(),
            "token_proxy",
            created,
            display_names_by_model.get(&model),
        ));
    }

    json!({
        "object": "list",
        "data": data,
    })
}

fn model_catalog_item(
    id: &str,
    owned_by: &str,
    created: i64,
    display_name: Option<&String>,
) -> Value {
    let display_name = display_name
        .filter(|value| !value.is_empty())
        .map(|value| value.clone())
        .unwrap_or_else(|| id.to_string());
    let item = json!({
        "id": id,
        "object": "model",
        "created": created,
        "owned_by": owned_by,
        "display_name": display_name,
    });
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_catalog_response_default_dedupes_bare_ids_without_prefix() {
        let sources = vec![
            (
                "alpha".to_string(),
                vec![
                    ModelCatalogEntry {
                        id: "gpt-unique-a".to_string(),
                        display_name: None,
                    },
                    ModelCatalogEntry {
                        id: "gpt-shared".to_string(),
                        display_name: None,
                    },
                ],
            ),
            (
                "beta".to_string(),
                vec![
                    ModelCatalogEntry {
                        id: "gpt-unique-b".to_string(),
                        display_name: None,
                    },
                    ModelCatalogEntry {
                        id: "gpt-shared".to_string(),
                        display_name: None,
                    },
                ],
            ),
        ];
        let body = build_model_catalog_response_body(&sources, false);
        let mut ids = body["data"]
            .as_array()
            .expect("data")
            .iter()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["gpt-shared", "gpt-unique-a", "gpt-unique-b"]);
    }

    #[test]
    fn model_catalog_response_prefix_mode_emits_upstream_id_paths() {
        let sources = vec![
            (
                "alpha".to_string(),
                vec![
                    ModelCatalogEntry {
                        id: "gpt-unique-a".to_string(),
                        display_name: None,
                    },
                    ModelCatalogEntry {
                        id: "gpt-shared".to_string(),
                        display_name: None,
                    },
                ],
            ),
            (
                "beta".to_string(),
                vec![
                    ModelCatalogEntry {
                        id: "gpt-unique-b".to_string(),
                        display_name: None,
                    },
                    ModelCatalogEntry {
                        id: "gpt-shared".to_string(),
                        display_name: None,
                    },
                ],
            ),
        ];
        let body = build_model_catalog_response_body(&sources, true);
        let mut ids = body["data"]
            .as_array()
            .expect("data")
            .iter()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "alpha/gpt-shared",
                "alpha/gpt-unique-a",
                "beta/gpt-shared",
                "beta/gpt-unique-b",
                "gpt-shared",
            ]
        );
    }

    #[test]
    fn xai_catalog_uses_protocol_ids_before_display_names() {
        let value = json!({
            "data": [
                { "id": "display-id", "model": "grok-4.5" },
                { "modelId": "grok-build-0.1" },
                { "model_id": "grok-composer-2.5-fast" },
                { "name": "Grok Meta Display Name", "_meta": { "model": "grok-meta" } },
                { "name": "models/grok-name" },
                { "id": "grok-safe", "_meta": "not-an-object" },
                { "model": "grok-4.5" }
            ]
        });

        assert_eq!(
            extract_model_entries_from_catalog("xai", &value)
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![
                "grok-4.5",
                "grok-build-0.1",
                "grok-composer-2.5-fast",
                "grok-meta",
                "grok-name",
                "grok-safe",
            ]
        );
    }

    #[test]
    fn xai_catalog_probe_uses_cli_gateway_models_path() {
        assert_eq!(
            model_catalog_probe_paths("xai"),
            Some(("/v1/models", "/v1/models"))
        );
    }

    #[test]
    fn catalog_preserves_provider_display_names_and_model_id_fallback() {
        let entries = extract_model_entries_from_catalog(
            "gemini",
            &json!({
                "models": [
                    { "name": "models/gemini-3-pro-preview", "displayName": "Gemini 3 Pro Preview" },
                    { "name": "models/gemini-3-flash-preview" }
                ]
            }),
        );
        assert_eq!(entries[0].id, "gemini-3-pro-preview");
        assert_eq!(
            entries[0].display_name.as_deref(),
            Some("Gemini 3 Pro Preview")
        );
        assert_eq!(entries[1].id, "gemini-3-flash-preview");
        assert_eq!(entries[1].display_name, None);

        let body = build_model_catalog_response_body(&[("gemini".to_string(), entries)], false);
        let models = body["data"].as_array().expect("models");
        assert_eq!(models[0]["display_name"], "Gemini 3 Pro Preview");
        assert_eq!(models[1]["display_name"], "gemini-3-flash-preview");
    }

    #[test]
    fn xai_live_catalog_merges_with_builtin_fallback() {
        let mut models = builtin_model_ids("xai");
        let mut entries = model_catalog_entries_from_ids(&models);
        merge_model_catalog_entries(
            &mut entries,
            extract_model_entries_from_catalog(
                "xai",
                &json!({ "data": [{ "model": "grok-live" }] }),
            ),
        );
        models = entries.into_iter().map(|entry| entry.id).collect();

        assert!(models.contains(&"grok-4.5".to_string()));
        assert!(models.contains(&"grok-live".to_string()));
    }
}
