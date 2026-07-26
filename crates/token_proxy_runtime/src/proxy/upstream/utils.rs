use axum::http::StatusCode;
use std::error::Error as _;
use std::sync::atomic::Ordering;

use crate::proxy::redact::redact_query_param_value;
use crate::proxy::ProxyState;
use token_proxy_config::UpstreamOrderStrategy;

const RETRYABLE_TRANSPORT_ERROR_MARKERS: &[&str] = &[
    "authentication failed",
    "proxy authentication required",
    "connection refused",
    "no route to host",
    "network is unreachable",
    "no such host",
];
/// 共享连接池毒连接 / H2 协议层故障：应 rotate client 或降 HTTP/1.1，而不是死复用同一 session。
const STALE_CONNECTION_TRANSPORT_MARKERS: &[&str] = &[
    "unspecific protocol error",
    "http2 error",
    "stream error received",
    "connection closed before message completed",
    "connection closed",
    "connection reset",
    "connection reset by peer",
    "broken pipe",
    "unexpected end of file",
    "error sending request",
];

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
    match state.config.upstream_strategy.order {
        UpstreamOrderStrategy::FillFirst => 0,
        UpstreamOrderStrategy::RoundRobin => state
            .cursors
            .get(provider)
            .and_then(|cursors| cursors.get(group_index))
            .map(|cursor| cursor.fetch_add(1, Ordering::Relaxed) % group_len)
            .unwrap_or(0),
    }
}

pub(super) fn is_retryable_error(err: &reqwest::Error) -> bool {
    err.is_timeout()
        || err.is_connect()
        || is_stale_connection_transport_error(err)
        || is_retryable_transport_error_message(&err.to_string())
}

/// 首响应头前、适合 force-fresh / H1 降级的连接或 H2 协议故障。
pub(super) fn is_stale_connection_transport_error(err: &reqwest::Error) -> bool {
    if err.is_builder() || err.is_timeout() || err.is_connect() {
        return false;
    }
    // 部分 H2 故障只以 source chain 暴露；始终扫 chain，避免只靠顶层 Display。
    source_chain_contains_any(err, STALE_CONNECTION_TRANSPORT_MARKERS)
        || message_contains_any(&err.to_string(), STALE_CONNECTION_TRANSPORT_MARKERS)
}

pub(super) fn is_retryable_transport_error_message(message: &str) -> bool {
    // Some proxy failures, especially SOCKS5 auth rejection, only surface as text inside reqwest errors.
    message_contains_any(message, RETRYABLE_TRANSPORT_ERROR_MARKERS)
}

fn source_chain_contains_any(err: &reqwest::Error, markers: &[&str]) -> bool {
    let mut source = err.source();
    while let Some(cause) = source {
        if message_contains_any(&cause.to_string(), markers) {
            return true;
        }
        source = cause.source();
    }
    false
}

fn message_contains_any(message: &str, markers: &[&str]) -> bool {
    let message = message.to_ascii_lowercase();
    markers.iter().any(|marker| message.contains(marker))
}

pub(super) fn is_retryable_status(status: StatusCode) -> bool {
    // 为了尽量提供“无反馈”的自动切换体验，以下错误都允许继续尝试下一个渠道：
    // - 显式可回退的鉴权/路由/请求超时/语义校验错误：401/404/408/422
    // - 配额/权限/重定向：400/402/403/429/307
    // - 所有 5xx，包括 504 与 Cloudflare 524。
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::UNAUTHORIZED
            | StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::PAYLOAD_TOO_LARGE
            | StatusCode::UNPROCESSABLE_ENTITY
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::TEMPORARY_REDIRECT
    ) || status.is_server_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_markers_cover_h2_protocol_and_connection_closed() {
        assert!(message_contains_any(
            "stream error received: unspecific protocol error detected",
            STALE_CONNECTION_TRANSPORT_MARKERS
        ));
        assert!(message_contains_any(
            "error sending request for url (https://example.com)",
            STALE_CONNECTION_TRANSPORT_MARKERS
        ));
        assert!(message_contains_any(
            "connection closed before message completed",
            STALE_CONNECTION_TRANSPORT_MARKERS
        ));
        assert!(!message_contains_any(
            "certificate verify failed",
            STALE_CONNECTION_TRANSPORT_MARKERS
        ));
    }

    #[test]
    fn retryable_proxy_markers_still_match() {
        assert!(is_retryable_transport_error_message(
            "SOCKS5 authentication failed"
        ));
        assert!(is_retryable_transport_error_message("connection refused"));
        assert!(!is_retryable_transport_error_message(
            "unspecific protocol error"
        ));
    }

    #[test]
    fn payment_required_is_retryable_for_account_failover() {
        assert!(is_retryable_status(StatusCode::PAYMENT_REQUIRED));
    }
}
