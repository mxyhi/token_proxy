use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use reqwest::{Client, Proxy};

use crate::oauth_util::build_reqwest_client;

use super::store::CodexAccountStore;
use super::types::CodexAccountSummary;

const CODEX_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
// Match Codex CLI UA to avoid edge filtering on some proxies.
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.50.0 (Mac OS 26.0.1; arm64) Apple_Terminal/464";

#[derive(Clone, Serialize)]
pub(crate) struct CodexQuotaItem {
    pub(crate) name: String,
    pub(crate) percentage: f64,
    pub(crate) used: Option<f64>,
    pub(crate) limit: Option<f64>,
    pub(crate) reset_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct CodexQuotaSummary {
    pub(crate) account_id: String,
    pub(crate) plan_type: Option<String>,
    pub(crate) quotas: Vec<CodexQuotaItem>,
    pub(crate) error: Option<String>,
}

pub(crate) async fn fetch_quotas(
    store: &CodexAccountStore,
) -> Result<Vec<CodexQuotaSummary>, String> {
    let accounts = store.list_accounts().await?;
    let proxy_url = store.app_proxy_url().await;
    let mut results = Vec::with_capacity(accounts.len());
    for account in accounts {
        match fetch_account_quota(store, &account, proxy_url.as_deref()).await {
            Ok(summary) => results.push(summary),
            Err(err) => results.push(CodexQuotaSummary {
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
    store: &CodexAccountStore,
    account: &CodexAccountSummary,
    proxy_url: Option<&str>,
) -> Result<CodexQuotaSummary, String> {
    let record = store.get_account_record(&account.account_id).await?;
    let response = request_usage(&record.access_token, record.account_id.as_deref(), proxy_url).await?;
    Ok(map_usage_response(account, response))
}

async fn request_usage(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
    proxy_url: Option<&str>,
) -> Result<CodexUsageResponse, String> {
    let attempts = build_usage_attempts(proxy_url);
    let mut send_errors = Vec::new();

    for attempt in attempts {
        match request_usage_once(access_token, chatgpt_account_id, &attempt).await {
            Ok(response) => return Ok(response),
            Err(UsageRequestError::Send(err)) => {
                send_errors.push(format!(
                    "{}: {}",
                    attempt.label,
                    format_reqwest_error(&err)
                ));
            }
            Err(err) => {
                return Err(format!(
                    "Codex usage request failed: {}",
                    format_usage_error(err)
                ));
            }
        }
    }

    let detail = if send_errors.is_empty() {
        "unknown error".to_string()
    } else {
        send_errors.join(" | ")
    };
    Err(format!("Codex usage request failed: {detail}"))
}

fn map_usage_response(
    account: &CodexAccountSummary,
    response: CodexUsageResponse,
) -> CodexQuotaSummary {
    let mut quotas = Vec::new();
    if let Some(rate_limit) = response.rate_limit {
        if let Some(item) = build_window_quota("codex-session", rate_limit.primary_window) {
            quotas.push(item);
        }
        if let Some(item) = build_window_quota("codex-weekly", rate_limit.secondary_window) {
            quotas.push(item);
        }
    }

    CodexQuotaSummary {
        account_id: account.account_id.clone(),
        plan_type: response.plan_type,
        quotas,
        error: None,
    }
}

fn build_window_quota(name: &str, window: Option<CodexRateWindow>) -> Option<CodexQuotaItem> {
    let window = window?;
    let used_percent = window.used_percent?;
    let percentage = (100.0 - used_percent).clamp(0.0, 100.0);
    Some(CodexQuotaItem {
        name: name.to_string(),
        percentage,
        used: None,
        limit: None,
        reset_at: window.reset_at.and_then(reset_at_from_seconds),
    })
}

fn reset_at_from_seconds(seconds: i64) -> Option<String> {
    let value = OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    Some(value.format(&Rfc3339).unwrap_or_else(|_| seconds.to_string()))
}

async fn request_usage_once(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
    attempt: &UsageAttempt,
) -> Result<CodexUsageResponse, UsageRequestError> {
    let http = build_usage_client(attempt.proxy_url.as_deref(), attempt.http1_only)
        .map_err(UsageRequestError::Build)?;
    let mut request = http
        .get(CODEX_USAGE_ENDPOINT)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", CODEX_USER_AGENT);
    if let Some(account_id) = chatgpt_account_id.filter(|value| !value.trim().is_empty()) {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = request.send().await.map_err(UsageRequestError::Send)?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| UsageRequestError::Decode(format!("Failed to read response: {err}")))?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        return Err(UsageRequestError::Status(status.as_u16(), body.to_string()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|err| UsageRequestError::Decode(format!("Invalid response: {err}")))
}

fn build_usage_client(proxy_url: Option<&str>, http1_only: bool) -> Result<Client, String> {
    if !http1_only {
        return build_reqwest_client(proxy_url, Duration::from_secs(30))
            .map_err(|err| format!("Failed to build Codex usage client: {err}"));
    }

    let mut builder = Client::builder().timeout(Duration::from_secs(30));
    let proxy_url = proxy_url.map(str::trim).filter(|value| !value.is_empty());
    if let Some(proxy_url) = proxy_url {
        let proxy = Proxy::all(proxy_url)
            .map_err(|_| "app_proxy_url is not a valid URL.".to_string())?;
        builder = builder.proxy(proxy);
    }
    builder
        .http1_only()
        .build()
        .map_err(|err| format!("Failed to build Codex usage client: {err}"))
}

fn build_usage_attempts(proxy_url: Option<&str>) -> Vec<UsageAttempt> {
    let mut attempts = Vec::new();
    attempts.push(UsageAttempt {
        label: "primary",
        proxy_url: proxy_url.map(|value| value.to_string()),
        http1_only: false,
    });

    if let Some(proxy_url) = proxy_url {
        if let Some(upgraded) = upgrade_socks5(proxy_url) {
            attempts.push(UsageAttempt {
                label: "socks5h",
                proxy_url: Some(upgraded),
                http1_only: false,
            });
        }
        attempts.push(UsageAttempt {
            label: "http1",
            proxy_url: Some(proxy_url.to_string()),
            http1_only: true,
        });
    }

    attempts
}

fn upgrade_socks5(proxy_url: &str) -> Option<String> {
    let value = proxy_url.trim();
    if value.starts_with("socks5h://") {
        return None;
    }
    if value.starts_with("socks5://") {
        return Some(value.replacen("socks5://", "socks5h://", 1));
    }
    None
}

fn format_usage_error(err: UsageRequestError) -> String {
    match err {
        UsageRequestError::Build(message) => message,
        UsageRequestError::Send(err) => format_reqwest_error(&err),
        UsageRequestError::Status(status, body) => {
            format!("status {status}: {body}")
        }
        UsageRequestError::Decode(message) => message,
    }
}

fn format_reqwest_error(err: &reqwest::Error) -> String {
    let mut details = vec![err.to_string()];
    let mut flags = Vec::new();
    if err.is_timeout() {
        flags.push("timeout");
    }
    if err.is_connect() {
        flags.push("connect");
    }
    if err.is_request() {
        flags.push("request");
    }
    if err.is_builder() {
        flags.push("builder");
    }
    if !flags.is_empty() {
        details.push(format!("flags=[{}]", flags.join(",")));
    }

    let mut source = err.source();
    let mut depth = 0;
    while let Some(cause) = source {
        if depth >= 4 {
            break;
        }
        details.push(format!("cause: {cause}"));
        source = cause.source();
        depth += 1;
    }
    details.join(" | ")
}

struct UsageAttempt {
    label: &'static str,
    proxy_url: Option<String>,
    http1_only: bool,
}

enum UsageRequestError {
    Build(String),
    Send(reqwest::Error),
    Status(u16, String),
    Decode(String),
}

#[derive(Deserialize)]
struct CodexUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<CodexRateLimit>,
}

#[derive(Deserialize)]
struct CodexRateLimit {
    primary_window: Option<CodexRateWindow>,
    secondary_window: Option<CodexRateWindow>,
}

#[derive(Deserialize)]
struct CodexRateWindow {
    used_percent: Option<f64>,
    reset_at: Option<i64>,
}
