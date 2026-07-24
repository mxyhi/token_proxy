use axum::http::HeaderMap;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use time::OffsetDateTime;

use token_proxy_account_store::oauth_util::{build_reqwest_client_no_redirect, now_rfc3339};

use super::store::XaiAccountStore;
use super::types::{XaiQuotaCache, XaiQuotaItem, XaiQuotaSummary};
use super::{
    CLI_BILLING_USER_AGENT, CLI_CLIENT_VERSION, CLI_CLIENT_VERSION_HEADER, CLI_TOKEN_AUTH_HEADER,
    CLI_TOKEN_AUTH_VALUE,
};

const BILLING_WEEKLY_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const BILLING_MONTHLY_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const ACTIVE_PROBE_URL: &str = "https://cli-chat-proxy.grok.com/v1/responses";
const ACTIVE_PROBE_MODEL: &str = "grok-4.5";

pub async fn fetch_quotas(store: &XaiAccountStore) -> Result<Vec<XaiQuotaSummary>, String> {
    let accounts = store.list_accounts().await?;
    let mut summaries = Vec::with_capacity(accounts.len());
    for account in accounts {
        let cache = match store.refresh_quota_cache_guarded(&account.account_id).await {
            Ok(cache) => cache,
            Err(error) => XaiQuotaCache {
                error: Some(error),
                checked_at: Some(now_rfc3339()),
                ..XaiQuotaCache::default()
            },
        };
        summaries.push(XaiQuotaSummary {
            account_id: account.account_id,
            plan_type: cache.plan_type,
            quotas: cache.quotas,
            error: cache.error,
        });
    }
    Ok(summaries)
}

pub(crate) async fn refresh_quota_cache(
    store: &XaiAccountStore,
    account_id: &str,
) -> Result<XaiQuotaCache, String> {
    let mut record = store.get_account_record(account_id).await?;
    let proxy_url = store.effective_proxy_url(None).await;
    let http = build_reqwest_client_no_redirect(proxy_url.as_deref(), Duration::from_secs(30))
        .map_err(|error| format!("Failed to build xAI quota client: {error}"))?;

    let mut billing = fetch_billing_windows(&http, &record.access_token).await;
    if billing.has_unauthorized() && !record.refresh_token.trim().is_empty() {
        tracing::warn!(account_id, "xai billing request requires token refresh");
        store.refresh_account(account_id).await?;
        record = store.load_account(account_id).await?;
        billing = fetch_billing_windows(&http, &record.access_token).await;
    }

    let mut cache = build_billing_cache(&billing);
    if !billing.has_authoritative_quota() {
        // Free 账户的 billing 不给百分比；仅手动刷新时发送一次轻量 Responses 探针。
        match active_quota_probe(&http, &record.access_token).await {
            Ok((headers, status)) => {
                if let Some(observed) = observe_quota_headers(&cache, &headers, status) {
                    cache = observed;
                }
                if status >= 400 && cache.error.is_none() {
                    cache.error = Some(format!("xAI quota probe returned status {status}."));
                }
            }
            Err(error) if cache.quotas.is_empty() => cache.error = Some(error),
            Err(error) => {
                tracing::warn!(account_id, error = %error, "xai active quota probe failed")
            }
        }
    }
    cache.checked_at = Some(now_rfc3339());
    let persisted = store.persist_quota_cache(account_id, cache).await?;
    tracing::info!(
        account_id,
        quota_items = persisted.quota.quotas.len(),
        plan = persisted.quota.plan_type.as_deref().unwrap_or("unknown"),
        "xai quota refresh completed"
    );
    Ok(persisted.quota)
}

/// 将响应头转换为统一剩余额度条，并保留已有 billing 条目。
pub(crate) fn observe_quota_headers(
    previous: &XaiQuotaCache,
    headers: &HeaderMap,
    status: u16,
) -> Option<XaiQuotaCache> {
    let requests = quota_item_from_headers(headers, "requests", "xai-requests");
    let tokens = quota_item_from_headers(headers, "tokens", "xai-tokens");
    let plan_type = first_header(headers, &["xai-subscription-tier", "x-subscription-tier"]);
    if requests.is_none() && tokens.is_none() && plan_type.is_none() {
        return None;
    }
    let mut items = previous.quotas.clone();
    if let Some(item) = requests {
        upsert_quota_item(&mut items, item);
    }
    if let Some(item) = tokens {
        upsert_quota_item(&mut items, item);
    }
    Some(XaiQuotaCache {
        plan_type: plan_type.or_else(|| previous.plan_type.clone()),
        quotas: items,
        error: (status >= 400).then(|| format!("xAI upstream returned status {status}.")),
        checked_at: Some(now_rfc3339()),
    })
}

async fn fetch_billing_windows(http: &Client, access_token: &str) -> BillingWindows {
    let (weekly, monthly) = tokio::join!(
        request_billing(http, BILLING_WEEKLY_URL, access_token),
        request_billing(http, BILLING_MONTHLY_URL, access_token)
    );
    BillingWindows { weekly, monthly }
}

async fn request_billing(
    http: &Client,
    url: &str,
    access_token: &str,
) -> Result<BillingResponse, String> {
    let response = apply_cli_headers(http.get(url), access_token, "application/json")
        .send()
        .await
        .map_err(|error| format!("xAI billing request failed: {error}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Failed to read xAI billing response: {error}"))?;
    if !(200..300).contains(&status) {
        return Ok(BillingResponse {
            status,
            value: None,
        });
    }
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| format!("Invalid xAI billing response: {error}"))?;
    Ok(BillingResponse {
        status,
        value: Some(value),
    })
}

async fn active_quota_probe(http: &Client, access_token: &str) -> Result<(HeaderMap, u16), String> {
    let response = apply_cli_headers(
        http.post(ACTIVE_PROBE_URL),
        access_token,
        "application/json, text/event-stream",
    )
    .json(&json!({
        "model": ACTIVE_PROBE_MODEL,
        "input": "hi",
        "stream": true,
        "store": false
    }))
    .send()
    .await
    .map_err(|error| format!("xAI quota probe failed: {error}"))?;
    Ok((response.headers().clone(), response.status().as_u16()))
}

fn apply_cli_headers(
    request: reqwest::RequestBuilder,
    access_token: &str,
    accept: &str,
) -> reqwest::RequestBuilder {
    request
        .bearer_auth(access_token)
        .header("Accept", accept)
        .header("Content-Type", "application/json")
        .header(CLI_TOKEN_AUTH_HEADER, CLI_TOKEN_AUTH_VALUE)
        .header(CLI_CLIENT_VERSION_HEADER, CLI_CLIENT_VERSION)
        .header("User-Agent", CLI_BILLING_USER_AGENT)
}

fn build_billing_cache(windows: &BillingWindows) -> XaiQuotaCache {
    let mut quotas = Vec::new();
    let mut plan_type = None;
    let mut errors = Vec::new();

    match &windows.weekly {
        Ok(response) if (200..300).contains(&response.status) => {
            if let Some(config) = response.value.as_ref().and_then(billing_config) {
                if let Some(used_percent) = config.get("creditUsagePercent").and_then(value_number)
                {
                    upsert_quota_item(
                        &mut quotas,
                        XaiQuotaItem {
                            name: "xai-weekly".to_string(),
                            percentage: remaining_percentage(used_percent),
                            used: Some(used_percent),
                            limit: Some(100.0),
                            reset_at: period_end(config),
                        },
                    );
                }
                if let Some(products) = config.get("productUsage").and_then(Value::as_array) {
                    for product in products {
                        let Some(name) = product.get("product").and_then(Value::as_str) else {
                            continue;
                        };
                        let Some(used_percent) = product.get("usagePercent").and_then(value_number)
                        else {
                            continue;
                        };
                        upsert_quota_item(
                            &mut quotas,
                            XaiQuotaItem {
                                name: format!("xai-product-{}", normalize_quota_name(name)),
                                percentage: remaining_percentage(used_percent),
                                used: Some(used_percent),
                                limit: Some(100.0),
                                reset_at: period_end(config),
                            },
                        );
                    }
                }
            }
        }
        Ok(response) => errors.push(format!("weekly billing status {}", response.status)),
        Err(error) => errors.push(error.clone()),
    }

    match &windows.monthly {
        Ok(response) if (200..300).contains(&response.status) => {
            if let Some(config) = response.value.as_ref().and_then(billing_config) {
                let limit = config.get("monthlyLimit").and_then(value_number);
                let used = config.get("used").and_then(value_number);
                plan_type = plan_from_monthly_limit(limit);
                if let (Some(used), Some(limit)) = (used, limit.filter(|value| *value > 0.0)) {
                    upsert_quota_item(
                        &mut quotas,
                        XaiQuotaItem {
                            name: "xai-monthly".to_string(),
                            percentage: ((limit - used).max(0.0) / limit * 100.0).clamp(0.0, 100.0),
                            used: Some(used),
                            limit: Some(limit),
                            reset_at: config
                                .get("billingPeriodEnd")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        },
                    );
                }
            }
        }
        Ok(response) => errors.push(format!("monthly billing status {}", response.status)),
        Err(error) => errors.push(error.clone()),
    }

    let successful_billing = windows
        .weekly
        .as_ref()
        .is_ok_and(|response| (200..300).contains(&response.status))
        || windows
            .monthly
            .as_ref()
            .is_ok_and(|response| (200..300).contains(&response.status));
    if successful_billing && plan_type.is_none() && quotas.is_empty() {
        plan_type = Some("free".to_string());
    }

    XaiQuotaCache {
        plan_type,
        quotas,
        error: (!errors.is_empty() && !successful_billing).then(|| errors.join(" | ")),
        checked_at: Some(now_rfc3339()),
    }
}

fn quota_item_from_headers(
    headers: &HeaderMap,
    dimension: &str,
    name: &str,
) -> Option<XaiQuotaItem> {
    let limit = parse_header_number(headers, &format!("x-ratelimit-limit-{dimension}"));
    let remaining = parse_header_number(headers, &format!("x-ratelimit-remaining-{dimension}"));
    let reset_at = headers
        .get(format!("x-ratelimit-reset-{dimension}"))
        .and_then(|value| value.to_str().ok())
        .and_then(parse_reset_at);
    if limit.is_none() && remaining.is_none() && reset_at.is_none() {
        return None;
    }
    let percentage = match (remaining, limit) {
        (Some(remaining), Some(limit)) if limit > 0.0 => {
            (remaining / limit * 100.0).clamp(0.0, 100.0)
        }
        _ => 0.0,
    };
    Some(XaiQuotaItem {
        name: name.to_string(),
        percentage,
        used: match (remaining, limit) {
            (Some(remaining), Some(limit)) => Some((limit - remaining).max(0.0)),
            _ => None,
        },
        limit,
        reset_at,
    })
}

fn first_header(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn parse_header_number(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<f64>().ok())
}

fn parse_reset_at(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).is_ok() {
        return Some(raw.to_string());
    }
    let value = raw.parse::<i64>().ok()?;
    let seconds = if value > 1_000_000_000_000 {
        value / 1000
    } else {
        value
    };
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn billing_config(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.get("config").unwrap_or(value).as_object()
}

fn value_number(value: &Value) -> Option<f64> {
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    if let Some(text) = value.as_str() {
        return text.trim().parse().ok();
    }
    let object = value.as_object()?;
    ["amount", "value", "cents"]
        .iter()
        .find_map(|key| object.get(*key).and_then(value_number))
}

fn period_end(config: &serde_json::Map<String, Value>) -> Option<String> {
    config
        .get("currentPeriod")
        .and_then(Value::as_object)
        .and_then(|period| period.get("end"))
        .and_then(Value::as_str)
        .or_else(|| config.get("billingPeriodEnd").and_then(Value::as_str))
        .map(str::to_string)
}

fn plan_from_monthly_limit(limit: Option<f64>) -> Option<String> {
    let limit = limit?;
    Some(
        if limit >= 150_000.0 {
            "supergrok_heavy"
        } else if limit >= 15_000.0 {
            "supergrok"
        } else {
            "free"
        }
        .to_string(),
    )
}

fn remaining_percentage(used_percent: f64) -> f64 {
    (100.0 - used_percent).clamp(0.0, 100.0)
}

fn normalize_quota_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Billing 与响应头可能重复返回同一维度；按名称覆盖旧快照，但保留首次出现顺序。
fn upsert_quota_item(items: &mut Vec<XaiQuotaItem>, next: XaiQuotaItem) {
    if let Some(existing) = items.iter_mut().find(|item| item.name == next.name) {
        *existing = next;
    } else {
        items.push(next);
    }
}

/// 网络刷新只拥有本次返回的 quota 维度；保存前按名称并入最新快照，避免覆盖并发响应头。
pub(crate) fn merge_quota_cache(current: &XaiQuotaCache, incoming: XaiQuotaCache) -> XaiQuotaCache {
    let mut quotas = current.quotas.clone();
    for item in incoming.quotas {
        upsert_quota_item(&mut quotas, item);
    }
    XaiQuotaCache {
        plan_type: incoming.plan_type.or_else(|| current.plan_type.clone()),
        quotas,
        error: incoming.error,
        checked_at: incoming.checked_at.or_else(|| current.checked_at.clone()),
    }
}

struct BillingResponse {
    status: u16,
    value: Option<Value>,
}

struct BillingWindows {
    weekly: Result<BillingResponse, String>,
    monthly: Result<BillingResponse, String>,
}

impl BillingWindows {
    fn has_unauthorized(&self) -> bool {
        [&self.weekly, &self.monthly]
            .iter()
            .any(|result| result.as_ref().is_ok_and(|response| response.status == 401))
    }

    fn has_authoritative_quota(&self) -> bool {
        let cache = build_billing_cache(self);
        !cache.quotas.is_empty()
            || cache
                .plan_type
                .as_deref()
                .is_some_and(|plan| !plan.eq_ignore_ascii_case("free"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn passive_headers_merge_request_and_token_windows() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("75"),
        );
        headers.insert("x-ratelimit-limit-tokens", HeaderValue::from_static("1000"));
        headers.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("250"),
        );
        let cache = observe_quota_headers(&XaiQuotaCache::default(), &headers, 200).unwrap();
        assert_eq!(cache.quotas.len(), 2);
        assert_eq!(cache.quotas[0].percentage, 75.0);
        assert_eq!(cache.quotas[1].percentage, 25.0);
    }

    #[test]
    fn passive_headers_replace_same_name_and_preserve_other_items() {
        let previous = XaiQuotaCache {
            quotas: vec![
                XaiQuotaItem {
                    name: "xai-requests".to_string(),
                    percentage: 12.0,
                    used: Some(88.0),
                    limit: Some(100.0),
                    reset_at: None,
                },
                XaiQuotaItem {
                    name: "xai-monthly".to_string(),
                    percentage: 64.0,
                    used: Some(36.0),
                    limit: Some(100.0),
                    reset_at: None,
                },
            ],
            ..XaiQuotaCache::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("200"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("150"),
        );

        let cache = observe_quota_headers(&previous, &headers, 200).expect("quota headers");
        assert_eq!(cache.quotas.len(), 2);
        assert_eq!(cache.quotas[0].name, "xai-requests");
        assert_eq!(cache.quotas[0].percentage, 75.0);
        assert_eq!(cache.quotas[1].name, "xai-monthly");
    }

    #[test]
    fn billing_cache_maps_weekly_and_monthly_percentages() {
        let windows = BillingWindows {
            weekly: Ok(BillingResponse {
                status: 200,
                value: Some(json!({
                    "config": {
                        "creditUsagePercent": 20,
                        "currentPeriod": { "end": "2999-01-01T00:00:00Z" }
                    }
                })),
            }),
            monthly: Ok(BillingResponse {
                status: 200,
                value: Some(json!({
                    "config": { "monthlyLimit": 15000, "used": 3750 }
                })),
            }),
        };
        let cache = build_billing_cache(&windows);
        assert_eq!(cache.plan_type.as_deref(), Some("supergrok"));
        assert_eq!(cache.quotas[0].percentage, 80.0);
        assert_eq!(cache.quotas[1].percentage, 75.0);
    }

    #[test]
    fn billing_cache_deduplicates_product_names() {
        let windows = BillingWindows {
            weekly: Ok(BillingResponse {
                status: 200,
                value: Some(json!({
                    "config": {
                        "productUsage": [
                            {"product": "Deep Search", "usagePercent": 20},
                            {"product": "deep-search", "usagePercent": 60}
                        ]
                    }
                })),
            }),
            monthly: Err("monthly unavailable".to_string()),
        };

        let cache = build_billing_cache(&windows);
        assert_eq!(cache.quotas.len(), 1);
        assert_eq!(cache.quotas[0].name, "xai-product-deep-search");
        assert_eq!(cache.quotas[0].percentage, 40.0);
    }

    #[test]
    fn successful_empty_billing_is_treated_as_free_and_needs_probe() {
        let windows = BillingWindows {
            weekly: Ok(BillingResponse {
                status: 200,
                value: Some(json!({"config": {}})),
            }),
            monthly: Ok(BillingResponse {
                status: 200,
                value: Some(json!({"config": {}})),
            }),
        };
        let cache = build_billing_cache(&windows);
        assert_eq!(cache.plan_type.as_deref(), Some("free"));
        assert!(!windows.has_authoritative_quota());
    }

    #[test]
    fn reset_header_accepts_unix_milliseconds() {
        assert_eq!(
            parse_reset_at("32503680000000").as_deref(),
            Some("3000-01-01T00:00:00Z")
        );
    }

    #[test]
    fn cli_base_url_constant_matches_probe_host() {
        assert!(ACTIVE_PROBE_URL.starts_with(super::super::CLI_BASE_URL));
    }
}
