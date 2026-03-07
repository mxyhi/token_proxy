use axum::http::header::{HeaderName, HeaderValue};
use axum::http::HeaderMap;
const HEADER_VERSION: &str = "0.21.0";
const HEADER_OPENAI_BETA: &str = "responses=experimental";
const DEFAULT_USER_AGENT: &str = "codex_cli_rs/0.50.0 (Mac OS 26.0.1; arm64) Apple_Terminal/464";

const HEADER_VERSION_NAME: HeaderName = HeaderName::from_static("version");
const HEADER_OPENAI_BETA_NAME: HeaderName = HeaderName::from_static("openai-beta");
const HEADER_SESSION_ID_NAME: HeaderName = HeaderName::from_static("session_id");
const HEADER_USER_AGENT_NAME: HeaderName = HeaderName::from_static("user-agent");
const HEADER_ACCEPT_NAME: HeaderName = HeaderName::from_static("accept");
const HEADER_CONNECTION_NAME: HeaderName = HeaderName::from_static("connection");
const HEADER_ORIGINATOR_NAME: HeaderName = HeaderName::from_static("originator");
const HEADER_ORIGINATOR: &str = "codex_cli_rs";

pub(crate) fn apply_codex_headers(headers: &mut HeaderMap, inbound: &HeaderMap) {
    ensure_header(headers, inbound, &HEADER_VERSION_NAME, HEADER_VERSION);
    ensure_header(
        headers,
        inbound,
        &HEADER_OPENAI_BETA_NAME,
        HEADER_OPENAI_BETA,
    );
    if !headers.contains_key(&HEADER_SESSION_ID_NAME) {
        if let Ok(value) = HeaderValue::from_str(&generate_session_id()) {
            headers.insert(HEADER_SESSION_ID_NAME, value);
        }
    }
    ensure_header(
        headers,
        inbound,
        &HEADER_USER_AGENT_NAME,
        DEFAULT_USER_AGENT,
    );
    ensure_header(headers, inbound, &HEADER_ORIGINATOR_NAME, HEADER_ORIGINATOR);
    ensure_header(headers, inbound, &HEADER_ACCEPT_NAME, "text/event-stream");
    ensure_header(headers, inbound, &HEADER_CONNECTION_NAME, "Keep-Alive");
}

fn ensure_header(headers: &mut HeaderMap, inbound: &HeaderMap, name: &HeaderName, fallback: &str) {
    if headers.contains_key(name) {
        return;
    }
    if let Some(value) = inbound.get(name) {
        headers.insert(name.clone(), value.clone());
        return;
    }
    if let Ok(value) = HeaderValue::from_str(fallback) {
        headers.insert(name.clone(), value);
    }
}

fn generate_session_id() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
