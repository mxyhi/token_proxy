use axum::http::HeaderMap;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

const TURN_STATE_HEADER: &str = "x-codex-turn-state";
const SESSION_ID_HEADER: &str = "session-id";
const LEGACY_SESSION_ID_HEADER: &str = "session_id";
const DEFAULT_PROVENANCE_TTL: Duration = Duration::from_secs(60 * 60);
const SWEEP_WRITE_INTERVAL: usize = 256;

#[derive(Clone)]
struct TurnStateOrigin {
    account_id: String,
    expires_at: Instant,
}

pub(super) struct CodexTurnStateProvenance {
    origins: Mutex<HashMap<String, TurnStateOrigin>>,
    writes: AtomicUsize,
    ttl: Duration,
}

#[derive(Clone)]
pub(super) struct CodexResponseIdentity {
    pub(super) account_id: String,
}

impl CodexTurnStateProvenance {
    pub(super) fn new() -> Self {
        Self::with_ttl(DEFAULT_PROVENANCE_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            origins: Mutex::new(HashMap::new()),
            writes: AtomicUsize::new(0),
            ttl,
        }
    }

    pub(super) fn guard_echo(
        &self,
        provider: &str,
        inbound: &HeaderMap,
        selected_account_id: Option<&str>,
        outbound: &mut HeaderMap,
    ) {
        if provider != "codex" || !has_non_empty_header(outbound, TURN_STATE_HEADER) {
            return;
        }
        let Some(account_id) = selected_account_id.filter(|value| !value.trim().is_empty()) else {
            return;
        };
        let Some(seed) = client_session_seed(inbound) else {
            return;
        };
        let Ok(mut origins) = self.origins.lock() else {
            tracing::warn!("Codex turn-state provenance lock poisoned; preserving client header");
            return;
        };
        let Some(origin) = origins.get(&seed).cloned() else {
            return;
        };
        if Instant::now() >= origin.expires_at {
            origins.remove(&seed);
            return;
        }
        if origin.account_id != account_id {
            outbound.remove(TURN_STATE_HEADER);
            tracing::debug!("removed Codex turn-state minted by a different account");
        }
    }

    pub(super) fn note_committed_response(
        &self,
        inbound: &HeaderMap,
        response: &axum::response::Response,
    ) {
        if !has_non_empty_header(response.headers(), TURN_STATE_HEADER) {
            return;
        }
        let Some(identity) = response.extensions().get::<CodexResponseIdentity>() else {
            return;
        };
        let Some(seed) = client_session_seed(inbound) else {
            return;
        };
        let Ok(mut origins) = self.origins.lock() else {
            tracing::warn!("Codex turn-state provenance lock poisoned; skipping provenance write");
            return;
        };
        origins.insert(
            seed,
            TurnStateOrigin {
                account_id: identity.account_id.clone(),
                expires_at: Instant::now() + self.ttl,
            },
        );
        let should_sweep = self.writes.fetch_add(1, Ordering::Relaxed) % SWEEP_WRITE_INTERVAL
            == SWEEP_WRITE_INTERVAL - 1;
        if should_sweep {
            let now = Instant::now();
            origins.retain(|_, origin| now < origin.expires_at);
        }
        tracing::debug!("recorded committed Codex turn-state provenance");
    }
}

fn client_session_seed(headers: &HeaderMap) -> Option<String> {
    [SESSION_ID_HEADER, LEGACY_SESSION_ID_HEADER]
        .into_iter()
        .find_map(|name| header_text(headers, name).map(str::to_string))
}

fn has_non_empty_header(headers: &HeaderMap, name: &str) -> bool {
    header_text(headers, name).is_some()
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::HeaderValue, response::Response};

    fn headers(session: &str, turn_state: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if !session.is_empty() {
            headers.insert(
                SESSION_ID_HEADER,
                HeaderValue::from_str(session).expect("session"),
            );
        }
        if !turn_state.is_empty() {
            headers.insert(
                TURN_STATE_HEADER,
                HeaderValue::from_str(turn_state).expect("turn state"),
            );
        }
        headers
    }

    fn committed_response(account_id: &str) -> Response {
        let mut response = Response::new(Body::empty());
        response
            .headers_mut()
            .insert(TURN_STATE_HEADER, HeaderValue::from_static("blob-a"));
        response.extensions_mut().insert(CodexResponseIdentity {
            account_id: account_id.to_string(),
        });
        response
    }

    #[test]
    fn same_account_preserves_and_different_account_removes_turn_state() {
        let provenance = CodexTurnStateProvenance::new();
        let inbound = headers("session-a", "blob-a");
        provenance.note_committed_response(&inbound, &committed_response("account-a"));

        let mut same = inbound.clone();
        provenance.guard_echo("codex", &inbound, Some("account-a"), &mut same);
        assert!(same.contains_key(TURN_STATE_HEADER));

        let mut different = inbound.clone();
        provenance.guard_echo("codex", &inbound, Some("account-b"), &mut different);
        assert!(!different.contains_key(TURN_STATE_HEADER));
    }

    #[test]
    fn unknown_missing_session_and_expired_provenance_preserve_turn_state() {
        let provenance = CodexTurnStateProvenance::with_ttl(Duration::ZERO);
        let inbound = headers("session-a", "blob-a");

        let mut unknown = inbound.clone();
        provenance.guard_echo("codex", &inbound, Some("account-b"), &mut unknown);
        assert!(unknown.contains_key(TURN_STATE_HEADER));

        provenance.note_committed_response(&inbound, &committed_response("account-a"));
        let mut expired = inbound.clone();
        provenance.guard_echo("codex", &inbound, Some("account-b"), &mut expired);
        assert!(expired.contains_key(TURN_STATE_HEADER));

        let no_session = headers("", "blob-a");
        let mut outbound = no_session.clone();
        provenance.guard_echo("codex", &no_session, Some("account-b"), &mut outbound);
        assert!(outbound.contains_key(TURN_STATE_HEADER));
    }

    #[test]
    fn uncommitted_attempt_does_not_create_provenance() {
        let provenance = CodexTurnStateProvenance::new();
        let inbound = headers("session-a", "blob-a");
        let response = Response::new(Body::empty());
        provenance.note_committed_response(&inbound, &response);

        let mut outbound = inbound.clone();
        provenance.guard_echo("codex", &inbound, Some("account-b"), &mut outbound);
        assert!(outbound.contains_key(TURN_STATE_HEADER));
    }
}
