use std::collections::{HashMap, HashSet};

use super::{
    model_mapping::compile_model_mappings, HeaderOverride, ProviderUpstreams, UpstreamConfig,
    UpstreamGroup, UpstreamOverrides, UpstreamRuntime,
};
use axum::http::header::{HeaderName, HeaderValue};

const APP_PROXY_URL_PLACEHOLDER: &str = "$app_proxy_url";

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
        if let Some(entry) = normalize_single_upstream(upstream, app_proxy_url)? {
            normalized.push(entry);
        }
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
) -> Result<Option<NormalizedUpstream>, String> {
    if !upstream.enabled {
        return Ok(None);
    }
    let provider = upstream.provider.trim();
    if provider.is_empty() {
        return Err(format!(
            "Upstream {} provider cannot be empty.",
            upstream.id
        ));
    }
    let base_url = upstream.base_url.trim();
    if base_url.is_empty() && provider != "kiro" {
        return Err(format!(
            "Upstream {} base_url cannot be empty.",
            upstream.id
        ));
    }
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
    if provider == "kiro" && kiro_account_id.is_none() {
        return Err(format!(
            "Upstream {} requires a Kiro account binding.",
            upstream.id
        ));
    }
    let proxy_url = normalize_upstream_proxy_url(
        upstream.proxy_url.as_deref(),
        app_proxy_url,
        &upstream.id,
    )?;
    let model_mappings = compile_model_mappings(&upstream.id, &upstream.model_mappings)?;
    let header_overrides = normalize_header_overrides(upstream.overrides.as_ref())?;
    let runtime = UpstreamRuntime {
        id: upstream.id.trim().to_string(),
        base_url: base_url.to_string(),
        api_key,
        kiro_account_id,
        kiro_preferred_endpoint: upstream.preferred_endpoint.clone(),
        proxy_url,
        priority: upstream.priority.unwrap_or(0),
        model_mappings,
        header_overrides,
    };
    Ok(Some(NormalizedUpstream {
        provider: provider.to_string(),
        runtime,
    }))
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
