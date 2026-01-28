use std::collections::{HashMap, HashSet};

use super::{
    model_mapping::compile_model_mappings, HeaderOverride, InboundApiFormat, ProviderUpstreams,
    UpstreamConfig, UpstreamGroup, UpstreamOverrides, UpstreamRuntime,
};
use super::types::InboundApiFormatMask;
use axum::http::header::{HeaderName, HeaderValue};

const APP_PROXY_URL_PLACEHOLDER: &str = "$app_proxy_url";
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_ANTIGRAVITY_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";

#[derive(Clone)]
pub(super) struct NormalizedUpstream {
    pub(super) provider: String,
    pub(super) runtime: UpstreamRuntime,
}

pub(super) fn normalize_upstreams(
    upstreams: &[UpstreamConfig],
    app_proxy_url: Option<&str>,
) -> Result<Vec<NormalizedUpstream>, String> {
    validate_upstream_ids(upstreams)?;
    let mut normalized = Vec::with_capacity(upstreams.len());
    for upstream in upstreams {
        normalized.extend(normalize_single_upstream(upstream, app_proxy_url)?);
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

fn group_upstreams_by_priority(mut upstreams: Vec<UpstreamRuntime>) -> Vec<UpstreamGroup> {
    upstreams.sort_by(|left, right| right.priority.cmp(&left.priority));
    let mut groups: Vec<UpstreamGroup> = Vec::new();
    for upstream in upstreams {
        match groups.last_mut() {
            Some(group) if group.priority == upstream.priority => group.items.push(upstream),
            _ => groups.push(UpstreamGroup {
                priority: upstream.priority,
                items: vec![upstream],
            }),
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

fn normalize_single_upstream(
    upstream: &UpstreamConfig,
    app_proxy_url: Option<&str>,
) -> Result<Vec<NormalizedUpstream>, String> {
    if !upstream.enabled {
        return Ok(Vec::new());
    }

    let providers = normalize_providers(upstream)?;
    validate_convert_from_map(upstream, &providers)?;

    let api_key = upstream
        .api_key
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let kiro_account_id = upstream
        .kiro_account_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let codex_account_id = upstream
        .codex_account_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let antigravity_account_id = upstream
        .antigravity_account_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let proxy_url = normalize_upstream_proxy_url(
        upstream.proxy_url.as_deref(),
        app_proxy_url,
        &upstream.id,
    )?;
    let model_mappings = compile_model_mappings(&upstream.id, &upstream.model_mappings)?;
    let header_overrides = normalize_header_overrides(upstream.overrides.as_ref())?;

    let mut output = Vec::with_capacity(providers.len());
    for provider in providers {
        let base_url = resolve_base_url(&upstream.id, upstream.base_url.as_str(), &provider)?;
        validate_provider_account_binding(
            &upstream.id,
            &provider,
            kiro_account_id.as_deref(),
            codex_account_id.as_deref(),
            antigravity_account_id.as_deref(),
        )?;

        let mut allowed_inbound_formats = native_inbound_formats_for_provider(&provider);
        if let Some(extra) = upstream.convert_from_map.get(provider.as_str()) {
            allowed_inbound_formats.extend(extra.iter().copied());
        }

        let runtime = UpstreamRuntime {
            id: upstream.id.trim().to_string(),
            base_url,
            api_key: api_key.clone(),
            filter_prompt_cache_retention: upstream.filter_prompt_cache_retention,
            filter_safety_identifier: upstream.filter_safety_identifier,
            kiro_account_id: kiro_account_id.clone(),
            codex_account_id: codex_account_id.clone(),
            antigravity_account_id: antigravity_account_id.clone(),
            kiro_preferred_endpoint: upstream.preferred_endpoint.clone(),
            proxy_url: proxy_url.clone(),
            priority: upstream.priority.unwrap_or(0),
            model_mappings: model_mappings.clone(),
            header_overrides: header_overrides.clone(),
            allowed_inbound_formats,
        };
        output.push(NormalizedUpstream { provider, runtime });
    }

    Ok(output)
}

fn normalize_providers(upstream: &UpstreamConfig) -> Result<Vec<String>, String> {
    if upstream.providers.is_empty() {
        return Err(format!("Upstream {} providers cannot be empty.", upstream.id));
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
        .filter(|provider| matches!(provider.as_str(), "kiro" | "codex" | "antigravity"))
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

fn validate_convert_from_map(upstream: &UpstreamConfig, providers: &[String]) -> Result<(), String> {
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

fn resolve_base_url(
    upstream_id: &str,
    base_url: &str,
    provider: &str,
) -> Result<String, String> {
    let base_url = base_url.trim();
    if !base_url.is_empty() {
        return Ok(base_url.to_string());
    }

    if provider == "codex" {
        return Ok(DEFAULT_CODEX_BASE_URL.to_string());
    }
    if provider == "antigravity" {
        return Ok(DEFAULT_ANTIGRAVITY_BASE_URL.to_string());
    }
    if provider == "kiro" {
        return Ok(String::new());
    }

    Err(format!("Upstream {upstream_id} base_url cannot be empty."))
}

fn validate_provider_account_binding(
    upstream_id: &str,
    provider: &str,
    kiro_account_id: Option<&str>,
    codex_account_id: Option<&str>,
    antigravity_account_id: Option<&str>,
) -> Result<(), String> {
    if provider == "kiro" && kiro_account_id.is_none() {
        return Err(format!(
            "Upstream {upstream_id} requires a Kiro account binding."
        ));
    }
    if provider == "codex" && codex_account_id.is_none() {
        return Err(format!(
            "Upstream {upstream_id} requires a Codex account binding."
        ));
    }
    if provider == "antigravity" && antigravity_account_id.is_none() {
        return Err(format!(
            "Upstream {upstream_id} requires an Antigravity account binding."
        ));
    }
    Ok(())
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
        // Codex 的“native”更接近 OpenAI Responses；Chat 通常需要显式允许转换。
        "codex" => mask.insert(InboundApiFormat::OpenaiResponses),
        // Antigravity 原生处理 Gemini 路径；其它格式需显式允许转换后再走 Gemini 兼容层。
        "antigravity" => mask.insert(InboundApiFormat::Gemini),
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
                    Some(HeaderValue::from_str("").map_err(|_| {
                        format!("Invalid header value for {raw_name}")
                    })?)
                } else {
                    Some(HeaderValue::from_str(value).map_err(|_| {
                        format!("Invalid header value for {raw_name}")
                    })?)
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
        return Ok(Some(validate_proxy_url(app_proxy_url, upstream_id)?.to_string()));
    }
    Ok(Some(validate_proxy_url(value, upstream_id)?.to_string()))
}

fn validate_proxy_url<'a>(value: &'a str, upstream_id: &str) -> Result<&'a str, String> {
    let parsed = url::Url::parse(value).map_err(|_| {
        format!("Upstream {upstream_id} proxy_url is not a valid URL.")
    })?;
    match parsed.scheme() {
        "http" | "https" | "socks5" | "socks5h" => Ok(value),
        scheme => Err(format!(
            "Upstream {upstream_id} proxy_url scheme is not supported: {scheme}."
        )),
    }
}
