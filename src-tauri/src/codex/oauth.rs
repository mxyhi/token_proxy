use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::oauth_util::build_reqwest_client;

const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Clone)]
pub(crate) struct CodexOAuthClient {
    http: Client,
}

impl CodexOAuthClient {
    pub(crate) fn new(proxy_url: Option<&str>) -> Result<Self, String> {
        let http = build_reqwest_client(proxy_url, std::time::Duration::from_secs(30))
            .map_err(|err| format!("Failed to build Codex OAuth client: {err}"))?;
        Ok(Self { http })
    }

    pub(crate) fn build_authorize_url(
        redirect_uri: &str,
        state: &str,
        code_challenge: &str,
    ) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", OPENAI_CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", "openid email profile offline_access")
            .append_pair("state", state)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "login")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .finish();
        format!("{OPENAI_AUTH_URL}?{query}")
    }

    pub(crate) async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<CodexTokenResponse, String> {
        let payload = TokenExchangeRequest {
            grant_type: "authorization_code".to_string(),
            client_id: OPENAI_CLIENT_ID.to_string(),
            code: code.to_string(),
            redirect_uri: redirect_uri.to_string(),
            code_verifier: code_verifier.to_string(),
            refresh_token: None,
            scope: None,
        };
        self.post_form(payload).await
    }

    pub(crate) async fn refresh_token(&self, refresh_token: &str) -> Result<CodexTokenResponse, String> {
        let payload = TokenExchangeRequest {
            grant_type: "refresh_token".to_string(),
            client_id: OPENAI_CLIENT_ID.to_string(),
            code: String::new(),
            redirect_uri: String::new(),
            code_verifier: String::new(),
            refresh_token: Some(refresh_token.to_string()),
            scope: Some("openid profile email".to_string()),
        };
        self.post_form(payload).await
    }

    async fn post_form(&self, payload: TokenExchangeRequest) -> Result<CodexTokenResponse, String> {
        let body = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.append_pair("grant_type", &payload.grant_type)
                .append_pair("client_id", &payload.client_id);
            if !payload.code.is_empty() {
                form.append_pair("code", &payload.code);
            }
            if !payload.redirect_uri.is_empty() {
                form.append_pair("redirect_uri", &payload.redirect_uri);
            }
            if !payload.code_verifier.is_empty() {
                form.append_pair("code_verifier", &payload.code_verifier);
            }
            if let Some(refresh_token) = payload.refresh_token.as_deref() {
                form.append_pair("refresh_token", refresh_token);
            }
            if let Some(scope) = payload.scope.as_deref() {
                form.append_pair("scope", scope);
            }
            form.finish()
        };

        let response = self
            .http
            .post(OPENAI_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|err| format!("Codex OAuth request failed: {err}"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("Failed to read Codex OAuth response: {err}"))?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            return Err(format!(
                "Codex OAuth request failed (status {}): {}",
                status.as_u16(),
                body
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|err| format!("Failed to parse Codex OAuth response: {err}"))
    }
}

#[derive(Serialize)]
struct TokenExchangeRequest {
    grant_type: String,
    client_id: String,
    code: String,
    redirect_uri: String,
    code_verifier: String,
    refresh_token: Option<String>,
    scope: Option<String>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct CodexTokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) id_token: String,
    pub(crate) expires_in: i64,
}
