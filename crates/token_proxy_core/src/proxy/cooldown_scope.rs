use axum::http::HeaderMap;
use std::sync::atomic::{AtomicU64, Ordering};

use super::config::{InboundApiFormat, ProxyConfig};

const CODEX_PROVIDER: &str = "codex";
const SESSION_ID_HEADER: &str = "session_id";

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

        // Codex CLI sends `session_id` for conversations. Missing headers must not
        // fall back to global state, otherwise independent requests poison each other.
        headers
            .get(SESSION_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
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
