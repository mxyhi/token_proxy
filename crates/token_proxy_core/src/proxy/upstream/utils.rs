use axum::http::StatusCode;
use std::sync::atomic::Ordering;

use super::super::{config::UpstreamStrategy, ProxyState};
use crate::proxy::redact::redact_query_param_value;

pub(super) fn extract_query_param(path_with_query: &str, name: &str) -> Option<String> {
    let url = url::Url::parse(&format!("http://localhost{path_with_query}")).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

pub(super) fn ensure_query_param(url: &str, name: &str, value: &str) -> Result<String, String> {
    let mut parsed = url::Url::parse(url).map_err(|err| err.to_string())?;
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    {
        let mut writer = parsed.query_pairs_mut();
        writer.clear();
        for (key, existing) in pairs {
            if key == name {
                continue;
            }
            writer.append_pair(&key, &existing);
        }
        writer.append_pair(name, value);
    }

    Ok(parsed.to_string())
}

pub(super) fn sanitize_upstream_error(provider: &str, err: &reqwest::Error) -> String {
    let message = err.to_string();
    if provider == "gemini" {
        return redact_query_param_value(&message, super::GEMINI_API_KEY_QUERY);
    }
    message
}

pub(super) fn resolve_group_start(
    state: &ProxyState,
    provider: &str,
    group_index: usize,
    group_len: usize,
) -> usize {
    match state.config.upstream_strategy {
        UpstreamStrategy::PriorityFillFirst => 0,
        UpstreamStrategy::PriorityRoundRobin => state
            .cursors
            .get(provider)
            .and_then(|cursors| cursors.get(group_index))
            .map(|cursor| cursor.fetch_add(1, Ordering::Relaxed) % group_len)
            .unwrap_or(0),
    }
}

pub(super) fn is_retryable_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
}

pub(super) fn is_retryable_status(status: StatusCode) -> bool {
    // 为了尽量提供“无反馈”的自动切换体验，以下错误都允许继续尝试下一个渠道：
    // - 显式可回退的鉴权/路由/请求超时/语义校验错误：401/404/408/422
    // - 配额/权限/重定向：400/403/429/307
    // - 所有 5xx，包括 504 与 Cloudflare 524。
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::UNPROCESSABLE_ENTITY
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::TEMPORARY_REDIRECT
    ) || status.is_server_error()
}
