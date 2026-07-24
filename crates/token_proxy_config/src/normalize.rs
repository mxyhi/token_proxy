use std::collections::{HashMap, HashSet};

use super::types::InboundApiFormatMask;
use super::{
    hot_model_mappings::merge_hot_model_mappings, model_mapping::compile_model_mappings,
    AccountProvider, HeaderOverride, InboundApiFormat, ProviderUpstreams, StaticApiKeyHeaders,
    UpstreamConfig, UpstreamCredential, UpstreamGroup, UpstreamOverrides, UpstreamRuntime,
};
use axum::http::header::{HeaderName, HeaderValue};

const APP_PROXY_URL_PLACEHOLDER: &str = "$app_proxy_url";
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
// 配置校验不依赖 provider 运行时；测试会锁定该可信端点与 xAI crate 一致。
const XAI_CLI_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";

#[derive(Clone)]
pub(super) struct NormalizedUpstream {
    pub(super) provider: String,
    pub(super) runtime: UpstreamRuntime,
}

pub(super) fn normalize_upstreams(
    upstreams: &[UpstreamConfig],
    app_proxy_url: Option<&str>,
    hot_model_mappings: &HashMap<String, String>,
) -> Result<Vec<NormalizedUpstream>, String> {
    validate_upstream_ids(upstreams)?;
    // Account-as-Upstream：同一账户引用只能被一条 Upstream 绑定。
    validate_unique_account_bindings(upstreams)?;
    let mut normalized = Vec::with_capacity(upstreams.len());
    for upstream in upstreams {
        normalized.extend(normalize_single_upstream(
            upstream,
            app_proxy_url,
            hot_model_mappings,
        )?);
    }
    Ok(normalized)
}

pub(super) fn build_provider_upstreams(
    upstreams: Vec<NormalizedUpstream>,
) -> Result<HashMap<String, ProviderUpstreams>, String> {
    let mut grouped: HashMap<String, Vec<UpstreamRuntime>> = HashMap::new();
    for upstream in upstreams {
        grouped
            .entry(upstream.provider)
            .or_default()
            .push(upstream.runtime);
    }
    let mut output = HashMap::new();
    for (provider, upstreams) in grouped {
        let groups = group_upstreams_by_priority(upstreams);
        output.insert(provider, ProviderUpstreams { groups });
    }
    Ok(output)
}

fn group_upstreams_by_priority(upstreams: Vec<UpstreamRuntime>) -> Vec<UpstreamGroup> {
    // Keep same-priority order stable by preserving config insertion order.
    let mut grouped: HashMap<i32, Vec<UpstreamRuntime>> = HashMap::new();
    for upstream in upstreams {
        grouped.entry(upstream.priority).or_default().push(upstream);
    }
    let mut priorities: Vec<i32> = grouped.keys().copied().collect();
    priorities.sort_by(|left, right| right.cmp(left));
    let mut groups = Vec::with_capacity(priorities.len());
    for priority in priorities {
        if let Some(items) = grouped.remove(&priority) {
            groups.push(UpstreamGroup { priority, items });
        }
    }
    groups
}

fn validate_upstream_ids(upstreams: &[UpstreamConfig]) -> Result<(), String> {
    let mut seen_ids = HashSet::new();
    for upstream in upstreams {
        let id = upstream.id.trim();
        if id.is_empty() {
            return Err("Upstream id cannot be empty.".to_string());
        }
        if !seen_ids.insert(id.to_string()) {
            return Err(format!("Upstream id already exists: {id}."));
        }
    }
    Ok(())
}

/// 同一 (provider, account_id) 不得被两条 Upstream 绑定；错误不含 credential secret。
fn validate_unique_account_bindings(upstreams: &[UpstreamConfig]) -> Result<(), String> {
    let mut seen: HashMap<(AccountProvider, String), String> = HashMap::new();
    for upstream in upstreams {
        let Some((provider, account_id)) = upstream.credential.account_binding() else {
            continue;
        };
        let account_id = account_id.trim();
        // 空 account_id 由 project_credential 硬失败；此处只查重复绑定。
        if account_id.is_empty() {
            continue;
        }
        let key = (provider, account_id.to_string());
        if let Some(existing_id) = seen.get(&key) {
            return Err(format!(
                "Account binding {}:{} is bound by multiple upstreams: {} and {}.",
                provider.as_str(),
                account_id,
                existing_id,
                upstream.id.trim()
            ));
        }
        seen.insert(key, upstream.id.trim().to_string());
    }
    Ok(())
}

fn normalize_single_upstream(
    upstream: &UpstreamConfig,
    app_proxy_url: Option<&str>,
    hot_model_mappings: &HashMap<String, String>,
) -> Result<Vec<NormalizedUpstream>, String> {
    if !upstream.enabled {
        return Ok(Vec::new());
    }

    let providers = normalize_providers(upstream)?;
    validate_convert_from_map(upstream, &providers)?;
    let runtime_providers = resolve_runtime_providers(
        &upstream.id,
        &providers,
        upstream.use_chat_completions_for_responses,
    )?;
    // 将 credential 投影为 runtime 仍消费的 api_key / 三个 account_id 字段。
    let projected = project_credential(&upstream.id, &upstream.credential, &providers)?;
    let api_keys = projected.api_keys;
    let kiro_account_id = projected.kiro_account_id;
    let codex_account_id = projected.codex_account_id;
    let xai_account_id = projected.xai_account_id;
    let proxy_url =
        normalize_upstream_proxy_url(upstream.proxy_url.as_deref(), app_proxy_url, &upstream.id)?;
    let available_models = normalize_available_models(&upstream.available_models);
    let advertised_model_ids = if available_models.is_empty() {
        collect_advertised_model_ids(&upstream.model_mappings)
    } else {
        available_models.clone()
    };
    tracing::debug!(
        upstream_id = %upstream.id,
        available_model_count = available_models.len(),
        unrestricted = available_models.is_empty(),
        credential_type = projected.credential_type,
        "normalized upstream model availability"
    );
    let merged_model_mappings =
        merge_hot_model_mappings(hot_model_mappings, &upstream.model_mappings);
    let model_mappings = compile_model_mappings(&upstream.id, &merged_model_mappings)?;
    let header_overrides = normalize_header_overrides(upstream.overrides.as_ref())?;

    let mut output = Vec::with_capacity(runtime_providers.len() * api_keys.len().max(1));
    for (provider, runtime_provider) in runtime_providers {
        let base_url = resolve_base_url(
            &upstream.id,
            upstream.base_url.as_str(),
            runtime_provider.as_str(),
        )?;

        let mut allowed_inbound_formats =
            default_inbound_formats_for_provider(provider.as_str(), runtime_provider.as_str());
        if let Some(extra) = upstream.convert_from_map.get(provider.as_str()) {
            allowed_inbound_formats.extend(extra.iter().copied());
        }

        for (index, api_key) in api_keys.iter().enumerate() {
            let api_key_headers = api_key
                .as_deref()
                .map(|key| StaticApiKeyHeaders::new(&upstream.id, key))
                .transpose()?;
            let runtime = UpstreamRuntime {
                id: upstream.id.trim().to_string(),
                selector_key: build_selector_key(&upstream.id, api_keys.len(), index),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                api_key_headers,
                filter_prompt_cache_retention: upstream.filter_prompt_cache_retention,
                filter_safety_identifier: upstream.filter_safety_identifier,
                rewrite_developer_role_to_system: upstream.rewrite_developer_role_to_system,
                kiro_account_id: kiro_account_id.clone(),
                codex_account_id: codex_account_id.clone(),
                xai_account_id: xai_account_id.clone(),
                kiro_preferred_endpoint: upstream.preferred_endpoint.clone(),
                // Upstream 级 proxy/priority 属于新模型，不能从 account 字段迁移误删。
                proxy_url: proxy_url.clone(),
                priority: upstream.priority.unwrap_or(0),
                available_models: available_models.clone(),
                advertised_model_ids: advertised_model_ids.clone(),
                model_mappings: model_mappings.clone(),
                header_overrides: header_overrides.clone(),
                allowed_inbound_formats,
            };
            output.push(NormalizedUpstream {
                provider: runtime_provider.clone(),
                runtime,
            });
        }
    }

    Ok(output)
}

/// credential 投影到 runtime 字段；不暴露 key / account_id 到日志。
struct CredentialProjection {
    api_keys: Vec<Option<String>>,
    kiro_account_id: Option<String>,
    codex_account_id: Option<String>,
    xai_account_id: Option<String>,
    credential_type: &'static str,
}

/// 从判别联合生成 runtime 投影，并校验账户型约束。
/// kiro/codex/xai 必须使用 Account；Passthrough 仅允许非账户型 provider。
fn project_credential(
    upstream_id: &str,
    credential: &UpstreamCredential,
    providers: &[String],
) -> Result<CredentialProjection, String> {
    match credential {
        UpstreamCredential::ApiKeys { api_keys } => {
            validate_api_keys_not_for_account_providers(upstream_id, providers)?;
            let normalized = normalize_api_key_list(api_keys);
            Ok(CredentialProjection {
                api_keys: normalized,
                kiro_account_id: None,
                codex_account_id: None,
                xai_account_id: None,
                credential_type: "api_keys",
            })
        }
        UpstreamCredential::Account {
            provider,
            account_id,
        } => {
            let account_id = account_id.trim();
            if account_id.is_empty() {
                return Err(format!(
                    "Upstream {upstream_id} account credential account_id cannot be empty."
                ));
            }
            validate_account_credential_providers(upstream_id, *provider, providers)?;
            let (kiro_account_id, codex_account_id, xai_account_id) = match provider {
                AccountProvider::Kiro => (Some(account_id.to_string()), None, None),
                AccountProvider::Codex => (None, Some(account_id.to_string()), None),
                AccountProvider::Xai => (None, None, Some(account_id.to_string())),
            };
            Ok(CredentialProjection {
                api_keys: vec![None],
                kiro_account_id,
                codex_account_id,
                xai_account_id,
                credential_type: "account",
            })
        }
        UpstreamCredential::Passthrough => {
            // Account-as-Upstream：账户型上游禁止 passthrough，必须显式 Account。
            if providers_include_account_type(providers) {
                return Err(format!(
                    "Upstream {upstream_id} account-based providers require account credential; passthrough is not allowed."
                ));
            }
            Ok(CredentialProjection {
                api_keys: vec![None],
                kiro_account_id: None,
                codex_account_id: None,
                xai_account_id: None,
                credential_type: "passthrough",
            })
        }
    }
}

fn providers_include_account_type(providers: &[String]) -> bool {
    providers
        .iter()
        .any(|provider| AccountProvider::from_provider_str(provider).is_some())
}

/// api_keys 不得用于 kiro/codex/xai；xai 保留既有错误文案。
fn validate_api_keys_not_for_account_providers(
    upstream_id: &str,
    providers: &[String],
) -> Result<(), String> {
    if providers.iter().any(|provider| provider == "xai") {
        return Err(format!(
            "Upstream {upstream_id} xAI OAuth provider does not accept api_keys."
        ));
    }
    if providers
        .iter()
        .any(|provider| matches!(provider.as_str(), "kiro" | "codex"))
    {
        return Err(format!(
            "Upstream {upstream_id} does not accept api_keys for account-based providers."
        ));
    }
    Ok(())
}

/// account credential：providers 中恰好一个匹配的账户型 provider。
fn validate_account_credential_providers(
    upstream_id: &str,
    bound: AccountProvider,
    providers: &[String],
) -> Result<(), String> {
    let account_providers = providers
        .iter()
        .filter_map(|provider| AccountProvider::from_provider_str(provider))
        .collect::<Vec<_>>();
    // 无账户型 / 绑定 provider 不一致：沿用旧平铺字段错误文案。
    if account_providers.is_empty() || account_providers.iter().all(|actual| *actual != bound) {
        return account_provider_mismatch_error(upstream_id, bound);
    }
    if account_providers.len() != 1 {
        return Err(format!(
            "Upstream {upstream_id} account credential requires exactly one account-based provider in providers[]."
        ));
    }
    Ok(())
}

fn account_provider_mismatch_error(
    upstream_id: &str,
    bound: AccountProvider,
) -> Result<(), String> {
    match bound {
        AccountProvider::Xai => Err(format!(
            "Upstream {upstream_id} xai_account_id requires provider xai."
        )),
        AccountProvider::Kiro => Err(format!(
            "Upstream {upstream_id} kiro_account_id requires provider kiro."
        )),
        AccountProvider::Codex => Err(format!(
            "Upstream {upstream_id} codex_account_id requires provider codex."
        )),
    }
}

fn normalize_available_models(models: &[String]) -> Vec<String> {
    let mut normalized = models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn collect_advertised_model_ids(model_mappings: &HashMap<String, String>) -> Vec<String> {
    let mut ids = model_mappings
        .keys()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty() && !key.contains('*'))
        .map(str::to_string)
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

fn resolve_runtime_providers(
    upstream_id: &str,
    providers: &[String],
    use_chat_completions_for_responses: bool,
) -> Result<Vec<(String, String)>, String> {
    let mut output = Vec::with_capacity(providers.len());
    let mut seen_runtime_providers = HashSet::new();
    for provider in providers {
        let runtime_provider =
            runtime_provider_for_upstream(provider.as_str(), use_chat_completions_for_responses);
        if !seen_runtime_providers.insert(runtime_provider.to_string()) {
            return Err(format!(
                "Upstream {upstream_id} providers collapse to duplicate runtime provider: {runtime_provider}."
            ));
        }
        output.push((provider.clone(), runtime_provider.to_string()));
    }
    Ok(output)
}

fn normalize_api_key_list(api_keys: &[String]) -> Vec<Option<String>> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in api_keys {
        let trimmed = value.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        output.push(Some(trimmed.to_string()));
    }
    if output.is_empty() {
        output.push(None);
    }
    output
}

fn build_selector_key(upstream_id: &str, api_key_count: usize, index: usize) -> String {
    let id = upstream_id.trim();
    if api_key_count <= 1 {
        return id.to_string();
    }
    format!("{id}#{}", index + 1)
}

fn runtime_provider_for_upstream(provider: &str, use_chat_completions_for_responses: bool) -> &str {
    if provider == "openai-response" && use_chat_completions_for_responses {
        return "openai";
    }
    provider
}

fn is_supported_provider(provider: &str) -> bool {
    matches!(
        provider,
        "openai" | "openai-response" | "anthropic" | "gemini" | "kiro" | "codex" | "xai"
    )
}

fn normalize_providers(upstream: &UpstreamConfig) -> Result<Vec<String>, String> {
    if upstream.providers.is_empty() {
        return Err(format!(
            "Upstream {} providers cannot be empty.",
            upstream.id
        ));
    }

    let mut providers = Vec::with_capacity(upstream.providers.len());
    let mut seen = HashSet::new();
    for provider in &upstream.providers {
        let trimmed = provider.trim();
        if trimmed.is_empty() {
            return Err(format!(
                "Upstream {} providers cannot include empty values.",
                upstream.id
            ));
        }
        let normalized = trimmed.to_string();
        if !is_supported_provider(&normalized) {
            return Err(format!(
                "Upstream {} provider {} is not supported.",
                upstream.id, normalized
            ));
        }
        if !seen.insert(normalized.clone()) {
            return Err(format!(
                "Upstream {} providers contains duplicate: {trimmed}.",
                upstream.id
            ));
        }
        providers.push(normalized);
    }

    validate_provider_mix(&upstream.id, &providers)?;
    Ok(providers)
}

fn validate_provider_mix(upstream_id: &str, providers: &[String]) -> Result<(), String> {
    let specials = providers
        .iter()
        .filter(|provider| matches!(provider.as_str(), "kiro" | "codex" | "xai"))
        .collect::<Vec<_>>();
    if specials.is_empty() {
        return Ok(());
    }
    if providers.len() > 1 {
        return Err(format!(
            "Upstream {upstream_id} providers cannot mix {} with other providers.",
            specials
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

fn validate_convert_from_map(
    upstream: &UpstreamConfig,
    providers: &[String],
) -> Result<(), String> {
    if upstream.convert_from_map.is_empty() {
        return Ok(());
    }
    let provider_set: HashSet<&str> = providers.iter().map(|value| value.as_str()).collect();
    for provider in upstream.convert_from_map.keys() {
        let trimmed = provider.trim();
        if trimmed.is_empty() {
            return Err(format!(
                "Upstream {} convert_from_map cannot include empty provider keys.",
                upstream.id
            ));
        }
        if !provider_set.contains(trimmed) {
            return Err(format!(
                "Upstream {} convert_from_map provider is not in providers[]: {provider}.",
                upstream.id
            ));
        }
    }
    Ok(())
}

fn resolve_base_url(upstream_id: &str, base_url: &str, provider: &str) -> Result<String, String> {
    let base_url = base_url.trim();
    if provider == "xai" {
        if base_url.is_empty()
            || base_url.trim_end_matches('/') == XAI_CLI_BASE_URL.trim_end_matches('/')
        {
            return Ok(XAI_CLI_BASE_URL.to_string());
        }
        return Err(format!(
            "Upstream {upstream_id} xAI OAuth base_url must be {}.",
            XAI_CLI_BASE_URL
        ));
    }
    if !base_url.is_empty() {
        return Ok(base_url.to_string());
    }

    if provider == "codex" {
        return Ok(DEFAULT_CODEX_BASE_URL.to_string());
    }
    if provider == "kiro" {
        return Ok(String::new());
    }

    Err(format!("Upstream {upstream_id} base_url cannot be empty."))
}

fn default_inbound_formats_for_provider(
    configured_provider: &str,
    runtime_provider: &str,
) -> InboundApiFormatMask {
    if configured_provider == "openai-response" && runtime_provider == "openai" {
        let mut mask = InboundApiFormatMask::default();
        mask.insert(InboundApiFormat::OpenaiResponses);
        return mask;
    }
    native_inbound_formats_for_provider(runtime_provider)
}

fn native_inbound_formats_for_provider(provider: &str) -> InboundApiFormatMask {
    let mut mask = InboundApiFormatMask::default();
    match provider {
        "openai" => mask.insert(InboundApiFormat::OpenaiChat),
        "openai-response" => mask.insert(InboundApiFormat::OpenaiResponses),
        "anthropic" => mask.insert(InboundApiFormat::AnthropicMessages),
        "gemini" => mask.insert(InboundApiFormat::Gemini),
        // Kiro 仅作为 Anthropic `/v1/messages` 的同协议 provider；
        // OpenAI endpoints（/v1/chat/completions、/v1/responses）若要走 Kiro，需要显式通过
        // `convert_from_map.kiro` 授权（避免“意外命中 Kiro”）。
        "kiro" => mask.insert(InboundApiFormat::AnthropicMessages),
        "codex" => {
            mask.insert(InboundApiFormat::OpenaiChat);
            mask.insert(InboundApiFormat::OpenaiResponses);
        }
        "xai" => {
            mask.insert(InboundApiFormat::OpenaiChat);
            mask.insert(InboundApiFormat::OpenaiResponses);
            mask.insert(InboundApiFormat::AnthropicMessages);
            mask.insert(InboundApiFormat::Gemini);
        }
        _ => {}
    }
    mask
}

fn normalize_header_overrides(
    overrides: Option<&UpstreamOverrides>,
) -> Result<Option<Vec<HeaderOverride>>, String> {
    let Some(overrides) = overrides else {
        return Ok(None);
    };
    if overrides.header.is_empty() {
        return Ok(None);
    }

    let mut normalized = Vec::with_capacity(overrides.header.len());
    for (raw_name, raw_value) in &overrides.header {
        let trimmed = raw_name.trim();
        let name = HeaderName::from_bytes(trimmed.as_bytes())
            .map_err(|_| format!("Invalid header name in overrides: {raw_name}"))?;

        let value: Option<HeaderValue> = match raw_value {
            Some(value) => {
                if value.is_empty() {
                    // 允许空字符串，代表设置为空值。
                    Some(
                        HeaderValue::from_str("")
                            .map_err(|_| format!("Invalid header value for {raw_name}"))?,
                    )
                } else {
                    Some(
                        HeaderValue::from_str(value)
                            .map_err(|_| format!("Invalid header value for {raw_name}"))?,
                    )
                }
            }
            None => None,
        };

        normalized.push(HeaderOverride { name, value });
    }

    // 用户输入大小写混合时，保持用户写法；应用阶段再做覆盖策略。
    Ok(Some(normalized))
}

fn normalize_upstream_proxy_url(
    proxy_url: Option<&str>,
    app_proxy_url: Option<&str>,
    upstream_id: &str,
) -> Result<Option<String>, String> {
    // upstream 级 proxy：空=直连；`APP_PROXY` 占位解析为 app_proxy_url；否则校验自定义 URL。
    let value = proxy_url.unwrap_or_default().trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value == APP_PROXY_URL_PLACEHOLDER {
        let app_proxy_url = app_proxy_url.unwrap_or_default().trim();
        if app_proxy_url.is_empty() {
            return Err(format!(
                "Upstream {upstream_id} proxy_url is set to {APP_PROXY_URL_PLACEHOLDER}, but app_proxy_url is empty."
            ));
        }
        return Ok(Some(
            validate_proxy_url(app_proxy_url, upstream_id)?.to_string(),
        ));
    }
    Ok(Some(validate_proxy_url(value, upstream_id)?.to_string()))
}

fn validate_proxy_url<'a>(value: &'a str, upstream_id: &str) -> Result<&'a str, String> {
    let parsed = url::Url::parse(value)
        .map_err(|_| format!("Upstream {upstream_id} proxy_url is not a valid URL."))?;
    match parsed.scheme() {
        "http" | "https" | "socks5" | "socks5h" => Ok(value),
        scheme => Err(format!(
            "Upstream {upstream_id} proxy_url scheme is not supported: {scheme}."
        )),
    }
}
