use serde_json::Value;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};

use crate::oauth_util::build_reqwest_client;

use super::endpoints;
use super::project;
use super::store::AntigravityAccountStore;
use super::types::{AntigravityAccountSummary, AntigravityQuotaItem, AntigravityQuotaSummary};

const FETCH_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";

pub async fn fetch_quotas(
    store: &AntigravityAccountStore,
) -> Result<Vec<AntigravityQuotaSummary>, String> {
    let accounts = store.list_accounts().await?;
    let proxy_url = store.app_proxy_url().await;
    let mut results = Vec::with_capacity(accounts.len());
    for account in accounts {
        match fetch_account_quota(store, &account, proxy_url.as_deref()).await {
            Ok(summary) => results.push(summary),
            Err(err) => results.push(AntigravityQuotaSummary {
                account_id: account.account_id.clone(),
                plan_type: None,
                quotas: Vec::new(),
                error: Some(err),
            }),
        }
    }
    Ok(results)
}

async fn fetch_account_quota(
    store: &AntigravityAccountStore,
    account: &AntigravityAccountSummary,
    proxy_url: Option<&str>,
) -> Result<AntigravityQuotaSummary, String> {
    let record = store.get_account_record(&account.account_id).await?;
    let client = build_reqwest_client(proxy_url, Duration::from_secs(20))?;
    let mut project_id = record.project_id.clone();
    let mut plan_type: Option<String> = None;
    let mut load_error: Option<String> = None;
    match project::load_code_assist_with_client(&client, &record.access_token).await {
        Ok(info) => {
            plan_type = info.plan_type.clone();
            if let Some(value) = info.project_id.clone() {
                project_id = Some(value.clone());
                let _ = store.update_project_id(&account.account_id, value).await;
            } else if let Some(tier_id) = info.plan_type.as_deref() {
                match project::onboard_user_with_client(&client, &record.access_token, tier_id)
                    .await
                {
                    Ok(Some(value)) => {
                        project_id = Some(value.clone());
                        let _ = store.update_project_id(&account.account_id, value).await;
                    }
                    Ok(None) => {}
                    Err(err) => load_error = Some(err),
                }
            }
        }
        Err(err) => load_error = Some(err),
    }
    let quotas =
        match fetch_available_models(&client, &record.access_token, project_id.as_deref()).await {
            Ok(quotas) => quotas,
            Err(err) => {
                if let Some(load_error) = load_error {
                    return Err(format!("{load_error}; {err}"));
                }
                return Err(err);
            }
        };
    if let Some(load_error) = load_error {
        tracing::warn!(error = %load_error, "antigravity loadCodeAssist failed for quota");
    }
    Ok(AntigravityQuotaSummary {
        account_id: account.account_id.clone(),
        plan_type,
        quotas,
        error: None,
    })
}

async fn fetch_available_models(
    client: &reqwest::Client,
    access_token: &str,
    project_id: Option<&str>,
) -> Result<Vec<AntigravityQuotaItem>, String> {
    let user_agent = endpoints::default_user_agent();
    let payload = if let Some(project_id) = project_id.filter(|value| !value.trim().is_empty()) {
        serde_json::json!({ "project": project_id })
    } else {
        serde_json::json!({})
    };
    let mut last_error: Option<String> = None;
    for base in endpoints::BASE_URLS {
        let url = format!("{}{}", base, FETCH_MODELS_PATH);
        let response = client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(USER_AGENT, user_agent.as_str())
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                last_error = Some(format!("fetchAvailableModels failed: {err}"));
                continue;
            }
        };
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let message = format!("fetchAvailableModels failed: {status} {body}");
            if should_retry_status(status) {
                last_error = Some(message);
                continue;
            }
            return Err(message);
        }
        let value: Value = response
            .json()
            .await
            .map_err(|err| format!("fetchAvailableModels parse failed: {err}"))?;
        return Ok(extract_quota_items(&value));
    }
    Err(last_error.unwrap_or_else(|| "fetchAvailableModels failed.".to_string()))
}

fn extract_quota_items(value: &Value) -> Vec<AntigravityQuotaItem> {
    let mut items = Vec::new();
    let Some(models) = value.get("models") else {
        return items;
    };
    match models {
        Value::Array(list) => {
            for model in list {
                let Some(name) = model.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(item) = build_quota_item(name, model.get("quotaInfo")) {
                    items.push(item);
                }
            }
        }
        Value::Object(map) => {
            for (name, info) in map {
                if let Some(item) = build_quota_item(name, info.get("quotaInfo")) {
                    items.push(item);
                }
            }
        }
        _ => {}
    }
    items
}

fn build_quota_item(name: &str, quota_info: Option<&Value>) -> Option<AntigravityQuotaItem> {
    let name_lower = name.to_lowercase();
    if !name_lower.contains("gemini") && !name_lower.contains("claude") {
        return None;
    }
    let quota = quota_info?.as_object()?;
    let remaining_fraction = quota
        .get("remainingFraction")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let percentage = (remaining_fraction * 100.0).clamp(0.0, 100.0);
    let reset_at = quota
        .get("resetTime")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    Some(AntigravityQuotaItem {
        name: name.to_string(),
        percentage,
        reset_at,
    })
}

fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "quota.test.rs"]
mod tests;
