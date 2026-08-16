use axum::http::header::{HeaderName, HeaderValue};
use axum::http::HeaderMap;

use token_proxy_account_codex::{
    is_official_originator, official_originator_from_user_agent, supported_official_user_agent,
    DEFAULT_ORIGINATOR, USER_AGENT,
};

const HEADER_USER_AGENT_NAME: HeaderName = HeaderName::from_static("user-agent");
const HEADER_ACCEPT_NAME: HeaderName = HeaderName::from_static("accept");
const HEADER_OPENAI_BETA_NAME: HeaderName = HeaderName::from_static("openai-beta");
const HEADER_LEGACY_SESSION_ID_NAME: HeaderName = HeaderName::from_static("session_id");
const HEADER_CONNECTION_NAME: HeaderName = HeaderName::from_static("connection");
const HEADER_ORIGINATOR_NAME: HeaderName = HeaderName::from_static("originator");
const HEADER_SESSION_ID_NAME: HeaderName = HeaderName::from_static("session-id");
const HEADER_THREAD_ID_NAME: HeaderName = HeaderName::from_static("thread-id");
const HEADER_CLIENT_REQUEST_ID_NAME: HeaderName = HeaderName::from_static("x-client-request-id");
const HEADER_CODEX_BETA_FEATURES_NAME: HeaderName =
    HeaderName::from_static("x-codex-beta-features");
const REMOTE_COMPACTION_V2: &str = "remote_compaction_v2";

pub(crate) fn apply_codex_headers(headers: &mut HeaderMap, inbound: &HeaderMap) {
    headers.remove(&HEADER_OPENAI_BETA_NAME);
    headers.remove(&HEADER_LEGACY_SESSION_ID_NAME);
    headers.remove(&HEADER_CONNECTION_NAME);

    let fallback_session_id = generate_session_id();
    let session_id = copy_inbound_or_generate(
        headers,
        inbound,
        &HEADER_SESSION_ID_NAME,
        &fallback_session_id,
    );
    let thread_id = copy_inbound_or_generate(headers, inbound, &HEADER_THREAD_ID_NAME, &session_id);
    copy_inbound_or_generate(headers, inbound, &HEADER_CLIENT_REQUEST_ID_NAME, &thread_id);

    apply_codex_identity_headers(headers, inbound);
    apply_default_remote_compaction_feature(headers);
    force_header(headers, &HEADER_ACCEPT_NAME, "text/event-stream");
}

fn apply_default_remote_compaction_feature(headers: &mut HeaderMap) {
    if headers
        .get(&HEADER_CODEX_BETA_FEATURES_NAME)
        .and_then(valid_header_value)
        .is_some()
    {
        return;
    }
    force_header(
        headers,
        &HEADER_CODEX_BETA_FEATURES_NAME,
        REMOTE_COMPACTION_V2,
    );
    tracing::debug!("enabled default Codex remote compaction feature");
}

pub(crate) fn ensure_remote_compaction_feature(headers: &mut HeaderMap) {
    let existing = headers
        .get(&HEADER_CODEX_BETA_FEATURES_NAME)
        .and_then(valid_header_value)
        .unwrap_or_default()
        .to_string();
    if existing
        .split(',')
        .map(str::trim)
        .any(|feature| feature == REMOTE_COMPACTION_V2)
    {
        return;
    }
    let had_existing_features = !existing.is_empty();
    let merged = if existing.is_empty() {
        REMOTE_COMPACTION_V2.to_string()
    } else {
        format!("{existing},{REMOTE_COMPACTION_V2}")
    };
    force_header(headers, &HEADER_CODEX_BETA_FEATURES_NAME, &merged);
    tracing::debug!(
        had_existing_features,
        "merged Codex remote compaction feature"
    );
}

fn force_header(headers: &mut HeaderMap, name: &HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name.clone(), value);
    }
}

fn copy_inbound_or_generate(
    headers: &mut HeaderMap,
    inbound: &HeaderMap,
    name: &HeaderName,
    fallback: &str,
) -> String {
    if let Some(value) = inbound.get(name).and_then(valid_header_value) {
        if let Ok(header_value) = HeaderValue::from_str(value) {
            headers.insert(name.clone(), header_value);
            return value.to_string();
        }
    }
    if let Some(value) = headers.get(name).and_then(valid_header_value) {
        return value.to_string();
    }
    force_header(headers, name, fallback);
    fallback.to_string()
}

fn valid_header_value(value: &HeaderValue) -> Option<&str> {
    value
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn apply_codex_identity_headers(headers: &mut HeaderMap, inbound: &HeaderMap) {
    let inbound_user_agent = inbound
        .get(&HEADER_USER_AGENT_NAME)
        .and_then(valid_header_value);
    let user_agent = if is_native_codex_request(inbound) {
        inbound_user_agent
            .and_then(supported_official_user_agent)
            .unwrap_or(USER_AGENT)
    } else {
        USER_AGENT
    };
    if inbound_user_agent.is_some_and(|value| value != user_agent) {
        tracing::debug!(
            inbound_user_agent,
            user_agent,
            "replaced unsupported Codex client identity"
        );
    }
    force_header(headers, &HEADER_USER_AGENT_NAME, user_agent);

    let originator = official_originator_from_user_agent(user_agent).unwrap_or(DEFAULT_ORIGINATOR);
    force_header(headers, &HEADER_ORIGINATOR_NAME, originator);
}

pub(crate) fn is_native_codex_request(inbound: &HeaderMap) -> bool {
    inbound
        .get(&HEADER_ORIGINATOR_NAME)
        .and_then(valid_header_value)
        .is_some_and(is_official_originator)
        || inbound
            .get(&HEADER_USER_AGENT_NAME)
            .and_then(valid_header_value)
            .and_then(official_originator_from_user_agent)
            .is_some()
}

fn generate_session_id() -> String {
    crate::proxy::kiro::utils::random_uuid()
}
