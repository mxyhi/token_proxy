use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::TryRngCore;
use reqwest::{Client, Proxy};
use sha2::{Digest, Sha256};
use std::time::Duration;
use time::OffsetDateTime;

pub(crate) fn generate_state(prefix: &str) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|err| format!("Failed to generate state: {err}"))?;
    Ok(format!("{prefix}-{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub(crate) fn generate_pkce() -> Result<(String, String), String> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|err| format!("Failed to generate PKCE: {err}"))?;
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    Ok((verifier, challenge))
}

pub(crate) fn sanitize_id_part(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push('_');
        }
    }
    let trimmed = output.trim_matches('_');
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(48).collect()
}

pub(crate) fn extract_email_from_jwt(token: &str) -> Option<String> {
    let value = decode_jwt_payload(token)?;
    value
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("preferred_username").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

pub(crate) fn extract_chatgpt_account_id_from_jwt(token: &str) -> Option<String> {
    let value = decode_jwt_payload(token)?;
    if let Some(id) = value
        .get("https://api.openai.com/auth")
        .and_then(|v| v.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
    {
        return Some(id.to_string());
    }
    value
        .get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub(crate) fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

pub(crate) fn expires_at_from_seconds(expires_in: i64) -> String {
    let seconds = if expires_in <= 0 { 3600 } else { expires_in };
    (OffsetDateTime::now_utc() + time::Duration::seconds(seconds))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

pub(crate) fn build_reqwest_client(
    proxy_url: Option<&str>,
    timeout: Duration,
) -> Result<Client, String> {
    let mut builder = Client::builder().timeout(timeout);
    let proxy_url = proxy_url.map(str::trim).filter(|value| !value.is_empty());
    if let Some(proxy_url) = proxy_url {
        let proxy = Proxy::all(proxy_url)
            .map_err(|_| "app_proxy_url is not a valid URL.".to_string())?;
        // proxy() already disables system proxies; no_proxy() would clear the proxy entirely.
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|err| format!("Failed to build HTTP client: {err}"))
}
