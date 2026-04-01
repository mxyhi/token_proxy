use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::oauth_util::build_reqwest_client;

use super::types::KiroTokenRecord;
use super::util::{expires_at_from_seconds, now_rfc3339};

const KIRO_AUTH_ENDPOINT: &str = "https://prod.us-east-1.auth.desktop.kiro.dev";
const KIRO_USER_AGENT: &str = "token-proxy/1.0.0";

#[derive(Clone)]
pub(crate) struct KiroOAuthClient {
    http: Client,
}

impl KiroOAuthClient {
    pub(crate) fn new(proxy_url: Option<&str>) -> Result<Self, String> {
        let http = build_reqwest_client(proxy_url, std::time::Duration::from_secs(30))
            .map_err(|err| format!("Failed to build Kiro OAuth client: {err}"))?;
        Ok(Self { http })
    }

    pub(crate) async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<KiroTokenResponse, String> {
        let payload = CreateTokenRequest {
            code: code.to_string(),
            code_verifier: code_verifier.to_string(),
            redirect_uri: redirect_uri.to_string(),
        };
        self.post_json("/oauth/token", &payload).await
    }

    pub(crate) async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<KiroTokenResponse, String> {
        let payload = RefreshTokenRequest {
            refresh_token: refresh_token.to_string(),
        };
        self.post_json("/refreshToken", &payload).await
    }

    async fn post_json<TReq: Serialize, TRes: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        payload: &TReq,
    ) -> Result<TRes, String> {
        let url = format!("{KIRO_AUTH_ENDPOINT}{path}");
        let response = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("User-Agent", KIRO_USER_AGENT)
            .json(payload)
            .send()
            .await
            .map_err(|err| format!("Kiro OAuth request failed: {err}"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("Failed to read Kiro OAuth response: {err}"))?;
        if !status.is_success() {
            return Err(format!(
                "Kiro OAuth request failed (status {})",
                status.as_u16()
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|err| format!("Failed to parse Kiro OAuth response: {err}"))
    }
}

pub(crate) fn build_login_url(
    provider: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("idp", provider)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("prompt", "select_account")
        .finish();
    format!("{KIRO_AUTH_ENDPOINT}/login?{query}")
}

pub(crate) async fn refresh_social_token(
    record: &KiroTokenRecord,
    proxy_url: Option<&str>,
) -> Result<KiroTokenRecord, String> {
    let client = KiroOAuthClient::new(proxy_url)?;
    let response = client.refresh_token(&record.refresh_token).await?;
    Ok(KiroTokenRecord {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        profile_arn: response.profile_arn,
        expires_at: expires_at_from_seconds(response.expires_in),
        auth_method: "social".to_string(),
        provider: record.provider.clone(),
        client_id: record.client_id.clone(),
        client_secret: record.client_secret.clone(),
        email: record.email.clone(),
        last_refresh: Some(now_rfc3339()),
        start_url: record.start_url.clone(),
        region: record.region.clone(),
        status: record.status,
        proxy_url: record.proxy_url.clone(),
        quota: record.quota.clone(),
    })
}

#[derive(Serialize)]
struct CreateTokenRequest {
    code: String,
    code_verifier: String,
    redirect_uri: String,
}

#[derive(Serialize)]
struct RefreshTokenRequest {
    #[serde(rename = "refreshToken")]
    refresh_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroTokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) profile_arn: Option<String>,
    pub(crate) expires_in: i64,
}
