use axum::http::StatusCode;
use std::sync::atomic::Ordering;

use super::super::{config::UpstreamStrategy, ProxyState};

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

pub(super) fn redact_query_param_value(message: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let mut output = String::with_capacity(message.len());
    let mut rest = message;

    while let Some(pos) = rest.find(&needle) {
        let (before, after) = rest.split_at(pos);
        output.push_str(before);
        output.push_str(&needle);
        output.push_str("***");

        let after = &after[needle.len()..];
        let mut end = after.len();
        for (idx, ch) in after.char_indices() {
            if matches!(ch, '&' | ')' | ' ' | '\n' | '\r' | '\t' | '"' | '\'') {
                end = idx;
                break;
            }
        }
        rest = &after[end..];
    }

    output.push_str(rest);
    output
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

pub(super) fn build_group_order(group_len: usize, start: usize) -> Vec<usize> {
    (0..group_len)
        .map(|offset| (start + offset) % group_len)
        .collect()
}

pub(super) fn is_retryable_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
}

pub(super) fn is_retryable_status(status: StatusCode) -> bool {
    // 对齐 new-api 的重试策略：429/307/5xx（排除 504/524）；额外允许 403 触发 fallback。
    if status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::TEMPORARY_REDIRECT
    {
        return true;
    }
    if status == StatusCode::GATEWAY_TIMEOUT {
        return false;
    }
    if status.as_u16() == 524 {
        // Cloudflare timeout.
        return false;
    }
    status.is_server_error()
}
