//! 出站 URL 纯拼接：`基础地址 + prefix + 版本段替换后的客户端路径`。
//!
//! 所见即所得——本模块不对拼接结果做任何段的删除、去重或补全：
//! - 家族由 provider 决定；未配置家族 = 恒等回退（prefix 空、版本段 `/v1`）。
//! - 客户端路径第一段为 `/v1` 时替换为版本段（suffix 第一段，不要求 vX 格式）；
//!   其余路径（`/v1beta/...`、`/alpha/search` 等）原样拼接。
//! - query 原样保留。

use super::config::EndpointCompose;
use super::config::DEFAULT_VERSION_SEGMENT;
use crate::types::UpstreamRuntime;

const INBOUND_VERSION_SEGMENT: &str = "/v1";

/// 入口：按 provider 家族取 `url_compose` 配置并纯拼接出站 URL。
pub(crate) fn compose_outbound_url(
    upstream: &UpstreamRuntime,
    provider: &str,
    path_with_query: &str,
) -> String {
    let compose = family_compose(upstream, provider);
    compose_from_parts(&upstream.base_url, compose, path_with_query)
}

/// 家族解析：不能用路径反推（provider=openai 收到转换后的 `/v1/chat/completions`
/// 时必须用 `url_compose.openai`）；未参与组合的 provider 一律恒等回退。
fn family_compose<'a>(
    upstream: &'a UpstreamRuntime,
    provider: &str,
) -> Option<&'a EndpointCompose> {
    let compose = &upstream.url_compose;
    match provider {
        "openai" => compose.openai.as_ref(),
        "openai-response" | "xai" => compose.openai_response.as_ref(),
        "anthropic" => compose.anthropic.as_ref(),
        // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (compose) START ══════════
        "dashscope" => compose.dashscope.as_ref(),
        // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (compose) END ══════════
        _ => None,
    }
}

fn compose_from_parts(
    base_url: &str,
    compose: Option<&EndpointCompose>,
    path_with_query: &str,
) -> String {
    let (path, query) = match path_with_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_with_query, None),
    };
    let base = base_url.trim().trim_end_matches('/');
    let prefix = compose.map(|value| value.prefix.as_str()).unwrap_or("");
    let version = compose
        .map(|value| version_segment(&value.suffix))
        .unwrap_or_else(|| DEFAULT_VERSION_SEGMENT.to_string());
    let mut url = format!("{base}{prefix}{}", replace_version_segment(path, &version));
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }
    url
}

/// suffix 第一段即"版本段"（`/v1`、`/v3`、`/abc` 皆可）；空 suffix 回退 `/v1`。
fn version_segment(suffix: &str) -> String {
    let normalized = super::config::normalize_segment(suffix);
    if normalized.is_empty() {
        return DEFAULT_VERSION_SEGMENT.to_string();
    }
    let segment = normalized
        .split('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| format!("/{segment}"));
    segment.unwrap_or_else(|| DEFAULT_VERSION_SEGMENT.to_string())
}

/// 仅当客户端路径第一段恰为 `/v1` 时替换为版本段（`/v10/...` 不受影响）。
fn replace_version_segment(path: &str, version: &str) -> String {
    let matched = path.starts_with(INBOUND_VERSION_SEGMENT)
        && (path.len() == INBOUND_VERSION_SEGMENT.len()
            || path.as_bytes()[INBOUND_VERSION_SEGMENT.len()] == b'/');
    if matched {
        format!("{version}{}", &path[INBOUND_VERSION_SEGMENT.len()..])
    } else {
        path.to_string()
    }
}

#[cfg(test)]
#[path = "compose.test.rs"]
mod tests;
