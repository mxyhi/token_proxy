use axum::http::HeaderMap;
use std::sync::atomic::{AtomicU64, Ordering};

use token_proxy_config::{InboundApiFormat, ProxyConfig};

const CODEX_PROVIDER: &str = "codex";
const THREAD_ID_HEADER: &str = "thread-id";
const SESSION_ID_HEADER: &str = "session-id";
const LEGACY_SESSION_ID_HEADER: &str = "session_id";

static NEXT_REQUEST_SCOPE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum CooldownScope {
    Global,
    CodexSession(String),
    Request(u64),
}

impl CooldownScope {
    pub(crate) fn codex_responses_request(
        config: &ProxyConfig,
        inbound_format: Option<InboundApiFormat>,
        headers: &HeaderMap,
    ) -> Self {
        if !config.codex_session_scoped_cooldown_enabled
            || inbound_format != Some(InboundApiFormat::OpenaiResponses)
        {
            return Self::Global;
        }

        // Current Codex sends `thread-id`/`session-id`; old builds sent `session_id`.
        // Missing headers must not
        // fall back to global state, otherwise independent requests poison each other.
        request_scope_header(headers)
            .map(|value| Self::CodexSession(value.to_string()))
            .unwrap_or_else(Self::next_request_scope)
    }

    pub(crate) fn for_provider(
        &self,
        provider: &str,
        inbound_format: Option<InboundApiFormat>,
    ) -> Self {
        if provider == CODEX_PROVIDER && inbound_format == Some(InboundApiFormat::OpenaiResponses) {
            return self.clone();
        }
        Self::Global
    }

    pub(crate) fn is_global(&self) -> bool {
        matches!(self, Self::Global)
    }

    pub(crate) fn is_request(&self) -> bool {
        matches!(self, Self::Request(_))
    }

    fn next_request_scope() -> Self {
        Self::Request(NEXT_REQUEST_SCOPE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

fn request_scope_header(headers: &HeaderMap) -> Option<&str> {
    [
        THREAD_ID_HEADER,
        SESSION_ID_HEADER,
        LEGACY_SESSION_ID_HEADER,
    ]
    .into_iter()
    .find_map(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use std::{collections::HashMap, time::Duration};

    use super::*;
    use crate::logging::LogLevel;
    use token_proxy_config::{
        UpstreamDispatchRuntime, UpstreamOrderStrategy, UpstreamStrategyRuntime,
    };

    #[test]
    fn codex_responses_scope_uses_thread_id_before_session_id() {
        let config = scoped_config();
        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_static("session-1"));
        headers.insert("thread-id", HeaderValue::from_static("thread-1"));

        let scope = CooldownScope::codex_responses_request(
            &config,
            Some(InboundApiFormat::OpenaiResponses),
            &headers,
        );

        assert_eq!(scope, CooldownScope::CodexSession("thread-1".to_string()));
    }

    #[test]
    fn codex_responses_scope_accepts_legacy_session_id() {
        let config = scoped_config();
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("legacy-session"));

        let scope = CooldownScope::codex_responses_request(
            &config,
            Some(InboundApiFormat::OpenaiResponses),
            &headers,
        );

        assert_eq!(
            scope,
            CooldownScope::CodexSession("legacy-session".to_string())
        );
    }

    fn scoped_config() -> ProxyConfig {
        ProxyConfig {
            host: "127.0.0.1".to_string(),
            port: 9208,
            local_api_key: None,
            cors_enabled: false,
            model_list_prefix: false,
            log_level: LogLevel::Silent,
            max_request_body_bytes: 1024,
            retryable_failure_cooldown: Duration::from_secs(15),
            same_upstream_retry_count: 1,
            codex_session_scoped_cooldown_enabled: true,
            xai_inject_x_search: false,
            stream_first_output_timeout: Duration::from_secs(60),
            sync_response_timeout: Duration::from_secs(120),
            upstream_strategy: UpstreamStrategyRuntime {
                order: UpstreamOrderStrategy::RoundRobin,
                dispatch: UpstreamDispatchRuntime::Serial,
            },
            hot_model_mappings: HashMap::new(),
            upstreams: HashMap::new(),
            kiro_preferred_endpoint: None,
        }
    }
}
