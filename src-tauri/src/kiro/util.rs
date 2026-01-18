use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
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
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("preferred_username").and_then(|v| v.as_str()))
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
