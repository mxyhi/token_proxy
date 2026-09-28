use reqwest::Client;
use serde::{Deserialize, Serialize};

use token_proxy_account_store::oauth_util::build_reqwest_client;

use super::error::{format_oauth_status_error, oauth_status_is_retryable};

const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_MOBILE_CLIENT_ID: &str = "app_LlGpXReQgckcGGUo2JrYvtJK";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexRefreshTokenClient {
    Codex,
    Mobile,
}

impl CodexRefreshTokenClient {
    pub fn from_client_id(client_id: &str) -> Option<Self> {
        match client_id.trim() {
            OPENAI_CLIENT_ID => Some(Self::Codex),
            OPENAI_MOBILE_CLIENT_ID => Some(Self::Mobile),
            _ => None,
        }
    }

    pub fn client_id(self) -> &'static str {
        match self {
            Self::Codex => OPENAI_CLIENT_ID,
            Self::Mobile => OPENAI_MOBILE_CLIENT_ID,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Mobile => "mobile",
        }
    }
}

#[derive(Clone)]
pub(crate) struct CodexOAuthClient {
    http: Client,
    token_url: String,
}

impl CodexOAuthClient {
    pub(crate) fn new(proxy_url: Option<&str>) -> Result<Self, String> {
        Self::new_with_token_url(proxy_url, OPENAI_TOKEN_URL)
    }

    pub(crate) fn new_with_token_url(
        proxy_url: Option<&str>,
        token_url: &str,
    ) -> Result<Self, String> {
        let http = build_reqwest_client(proxy_url, std::time::Duration::from_secs(30))
            .map_err(|err| format!("Failed to build Codex OAuth client: {err}"))?;
        Ok(Self {
            http,
            token_url: token_url.to_string(),
        })
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

    pub(crate) async fn refresh_token_with_client(
        &self,
        refresh_token: &str,
        client: CodexRefreshTokenClient,
    ) -> Result<CodexTokenResponse, String> {
        let payload = TokenExchangeRequest {
            grant_type: "refresh_token".to_string(),
            client_id: client.client_id().to_string(),
            code: String::new(),
            redirect_uri: String::new(),
            code_verifier: String::new(),
            refresh_token: Some(refresh_token.to_string()),
            scope: Some("openid profile email".to_string()),
        };
        tracing::debug!(
            client = client.as_str(),
            "codex refresh token exchange start"
        );
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

        let attempts = if payload.grant_type == "refresh_token" {
            3
        } else {
            1
        };
        // A refresh round, including retry delays, stays inside the single-flight wait budget.
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            for attempt in 0..attempts {
                match self.post_form_once(&body).await {
                    Ok(response) => return Ok(response),
                    Err((message, retryable)) => {
                        if !retryable || attempt + 1 == attempts {
                            return Err(message);
                        }
                        let delay_seconds = attempt + 1;
                        tracing::warn!(
                            attempt = attempt + 1,
                            delay_seconds,
                            "codex oauth transient failure; retry deferred"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(delay_seconds)).await;
                    }
                }
            }
            unreachable!("at least one OAuth attempt")
        })
        .await
        .map_err(|_| "Codex OAuth refresh round timed out after 30 seconds.".to_string())?
    }

    async fn post_form_once(&self, body: &str) -> Result<CodexTokenResponse, (String, bool)> {
        let response = self
            .http
            .post(&self.token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|error| {
                (
                    format!("Codex OAuth request failed: {error}"),
                    error.is_connect() || error.is_timeout(),
                )
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            (
                format!("Failed to read Codex OAuth response: {error}"),
                false,
            )
        })?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            return Err((
                format_oauth_status_error(status.as_u16(), &body),
                oauth_status_is_retryable(status.as_u16(), &body),
            ));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            (
                format!("Failed to parse Codex OAuth response: {error}"),
                false,
            )
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_token_client_ids_match_openai_clients() {
        assert_eq!(
            CodexRefreshTokenClient::Codex.client_id(),
            "app_EMoamEEZ73f0CkXaXp7hrann"
        );
        assert_eq!(
            CodexRefreshTokenClient::Mobile.client_id(),
            "app_LlGpXReQgckcGGUo2JrYvtJK"
        );
    }

    #[test]
    fn refresh_token_client_resolves_persisted_client_ids() {
        assert_eq!(
            CodexRefreshTokenClient::from_client_id("app_EMoamEEZ73f0CkXaXp7hrann"),
            Some(CodexRefreshTokenClient::Codex)
        );
        assert_eq!(
            CodexRefreshTokenClient::from_client_id("app_LlGpXReQgckcGGUo2JrYvtJK"),
            Some(CodexRefreshTokenClient::Mobile)
        );
        assert_eq!(CodexRefreshTokenClient::from_client_id("unknown"), None);
    }
}
