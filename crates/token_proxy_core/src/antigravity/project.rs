use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use std::time::Duration;

use super::endpoints;
use crate::oauth_util::build_reqwest_client;

const LOAD_CODE_ASSIST_PATH: &str = "/v1internal:loadCodeAssist";
const ONBOARD_USER_PATH: &str = "/v1internal:onboardUser";

const API_USER_AGENT: &str = "google-api-nodejs-client/9.15.1";
const API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";
const CLIENT_METADATA: &str =
    r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#;

const MAX_ONBOARD_ATTEMPTS: usize = 5;
const ONBOARD_POLL_DELAY_SECS: u64 = 2;

#[derive(Clone, Default)]
pub(crate) struct AntigravityProjectInfo {
    pub(crate) project_id: Option<String>,
    pub(crate) plan_type: Option<String>,
}

pub(crate) async fn load_code_assist(
    access_token: &str,
    proxy_url: Option<&str>,
) -> Result<AntigravityProjectInfo, String> {
    let client = build_reqwest_client(proxy_url, Duration::from_secs(20))?;
    load_code_assist_with_client(&client, access_token).await
}

pub(crate) async fn load_code_assist_with_client(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<AntigravityProjectInfo, String> {
    let payload = serde_json::json!({
        "metadata": {
            "ideType": "ANTIGRAVITY",
            "platform": "PLATFORM_UNSPECIFIED",
            "pluginType": "GEMINI"
        }
    });
    let mut last_error: Option<String> = None;
    for base in endpoints::BASE_URLS {
        let url = format!("{}{}", base, LOAD_CODE_ASSIST_PATH);
        let response = client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(USER_AGENT, API_USER_AGENT)
            .header("X-Goog-Api-Client", API_CLIENT)
            .header("Client-Metadata", CLIENT_METADATA)
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                last_error = Some(format!("loadCodeAssist failed: {err}"));
                continue;
            }
        };
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let message = format!("loadCodeAssist failed: {status} {body}");
            if should_retry_status(status) {
                last_error = Some(message);
                continue;
            }
            return Err(message);
        }
        let value: Value = response
            .json()
            .await
            .map_err(|err| format!("loadCodeAssist parse failed: {err}"))?;
        return Ok(AntigravityProjectInfo {
            project_id: extract_project_id(&value),
            plan_type: extract_plan_type(&value),
        });
    }
    Err(last_error.unwrap_or_else(|| "loadCodeAssist failed.".to_string()))
}

pub(crate) async fn onboard_user(
    access_token: &str,
    proxy_url: Option<&str>,
    tier_id: &str,
) -> Result<Option<String>, String> {
    let client = build_reqwest_client(proxy_url, Duration::from_secs(30))?;
    onboard_user_with_client(&client, access_token, tier_id).await
}

pub(crate) async fn onboard_user_with_client(
    client: &reqwest::Client,
    access_token: &str,
    tier_id: &str,
) -> Result<Option<String>, String> {
    if tier_id.trim().is_empty() {
        return Ok(None);
    }
    let payload = serde_json::json!({
        "tierId": tier_id,
        "metadata": {
            "ideType": "ANTIGRAVITY",
            "platform": "PLATFORM_UNSPECIFIED",
            "pluginType": "GEMINI"
        }
    });
    for _ in 0..MAX_ONBOARD_ATTEMPTS {
        let mut last_error: Option<String> = None;
        for base in endpoints::BASE_URLS {
            let url = format!("{}{}", base, ONBOARD_USER_PATH);
            let response = client
                .post(url)
                .header(AUTHORIZATION, format!("Bearer {access_token}"))
                .header(USER_AGENT, API_USER_AGENT)
                .header("X-Goog-Api-Client", API_CLIENT)
                .header("Client-Metadata", CLIENT_METADATA)
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("onboardUser failed: {err}"));
                    continue;
                }
            };
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let message = format!("onboardUser failed: {status} {body}");
                if should_retry_status(status) {
                    last_error = Some(message);
                    continue;
                }
                return Err(message);
            }
            let value: Value = response
                .json()
                .await
                .map_err(|err| format!("onboardUser parse failed: {err}"))?;
            if value.get("done").and_then(Value::as_bool).unwrap_or(false) {
                return Ok(extract_onboard_project_id(&value));
            }
        }
        if let Some(message) = last_error {
            tracing::warn!(error = %message, "antigravity onboardUser retrying");
        }
        tokio::time::sleep(Duration::from_secs(ONBOARD_POLL_DELAY_SECS)).await;
    }
    Ok(None)
}

pub(crate) fn extract_project_id(value: &Value) -> Option<String> {
    if let Some(project) = value.get("cloudaicompanionProject") {
        if let Some(text) = project.as_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if let Some(obj) = project.as_object() {
            if let Some(text) = obj.get("id").and_then(Value::as_str) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub(crate) fn extract_plan_type(value: &Value) -> Option<String> {
    let tiers = value.get("allowedTiers")?.as_array()?;
    for tier in tiers {
        let obj = tier.as_object()?;
        let is_default = obj
            .get("isDefault")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !is_default {
            continue;
        }
        if let Some(id) = obj.get("id").and_then(Value::as_str) {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_onboard_project_id(value: &Value) -> Option<String> {
    let response = value.get("response")?;
    match response.get("cloudaicompanionProject") {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(Value::Object(obj)) => obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "project.test.rs"]
mod tests;
