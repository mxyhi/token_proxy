//! Agent Identity credential verification, signing, task registration, and recovery classification.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use crypto_box::SecretKey as Curve25519SecretKey;
use ed25519_dalek::pkcs8::DecodePrivateKey as _;
use ed25519_dalek::{Signer as _, SigningKey};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha512};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::types::CodexAgentIdentityRef;

const AGENT_TASK_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);
const AGENT_IDENTITY_JWKS_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_IDENTITY_JWT_AUDIENCE: &str = "codex-app-server";
const AGENT_IDENTITY_JWT_ISSUER: &str = "https://chatgpt.com/codex-backend/agent-identity";
pub(crate) const DEFAULT_CHATGPT_BACKEND_URL: &str = "https://chatgpt.com/backend-api";
pub(crate) const DEFAULT_AGENT_IDENTITY_AUTH_URL: &str = "https://auth.openai.com/api/accounts";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct AgentIdentityJwtClaims {
    pub iss: String,
    pub aud: String,
    pub iat: usize,
    pub exp: usize,
    pub agent_runtime_id: String,
    pub agent_private_key: String,
    pub account_id: String,
    pub chatgpt_user_id: String,
    pub email: String,
    pub plan_type: Value,
    pub chatgpt_account_is_fedramp: bool,
}

#[derive(Serialize)]
struct RegisterTaskRequest {
    timestamp: String,
    signature: String,
}

#[derive(Deserialize)]
struct RegisterTaskResponse {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default, rename = "taskId")]
    task_id_camel: Option<String>,
    #[serde(default)]
    encrypted_task_id: Option<String>,
    #[serde(default, rename = "encryptedTaskId")]
    encrypted_task_id_camel: Option<String>,
}

pub(crate) async fn fetch_jwks(
    client: &reqwest::Client,
    chatgpt_backend_url: &str,
) -> Result<JwkSet, String> {
    let response = client
        .get(agent_identity_jwks_url(chatgpt_backend_url))
        .timeout(AGENT_IDENTITY_JWKS_TIMEOUT)
        .send()
        .await
        .map_err(|_| "Failed to request Agent Identity JWKS.".to_string())?
        .error_for_status()
        .map_err(|_| "Agent Identity JWKS endpoint returned an error.".to_string())?;
    response
        .json()
        .await
        .map_err(|_| "Failed to decode Agent Identity JWKS.".to_string())
}

pub(crate) fn verify_jwt(jwt: &str, jwks: &JwkSet) -> Result<AgentIdentityJwtClaims, String> {
    let header = decode_header(jwt)
        .map_err(|_| "Failed to decode Agent Identity JWT header.".to_string())?;
    if header.alg != Algorithm::RS256 {
        return Err("Agent Identity JWT must use RS256.".to_string());
    }
    let kid = header
        .kid
        .ok_or_else(|| "Agent Identity JWT header does not include a kid.".to_string())?;
    let jwk = jwks
        .find(&kid)
        .ok_or_else(|| "Agent Identity JWT signing key is not trusted.".to_string())?;
    let decoding_key = DecodingKey::from_jwk(jwk)
        .map_err(|_| "Failed to build Agent Identity JWT decoding key.".to_string())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[AGENT_IDENTITY_JWT_AUDIENCE]);
    validation.set_issuer(&[AGENT_IDENTITY_JWT_ISSUER]);
    validation.required_spec_claims.insert("iss".to_string());
    validation.required_spec_claims.insert("aud".to_string());
    let claims = decode::<AgentIdentityJwtClaims>(jwt, &decoding_key, &validation)
        .map_err(|_| "Failed to verify Agent Identity JWT.".to_string())?
        .claims;
    validate_claims(&claims)?;
    Ok(claims)
}

pub(crate) fn validate_identity(identity: CodexAgentIdentityRef<'_>) -> Result<(), String> {
    if identity.agent_runtime_id.trim().is_empty() {
        return Err("Agent Identity runtime ID is required.".to_string());
    }
    signing_key(identity.agent_private_key)?;
    Ok(())
}

pub(crate) fn authorization_header(identity: CodexAgentIdentityRef<'_>) -> Result<String, String> {
    let task_id = identity
        .task_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Agent Identity task ID is required.".to_string())?;
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| "Failed to format Agent Identity assertion timestamp.".to_string())?;
    authorization_header_at(identity, task_id, &timestamp)
}

fn authorization_header_at(
    identity: CodexAgentIdentityRef<'_>,
    task_id: &str,
    timestamp: &str,
) -> Result<String, String> {
    let payload = format!("{}:{task_id}:{timestamp}", identity.agent_runtime_id);
    let signature = BASE64_STANDARD.encode(
        signing_key(identity.agent_private_key)?
            .sign(payload.as_bytes())
            .to_bytes(),
    );
    // BTreeMap preserves the official deterministic envelope ordering.
    let envelope = BTreeMap::from([
        ("agent_runtime_id", identity.agent_runtime_id),
        ("signature", signature.as_str()),
        ("task_id", task_id),
        ("timestamp", timestamp),
    ]);
    let encoded = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&envelope)
            .map_err(|_| "Failed to serialize Agent Identity assertion.".to_string())?,
    );
    Ok(format!("AgentAssertion {encoded}"))
}

/// 比较实际发送的 assertion 与当前身份，使用原 timestamp，避免把旧任务结果写入新身份。
pub(crate) fn authorization_matches(identity: CodexAgentIdentityRef<'_>, header: &str) -> bool {
    let Some(encoded) = header.strip_prefix("AgentAssertion ") else {
        return false;
    };
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(encoded) else {
        return false;
    };
    let Ok(envelope) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    let Some(timestamp) = envelope.get("timestamp").and_then(Value::as_str) else {
        return false;
    };
    let Some(task_id) = identity.task_id else {
        return false;
    };
    authorization_header_at(identity, task_id, timestamp).is_ok_and(|current| current == header)
}

pub(crate) async fn register_task(
    client: &reqwest::Client,
    auth_base_url: &str,
    identity: CodexAgentIdentityRef<'_>,
) -> Result<String, String> {
    validate_identity(identity)?;
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| "Failed to format Agent Identity registration timestamp.".to_string())?;
    let payload = format!("{}:{timestamp}", identity.agent_runtime_id);
    let request = RegisterTaskRequest {
        timestamp,
        signature: BASE64_STANDARD.encode(
            signing_key(identity.agent_private_key)?
                .sign(payload.as_bytes())
                .to_bytes(),
        ),
    };
    let url = format!(
        "{}/v1/agent/{}/task/register",
        auth_base_url.trim_end_matches('/'),
        identity.agent_runtime_id
    );
    let response = client
        .post(url)
        .timeout(AGENT_TASK_REGISTRATION_TIMEOUT)
        .json(&request)
        .send()
        .await
        .map_err(|_| "Agent Identity task registration request failed.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Agent Identity task registration returned status {}.",
            response.status().as_u16()
        ));
    }
    let response = response
        .json::<RegisterTaskResponse>()
        .await
        .map_err(|_| "Agent Identity task registration response is invalid.".to_string())?;
    task_id_from_response(identity, response)
}

pub(crate) fn is_task_invalid_response(status: u16, body: &[u8]) -> bool {
    if status != 401 {
        return false;
    }
    let lower = String::from_utf8_lossy(body).to_ascii_lowercase();
    let compact = lower
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    [
        r#""code":"invalid_task_id""#,
        r#""code":"task_not_found""#,
        r#""code":"task_expired""#,
        r#""error":"invalid_task_id""#,
    ]
    .iter()
    .any(|marker| compact.contains(marker))
        || [
            "invalid task_id",
            "invalid task id",
            "task_id is invalid",
            "task id is invalid",
            "task not found",
            "task expired",
            "unknown task_id",
            "unknown task id",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
}

pub(crate) fn normalize_plan_type(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            Some(value.trim().to_ascii_lowercase()).filter(|value| !value.is_empty())
        }
        Value::Object(object) => object
            .get("type")
            .or_else(|| object.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase),
        _ => None,
    }
}

fn validate_claims(claims: &AgentIdentityJwtClaims) -> Result<(), String> {
    if claims.agent_runtime_id.trim().is_empty()
        || claims.account_id.trim().is_empty()
        || claims.chatgpt_user_id.trim().is_empty()
    {
        return Err("Agent Identity JWT is missing required identity claims.".to_string());
    }
    signing_key(&claims.agent_private_key)?;
    Ok(())
}

fn task_id_from_response(
    identity: CodexAgentIdentityRef<'_>,
    response: RegisterTaskResponse,
) -> Result<String, String> {
    if let Some(task_id) = response.task_id.or(response.task_id_camel) {
        let task_id = task_id.trim();
        if !task_id.is_empty() {
            return Ok(task_id.to_string());
        }
    }
    let encrypted = response
        .encrypted_task_id
        .or(response.encrypted_task_id_camel)
        .ok_or_else(|| "Agent Identity task registration response omitted task ID.".to_string())?;
    decrypt_task_id(identity.agent_private_key, &encrypted)
}

fn decrypt_task_id(private_key: &str, encrypted_task_id: &str) -> Result<String, String> {
    let signing_key = signing_key(private_key)?;
    let ciphertext = BASE64_STANDARD
        .decode(encrypted_task_id.trim())
        .map_err(|_| "Encrypted Agent Identity task ID is not valid base64.".to_string())?;
    let plaintext = curve25519_secret_key(&signing_key)
        .unseal(&ciphertext)
        .map_err(|_| "Failed to decrypt Agent Identity task ID.".to_string())?;
    let task_id = String::from_utf8(plaintext)
        .map_err(|_| "Decrypted Agent Identity task ID is not valid UTF-8.".to_string())?;
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Err("Decrypted Agent Identity task ID is empty.".to_string());
    }
    Ok(task_id.to_string())
}

fn curve25519_secret_key(signing_key: &SigningKey) -> Curve25519SecretKey {
    let digest = Sha512::digest(signing_key.to_bytes());
    let mut secret_key = [0_u8; 32];
    secret_key.copy_from_slice(&digest[..32]);
    secret_key[0] &= 248;
    secret_key[31] &= 127;
    secret_key[31] |= 64;
    Curve25519SecretKey::from(secret_key)
}

fn signing_key(private_key: &str) -> Result<SigningKey, String> {
    let der = BASE64_STANDARD
        .decode(private_key.trim())
        .map_err(|_| "Agent Identity private key is not valid base64.".to_string())?;
    SigningKey::from_pkcs8_der(&der)
        .map_err(|_| "Agent Identity private key is not a valid PKCS#8 Ed25519 key.".to_string())
}

fn agent_identity_jwks_url(chatgpt_backend_url: &str) -> String {
    let base = chatgpt_backend_url.trim_end_matches('/');
    if base.contains("/backend-api") {
        format!("{base}/wham/agent-identities/jwks")
    } else {
        format!("{base}/agent-identities/jwks")
    }
}

#[cfg(test)]
mod tests {
    use crypto_box::aead::OsRng;
    use ed25519_dalek::pkcs8::EncodePrivateKey as _;
    use ed25519_dalek::{Signature, Verifier as _};
    use jsonwebtoken::{EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey as _;
    use rsa::traits::PublicKeyParts as _;
    use rsa::RsaPrivateKey;

    use super::*;

    fn test_identity(task_id: Option<&str>) -> (String, CodexAgentIdentityRef<'_>) {
        let encoded = BASE64_STANDARD.encode(
            SigningKey::from_bytes(&[7_u8; 32])
                .to_pkcs8_der()
                .expect("encode test key")
                .as_bytes(),
        );
        // The leaked test allocation keeps the borrowed test fixture simple and contains no real secret.
        let encoded = Box::leak(encoded.into_boxed_str()).to_string();
        let private_key = Box::leak(encoded.clone().into_boxed_str());
        (
            encoded,
            CodexAgentIdentityRef {
                agent_runtime_id: "runtime-test",
                agent_private_key: private_key,
                task_id,
                plan_type: Some("team"),
                chatgpt_account_is_fedramp: false,
            },
        )
    }

    #[test]
    fn availability_result_assertion_must_match_current_task_and_key() {
        let (_, identity) = test_identity(Some("task-a"));
        let header = authorization_header_at(identity, "task-a", "2026-07-22T12:00:00Z").unwrap();
        let (_, current) = test_identity(Some("task-a"));
        assert!(authorization_matches(current, &header));
        let (_, replaced) = test_identity(Some("task-b"));
        assert!(!authorization_matches(replaced, &header));
        let (_, current) = test_identity(Some("task-a"));
        assert!(!authorization_matches(current, "AgentAssertion invalid"));
    }

    #[test]
    fn assertion_uses_official_payload_and_envelope() {
        let (_key, identity) = test_identity(Some("task-test"));
        let header = authorization_header_at(identity, "task-test", "2026-07-22T12:00:00Z")
            .expect("build assertion");
        assert!(header.starts_with("AgentAssertion "));
        let encoded = header.trim_start_matches("AgentAssertion ");
        let envelope: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).expect("decode assertion"))
                .expect("decode envelope");
        assert_eq!(envelope["agent_runtime_id"], "runtime-test");
        assert_eq!(envelope["task_id"], "task-test");
        assert_eq!(envelope["timestamp"], "2026-07-22T12:00:00Z");
        let signature = BASE64_STANDARD
            .decode(envelope["signature"].as_str().expect("signature string"))
            .expect("decode assertion signature");
        let signature = Signature::from_slice(&signature).expect("Ed25519 signature");
        SigningKey::from_bytes(&[7_u8; 32])
            .verifying_key()
            .verify(b"runtime-test:task-test:2026-07-22T12:00:00Z", &signature)
            .expect("assertion signature should verify");
    }

    #[test]
    fn encrypted_task_id_round_trips_through_official_sealed_box_format() {
        let (private_key, _identity) = test_identity(None);
        let signing_key = signing_key(&private_key).expect("decode test signing key");
        let ciphertext = curve25519_secret_key(&signing_key)
            .public_key()
            .seal(&mut OsRng, b"task-sealed")
            .expect("seal task ID");
        let encoded = BASE64_STANDARD.encode(ciphertext);

        assert_eq!(
            decrypt_task_id(&private_key, &encoded).expect("decrypt task ID"),
            "task-sealed"
        );
    }

    #[test]
    fn jwt_verification_accepts_trusted_rs256_identity() {
        let (private_key, _identity) = test_identity(None);
        let (encoding_key, jwks) = test_rsa_keypair("test-key");
        let jwt = jsonwebtoken::encode(
            &test_jwt_header("test-key"),
            &serde_json::json!({
                "iss": AGENT_IDENTITY_JWT_ISSUER,
                "aud": AGENT_IDENTITY_JWT_AUDIENCE,
                "iat": 1_700_000_000usize,
                "exp": 4_000_000_000usize,
                "agent_runtime_id": "runtime-jwt",
                "agent_private_key": private_key,
                "account_id": "account-jwt",
                "chatgpt_user_id": "user-jwt",
                "email": "jwt@example.com",
                "plan_type": "pro",
                "chatgpt_account_is_fedramp": false
            }),
            &encoding_key,
        )
        .expect("encode Agent Identity JWT");

        let claims = verify_jwt(&jwt, &jwks).expect("verify trusted JWT");

        assert_eq!(claims.agent_runtime_id, "runtime-jwt");
        assert_eq!(claims.account_id, "account-jwt");
        assert_eq!(
            normalize_plan_type(&claims.plan_type).as_deref(),
            Some("pro")
        );
    }

    #[test]
    fn invalid_task_classifier_is_narrow_and_ignores_non_401() {
        assert!(is_task_invalid_response(
            401,
            br#"{"error":{"code":"invalid_task_id"}}"#
        ));
        assert!(is_task_invalid_response(401, b"task expired"));
        assert!(!is_task_invalid_response(403, b"task expired"));
        assert!(!is_task_invalid_response(401, b"invalid bearer token"));
    }

    #[test]
    fn private_key_validation_rejects_non_pkcs8_material() {
        let (_key, mut identity) = test_identity(None);
        identity.agent_private_key = "not-a-private-key";
        assert!(validate_identity(identity).is_err());
    }

    #[test]
    fn jwks_url_matches_chatgpt_backend_contract() {
        assert_eq!(
            agent_identity_jwks_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/wham/agent-identities/jwks"
        );
    }

    fn test_jwt_header(kid: &str) -> Header {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        header
    }

    fn test_rsa_keypair(kid: &str) -> (EncodingKey, JwkSet) {
        let private_key =
            RsaPrivateKey::new(&mut OsRng, 2_048).expect("generate temporary RSA test key");
        let public_key = private_key.to_public_key();
        let pkcs1 = private_key
            .to_pkcs1_der()
            .expect("encode temporary RSA test key");
        let encoding_key = EncodingKey::from_rsa_der(pkcs1.as_bytes());
        let jwks = serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "kid": kid,
                "use": "sig",
                "alg": "RS256",
                "n": URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be()),
                "e": URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be())
            }]
        }))
        .expect("temporary test JWKS should parse");

        (encoding_key, jwks)
    }
}
