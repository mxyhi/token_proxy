use serde::Serialize;
use serde_json::{Map, Value};

use crate::oauth_util::build_reqwest_client;

use super::store::KiroAccountStore;
use super::types::KiroAccountSummary;

const KIRO_USAGE_ENDPOINT: &str = "https://codewhisperer.us-east-1.amazonaws.com";
const KIRO_USAGE_TARGET: &str = "AmazonCodeWhispererService.GetUsageLimits";
const KIRO_USAGE_ORIGIN: &str = "AI_EDITOR";
const KIRO_USAGE_RESOURCE_TYPE: &str = "AGENTIC_REQUEST";
const KIRO_CONTENT_TYPE: &str = "application/x-amz-json-1.0";
const KIRO_ACCEPT: &str = "application/json";

#[derive(Clone, Serialize)]
pub struct KiroQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
    pub is_trial: bool,
}

#[derive(Clone, Serialize)]
pub struct KiroQuotaSummary {
    pub account_id: String,
    pub provider: String,
    pub plan_type: Option<String>,
    pub quotas: Vec<KiroQuotaItem>,
    pub error: Option<String>,
}

pub async fn fetch_quotas(
    store: &KiroAccountStore,
) -> Result<Vec<KiroQuotaSummary>, String> {
    let accounts = store.list_accounts().await?;
    let proxy_url = store.app_proxy_url().await;
    let mut results = Vec::with_capacity(accounts.len());
    for account in accounts {
        match fetch_account_quota(store, &account, proxy_url.as_deref()).await {
            Ok(summary) => results.push(summary),
            Err(err) => results.push(KiroQuotaSummary {
                account_id: account.account_id.clone(),
                provider: account.provider.clone(),
                plan_type: None,
                quotas: Vec::new(),
                error: Some(err),
            }),
        }
    }
    Ok(results)
}

async fn fetch_account_quota(
    store: &KiroAccountStore,
    account: &KiroAccountSummary,
    proxy_url: Option<&str>,
) -> Result<KiroQuotaSummary, String> {
    let record = store.get_account_record(&account.account_id).await?;
    let profile_arn = record
        .profile_arn
        .as_deref()
        .ok_or_else(|| "Missing Kiro profile ARN.".to_string())?;
    let response = request_usage_limits(&record.access_token, profile_arn, proxy_url).await?;
    Ok(map_usage_response(account, &response))
}

async fn request_usage_limits(
    access_token: &str,
    profile_arn: &str,
    proxy_url: Option<&str>,
) -> Result<Value, String> {
    let http = build_reqwest_client(proxy_url, std::time::Duration::from_secs(30))
        .map_err(|err| format!("Failed to build Kiro usage client: {err}"))?;
    let payload = serde_json::json!({
        "origin": KIRO_USAGE_ORIGIN,
        "profileArn": profile_arn,
        "resourceType": KIRO_USAGE_RESOURCE_TYPE,
    });
    let response = http
        .post(KIRO_USAGE_ENDPOINT)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", KIRO_CONTENT_TYPE)
        .header("x-amz-target", KIRO_USAGE_TARGET)
        .header("Accept", KIRO_ACCEPT)
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("Kiro usage request failed: {err}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("Failed to read Kiro usage response: {err}"))?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        return Err(format!(
            "Kiro usage request failed (status {}): {}",
            status.as_u16(),
            body
        ));
    }
    serde_json::from_slice(&bytes).map_err(|err| format!("Invalid Kiro usage response: {err}"))
}

fn map_usage_response(account: &KiroAccountSummary, value: &Value) -> KiroQuotaSummary {
    let plan_type = value
        .get("subscriptionInfo")
        .and_then(Value::as_object)
        .and_then(|info| get_string(info, "subscriptionTitle"));
    let reset_at = extract_reset_at(value);
    let quotas = value
        .get("usageBreakdownList")
        .and_then(Value::as_array)
        .map(|list| build_quota_items(list, reset_at.as_deref()))
        .unwrap_or_default();

    KiroQuotaSummary {
        account_id: account.account_id.clone(),
        provider: account.provider.clone(),
        plan_type,
        quotas,
        error: None,
    }
}

fn build_quota_items(items: &[Value], reset_at: Option<&str>) -> Vec<KiroQuotaItem> {
    let mut quotas = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let display_name = get_string(obj, "displayName")
            .or_else(|| get_string(obj, "resourceType"))
            .unwrap_or_else(|| "Usage".to_string());
        let (base_used, base_limit) = extract_usage_values(obj);
        let trial_info = obj.get("freeTrialInfo").and_then(Value::as_object);
        let trial_active = trial_info
            .and_then(|info| get_string(info, "freeTrialStatus"))
            .map(|status| status.eq_ignore_ascii_case("ACTIVE"))
            .unwrap_or(false);

        if trial_active {
            if let Some(info) = trial_info {
                let (trial_used, trial_limit) = extract_usage_values(info);
                let trial_reset = get_string(info, "freeTrialExpiry");
                if let Some(item) = build_quota_item(
                    format!("Bonus {display_name}"),
                    trial_used,
                    trial_limit,
                    trial_reset,
                    true,
                ) {
                    quotas.push(item);
                }
            }
        }

        let base_name = if trial_active {
            format!("{display_name} (Base)")
        } else {
            display_name
        };
        let base_reset = reset_at.map(|val| val.to_string());
        if let Some(item) = build_quota_item(base_name, base_used, base_limit, base_reset, false) {
            quotas.push(item);
        }
    }
    quotas
}

fn build_quota_item(
    name: String,
    used: Option<f64>,
    limit: Option<f64>,
    reset_at: Option<String>,
    is_trial: bool,
) -> Option<KiroQuotaItem> {
    if used.is_none() && limit.is_none() {
        return None;
    }
    let percentage = calc_percentage(used, limit);
    Some(KiroQuotaItem {
        name,
        percentage,
        used,
        limit,
        reset_at,
        is_trial,
    })
}

fn extract_usage_values(obj: &Map<String, Value>) -> (Option<f64>, Option<f64>) {
    let used = get_f64(obj, "currentUsageWithPrecision").or_else(|| get_f64(obj, "currentUsage"));
    let limit = get_f64(obj, "usageLimitWithPrecision").or_else(|| get_f64(obj, "usageLimit"));
    (used, limit)
}

fn get_string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(|val| val.to_string())
}

fn get_f64(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(as_f64)
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(num) => num.as_f64(),
        Value::String(val) => val.parse::<f64>().ok(),
        _ => None,
    }
}

fn calc_percentage(used: Option<f64>, limit: Option<f64>) -> f64 {
    let (Some(used), Some(limit)) = (used, limit) else {
        return 0.0;
    };
    if limit <= 0.0 {
        return 0.0;
    }
    let remaining = (limit - used) / limit * 100.0;
    remaining.clamp(0.0, 100.0)
}

fn extract_reset_at(value: &Value) -> Option<String> {
    let reset = value.get("nextDateReset")?;
    match reset {
        Value::String(val) => Some(val.to_string()),
        Value::Number(val) => Some(val.to_string()),
        _ => None,
    }
}
