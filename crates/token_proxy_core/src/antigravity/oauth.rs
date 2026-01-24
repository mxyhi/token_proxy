use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

use crate::oauth_util::build_reqwest_client;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json";

const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";

const DEFAULT_TIMEOUT_SECS: u64 = 20;

const SCOPES: [&str; 5] = [
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AntigravityTokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: i64,
    pub(crate) token_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct UserInfoResponse {
    email: Option<String>,
}

pub(crate) struct AntigravityOAuthClient {
    proxy_url: Option<String>,
}

impl AntigravityOAuthClient {
    pub(crate) fn new(proxy_url: Option<String>) -> Self {
        Self { proxy_url }
    }

    pub(crate) fn build_authorize_url(redirect_uri: &str, state: &str) -> String {
        let mut params = HashMap::new();
        params.insert("access_type", "offline");
        params.insert("client_id", CLIENT_ID);
        params.insert("prompt", "consent");
        params.insert("redirect_uri", redirect_uri);
        params.insert("response_type", "code");
        let scope = SCOPES.join(" ");
        params.insert("scope", scope.as_str());
        params.insert("state", state);
        let query = serde_urlencoded::to_string(params).unwrap_or_default();
        format!("{AUTH_URL}?{query}")
    }

    pub(crate) async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<AntigravityTokenResponse, String> {
        let client = build_reqwest_client(self.proxy_url.as_deref(), Duration::from_secs(DEFAULT_TIMEOUT_SECS))?;
        let mut params = HashMap::new();
        params.insert("code", code);
        params.insert("client_id", CLIENT_ID);
        params.insert("client_secret", CLIENT_SECRET);
        params.insert("redirect_uri", redirect_uri);
        params.insert("grant_type", "authorization_code");
        let response = client
            .post(TOKEN_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await
            .map_err(|err| format!("Token exchange failed: {err}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Token exchange failed: {status} {body}"));
        }
        response
            .json::<AntigravityTokenResponse>()
            .await
            .map_err(|err| format!("Failed to parse token response: {err}"))
    }

    pub(crate) async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<AntigravityTokenResponse, String> {
        let client = build_reqwest_client(self.proxy_url.as_deref(), Duration::from_secs(DEFAULT_TIMEOUT_SECS))?;
        let mut params = HashMap::new();
        params.insert("client_id", CLIENT_ID);
        params.insert("client_secret", CLIENT_SECRET);
        params.insert("refresh_token", refresh_token);
        params.insert("grant_type", "refresh_token");
        let response = client
            .post(TOKEN_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await
            .map_err(|err| format!("Token refresh failed: {err}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Token refresh failed: {status} {body}"));
        }
        response
            .json::<AntigravityTokenResponse>()
            .await
            .map_err(|err| format!("Failed to parse refresh response: {err}"))
    }

    pub(crate) async fn fetch_user_email(&self, access_token: &str) -> Result<Option<String>, String> {
        if access_token.trim().is_empty() {
            return Ok(None);
        }
        let client = build_reqwest_client(self.proxy_url.as_deref(), Duration::from_secs(DEFAULT_TIMEOUT_SECS))?;
        let response = client
            .get(USERINFO_URL)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|err| format!("Failed to fetch user info: {err}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("User info request failed: {status} {body}"));
        }
        let payload = response
            .json::<UserInfoResponse>()
            .await
            .map_err(|err| format!("Failed to parse user info: {err}"))?;
        Ok(payload.email.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()))
    }
}
