use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::form_urlencoded;

use crate::oauth_util::build_reqwest_client;

use super::types::KiroTokenRecord;
use super::util::{expires_at_from_seconds, now_rfc3339};

const SSO_OIDC_ENDPOINT: &str = "https://oidc.us-east-1.amazonaws.com";
const BUILDER_ID_START_URL: &str = "https://view.awsapps.com/start";
const KIRO_USER_AGENT: &str = "KiroIDE";
const DEFAULT_IDC_REGION: &str = "us-east-1";
const IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";
const IDC_USER_AGENT: &str = "node";
const CODEWHISPERER_ENDPOINT: &str = "https://codewhisperer.us-east-1.amazonaws.com";
const CODEWHISPERER_CONTENT_TYPE: &str = "application/x-amz-json-1.0";
const CODEWHISPERER_ACCEPT: &str = "application/json";
const CW_TARGET_LIST_PROFILES: &str = "AmazonCodeWhispererService.ListProfiles";
const CW_TARGET_LIST_CUSTOMIZATIONS: &str = "AmazonCodeWhispererService.ListAvailableCustomizations";
const DEFAULT_SCOPES: [&str; 5] = [
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
    "codewhisperer:transformations",
    "codewhisperer:taskassist",
];
const AUTH_CODE_SCOPES: [&str; 3] = [
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
];

#[derive(Clone)]
pub(crate) struct SsoOidcClient {
    http: Client,
}

impl SsoOidcClient {
    pub(crate) fn new(proxy_url: Option<&str>) -> Result<Self, String> {
        let http = build_reqwest_client(proxy_url, std::time::Duration::from_secs(30))
            .map_err(|err| format!("Failed to build OIDC client: {err}"))?;
        Ok(Self { http })
    }

    pub(crate) async fn register_client(&self) -> Result<RegisterClientResponse, String> {
        let payload = RegisterClientRequest {
            client_name: "Kiro IDE".to_string(),
            client_type: "public".to_string(),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            grant_types: vec![
                "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                "refresh_token".to_string(),
            ],
            redirect_uris: None,
            issuer_url: None,
        };
        self.post_json("/client/register", &payload).await
    }

    pub(crate) async fn register_client_for_auth_code(
        &self,
        redirect_uri: &str,
    ) -> Result<RegisterClientResponse, String> {
        let payload = RegisterClientRequest {
            client_name: "Kiro IDE".to_string(),
            client_type: "public".to_string(),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            grant_types: vec!["authorization_code".to_string(), "refresh_token".to_string()],
            redirect_uris: Some(vec![redirect_uri.to_string()]),
            issuer_url: Some(BUILDER_ID_START_URL.to_string()),
        };
        self.post_json("/client/register", &payload).await
    }

    pub(crate) async fn start_device_authorization(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<StartDeviceAuthResponse, String> {
        let payload = StartDeviceAuthRequest {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            start_url: BUILDER_ID_START_URL.to_string(),
        };
        self.post_json("/device_authorization", &payload).await
    }

    pub(crate) async fn create_token_device_code(
        &self,
        client_id: &str,
        client_secret: &str,
        device_code: &str,
    ) -> Result<CreateTokenResponse, TokenPollError> {
        let payload = CreateTokenDeviceCodeRequest {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            device_code: device_code.to_string(),
            grant_type: "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        };
        self.post_json_result("/token", &payload).await
    }

    pub(crate) async fn create_token_auth_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<CreateTokenResponse, String> {
        let payload = CreateTokenAuthCodeRequest {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            code: code.to_string(),
            code_verifier: code_verifier.to_string(),
            redirect_uri: redirect_uri.to_string(),
            grant_type: "authorization_code".to_string(),
        };
        self.post_json("/token", &payload).await
    }

    pub(crate) async fn refresh_token_with_region(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
        region: &str,
    ) -> Result<CreateTokenResponse, String> {
        let payload = RefreshTokenRequest {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            refresh_token: refresh_token.to_string(),
            grant_type: "refresh_token".to_string(),
        };
        let endpoint = oidc_endpoint_for_region(region);
        let url = format!("{endpoint}/token");
        let host = format!("oidc.{region}.amazonaws.com");
        let response = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Host", host)
            .header("Connection", "keep-alive")
            .header("x-amz-user-agent", IDC_AMZ_USER_AGENT)
            .header("Accept", "*/*")
            .header("Accept-Language", "*")
            .header("sec-fetch-mode", "cors")
            .header("User-Agent", IDC_USER_AGENT)
            .header("Accept-Encoding", "br, gzip, deflate")
            .json(&payload)
            .send()
            .await
            .map_err(|err| format!("IDC refresh request failed: {err}"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("Failed to read IDC refresh response: {err}"))?;
        if !status.is_success() {
            return Err(format!(
                "IDC token refresh failed (status {})",
                status.as_u16()
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|err| format!("Failed to parse IDC refresh response: {err}"))
    }

    pub(crate) async fn fetch_profile_arn(&self, access_token: &str) -> Option<String> {
        if let Some(arn) = self.try_list_profiles(access_token).await {
            return Some(arn);
        }
        self.try_list_customizations(access_token).await
    }

    async fn try_list_profiles(&self, access_token: &str) -> Option<String> {
        let payload = json!({"origin": "AI_EDITOR"});
        let value = self
            .post_codewhisperer(access_token, CW_TARGET_LIST_PROFILES, &payload)
            .await
            .ok()?;
        parse_profile_arn_from_profiles(&value)
    }

    async fn try_list_customizations(&self, access_token: &str) -> Option<String> {
        let payload = json!({"origin": "AI_EDITOR"});
        let value = self
            .post_codewhisperer(access_token, CW_TARGET_LIST_CUSTOMIZATIONS, &payload)
            .await
            .ok()?;
        parse_profile_arn_from_customizations(&value)
    }

    async fn post_codewhisperer(
        &self,
        access_token: &str,
        target: &str,
        payload: &Value,
    ) -> Result<Value, String> {
        let response = self
            .http
            .post(CODEWHISPERER_ENDPOINT)
            .header("Content-Type", CODEWHISPERER_CONTENT_TYPE)
            .header("x-amz-target", target)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", CODEWHISPERER_ACCEPT)
            .json(payload)
            .send()
            .await
            .map_err(|err| format!("CodeWhisperer request failed: {err}"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("Failed to read CodeWhisperer response: {err}"))?;
        if !status.is_success() {
            return Err(format!(
                "CodeWhisperer request failed (status {})",
                status.as_u16()
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|err| format!("Failed to parse CodeWhisperer response: {err}"))
    }

    pub(crate) async fn refresh_builder_token(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Result<KiroTokenRecord, String> {
        let payload = RefreshTokenRequest {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            refresh_token: refresh_token.to_string(),
            grant_type: "refresh_token".to_string(),
        };
        let response: CreateTokenResponse = self.post_json("/token", &payload).await?;
        Ok(KiroTokenRecord {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            profile_arn: None,
            expires_at: expires_at_from_seconds(response.expires_in),
            auth_method: "builder-id".to_string(),
            provider: "AWS".to_string(),
            client_id: Some(client_id.to_string()),
            client_secret: Some(client_secret.to_string()),
            email: None,
            last_refresh: Some(now_rfc3339()),
            start_url: None,
            region: None,
        })
    }

    async fn post_json<TReq: Serialize, TRes: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        payload: &TReq,
    ) -> Result<TRes, String> {
        self.post_json_result(path, payload).await.map_err(|err| match err {
            TokenPollError::Pending => "Authorization pending.".to_string(),
            TokenPollError::SlowDown => "Slow down.".to_string(),
            TokenPollError::Other(message) => message,
        })
    }

    async fn post_json_result<TReq: Serialize, TRes: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        payload: &TReq,
    ) -> Result<TRes, TokenPollError> {
        let url = format!("{SSO_OIDC_ENDPOINT}{path}");
        let response = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("User-Agent", KIRO_USER_AGENT)
            .json(payload)
            .send()
            .await
            .map_err(|err| TokenPollError::Other(format!("OIDC request failed: {err}")))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| TokenPollError::Other(format!("Failed to read OIDC response: {err}")))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map_err(|err| TokenPollError::Other(format!("Failed to parse OIDC response: {err}")));
        }
        if status == reqwest::StatusCode::BAD_REQUEST {
            if let Ok(error) = serde_json::from_slice::<OidcError>(&bytes) {
                return match error.error.as_str() {
                    "authorization_pending" => Err(TokenPollError::Pending),
                    "slow_down" => Err(TokenPollError::SlowDown),
                    _ => Err(TokenPollError::Other(format!(
                        "OIDC error: {}",
                        error.error
                    ))),
                };
            }
        }
        Err(TokenPollError::Other(format!(
            "OIDC request failed (status {})",
            status.as_u16()
        )))
    }
}

pub(crate) async fn refresh_builder_token(
    record: &KiroTokenRecord,
    proxy_url: Option<&str>,
) -> Result<KiroTokenRecord, String> {
    let client_id = record
        .client_id
        .as_deref()
        .ok_or_else(|| "Missing OIDC client_id.".to_string())?;
    let client_secret = record
        .client_secret
        .as_deref()
        .ok_or_else(|| "Missing OIDC client_secret.".to_string())?;
    let client = SsoOidcClient::new(proxy_url)?;
    client
        .refresh_builder_token(client_id, client_secret, &record.refresh_token)
        .await
}

pub(crate) async fn refresh_idc_token(
    record: &KiroTokenRecord,
    proxy_url: Option<&str>,
) -> Result<KiroTokenRecord, String> {
    let client_id = record
        .client_id
        .as_deref()
        .ok_or_else(|| "Missing OIDC client_id.".to_string())?;
    let client_secret = record
        .client_secret
        .as_deref()
        .ok_or_else(|| "Missing OIDC client_secret.".to_string())?;
    let region = record
        .region
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_IDC_REGION);
    let client = SsoOidcClient::new(proxy_url)?;
    let response = client
        .refresh_token_with_region(
            client_id,
            client_secret,
            &record.refresh_token,
            region,
        )
        .await?;
    Ok(KiroTokenRecord {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        profile_arn: record.profile_arn.clone(),
        expires_at: expires_at_from_seconds(response.expires_in),
        auth_method: "idc".to_string(),
        provider: "AWS".to_string(),
        client_id: Some(client_id.to_string()),
        client_secret: Some(client_secret.to_string()),
        email: record.email.clone(),
        last_refresh: Some(now_rfc3339()),
        start_url: record.start_url.clone(),
        region: Some(region.to_string()),
    })
}

pub(crate) fn build_auth_code_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    let scopes = AUTH_CODE_SCOPES.join(",");
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scopes", &scopes)
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    format!("{SSO_OIDC_ENDPOINT}/authorize?{query}")
}

fn oidc_endpoint_for_region(region: &str) -> String {
    let trimmed = region.trim();
    let region = if trimmed.is_empty() {
        DEFAULT_IDC_REGION
    } else {
        trimmed
    };
    format!("https://oidc.{region}.amazonaws.com")
}

fn parse_profile_arn_from_profiles(value: &Value) -> Option<String> {
    value
        .get("profileArn")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .or_else(|| {
            value
                .get("profiles")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("arn"))
                .and_then(Value::as_str)
                .map(|value| value.to_string())
        })
}

fn parse_profile_arn_from_customizations(value: &Value) -> Option<String> {
    value
        .get("profileArn")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .or_else(|| {
            value
                .get("customizations")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("arn"))
                .and_then(Value::as_str)
                .map(|value| value.to_string())
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterClientRequest {
    client_name: String,
    client_type: String,
    scopes: Vec<String>,
    grant_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect_uris: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issuer_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartDeviceAuthRequest {
    client_id: String,
    client_secret: String,
    start_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateTokenDeviceCodeRequest {
    client_id: String,
    client_secret: String,
    device_code: String,
    grant_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateTokenAuthCodeRequest {
    client_id: String,
    client_secret: String,
    code: String,
    code_verifier: String,
    redirect_uri: String,
    grant_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshTokenRequest {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    grant_type: String,
}

#[derive(Debug)]
pub(crate) enum TokenPollError {
    Pending,
    SlowDown,
    Other(String),
}


#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OidcError {
    error: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegisterClientResponse {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartDeviceAuthResponse {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    pub(crate) verification_uri: String,
    pub(crate) verification_uri_complete: String,
    pub(crate) expires_in: i64,
    pub(crate) interval: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_in: i64,
}
