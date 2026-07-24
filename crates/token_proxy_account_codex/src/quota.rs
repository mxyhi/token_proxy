use serde::Deserialize;
use std::error::Error as StdError;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use reqwest::{Client, Proxy};

use token_proxy_account_store::oauth_util::build_reqwest_client;

use super::error::{format_usage_status_error, usage_status_requires_relogin};
use super::store::CodexAccountStore;
use super::types::{
    CodexAccountSummary, CodexQuotaCache, CodexQuotaItem, CodexQuotaSummary, CodexTokenRecord,
};

const CODEX_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

pub async fn fetch_quotas(store: &CodexAccountStore) -> Result<Vec<CodexQuotaSummary>, String> {
    let accounts = store.list_accounts().await?;
    let mut results = Vec::with_capacity(accounts.len());
    for account in accounts {
        match store.get_account_record(&account.account_id).await {
            Ok(record) => match fetch_account_quota(store, &account, &record).await {
                Ok(result) => results.push(result.summary),
                Err(err) => results.push(CodexQuotaSummary {
                    account_id: account.account_id.clone(),
                    plan_type: None,
                    quotas: Vec::new(),
                    error: Some(err),
                }),
            },
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

pub(crate) async fn refresh_quota_cache_if_stale(
    store: &CodexAccountStore,
    account_id: &str,
) -> Result<CodexQuotaCache, String> {
    refresh_quota_cache(store, account_id).await
}

pub(crate) async fn refresh_quota_cache(
    store: &CodexAccountStore,
    account_id: &str,
) -> Result<CodexQuotaCache, String> {
    refresh_quota_cache_with_endpoint(store, account_id, CODEX_USAGE_ENDPOINT).await
}

#[cfg(test)]
pub(crate) async fn refresh_quota_cache_with_usage_endpoint(
    store: &CodexAccountStore,
    account_id: &str,
    usage_endpoint: &str,
) -> Result<CodexQuotaCache, String> {
    refresh_quota_cache_with_endpoint(store, account_id, usage_endpoint).await
}

async fn refresh_quota_cache_with_endpoint(
    store: &CodexAccountStore,
    account_id: &str,
    usage_endpoint: &str,
) -> Result<CodexQuotaCache, String> {
    let record = store.load_account(account_id).await?;
    let checked_at = token_proxy_account_store::oauth_util::now_rfc3339();
    let account = CodexAccountSummary {
        account_id: account_id.to_string(),
        email: record.email.clone(),
        expires_at: record.expires_at().map(|value| {
            value
                .format(&Rfc3339)
                .unwrap_or_else(|_| record.expires_at_str().unwrap_or_default().to_string())
        }),
        status: record.effective_status(),
        auth_method: record.auth_method(),
        auto_refresh_enabled: record.auto_refresh_enabled(),
    };
    let resolved = match store.get_account_record(account_id).await {
        Ok(record) => record,
        Err(err) => {
            let mut failed_record = record;
            failed_record.quota.error = Some(err);
            failed_record.quota.checked_at = Some(checked_at);
            return store
                .persist_quota_cache(account_id, failed_record)
                .await
                .map(|summary| summary.quota);
        }
    };
    match fetch_account_quota_with_endpoint(store, &account, &resolved, usage_endpoint).await {
        Ok(result) => {
            let mut next_record = result.record;
            next_record.quota = CodexQuotaCache {
                plan_type: result.summary.plan_type,
                quotas: result.summary.quotas,
                error: None,
                checked_at: Some(checked_at),
            };
            store
                .persist_quota_cache(account_id, next_record)
                .await
                .map(|summary| summary.quota)
        }
        Err(err) => {
            // Token refresh may have updated account status while quota fetch was running.
            // Reload before persisting quota error so we do not overwrite an invalid marker.
            let mut failed_record = store.load_account(account_id).await.unwrap_or(resolved);
            failed_record.quota.error = Some(err);
            failed_record.quota.checked_at = Some(checked_at);
            store
                .persist_quota_cache(account_id, failed_record)
                .await
                .map(|summary| summary.quota)
        }
    }
}

async fn fetch_account_quota(
    store: &CodexAccountStore,
    account: &CodexAccountSummary,
    record: &CodexTokenRecord,
) -> Result<CodexQuotaFetchResult, String> {
    fetch_account_quota_with_endpoint(store, account, record, CODEX_USAGE_ENDPOINT).await
}

async fn fetch_account_quota_with_endpoint(
    store: &CodexAccountStore,
    account: &CodexAccountSummary,
    record: &CodexTokenRecord,
    usage_endpoint: &str,
) -> Result<CodexQuotaFetchResult, String> {
    let mut effective_record = record.clone();
    let proxy_url = store.effective_proxy_url(None).await;
    let authorization = store.authorization_header(&account.account_id).await?;
    let response = match request_usage(
        usage_endpoint,
        &authorization,
        record.account_id.as_deref(),
        proxy_url.as_deref(),
    )
    .await
    {
        Ok(response) => response,
        Err(err) if err.task_invalid && record.agent_identity().is_some() => {
            let expected_task_id = record
                .agent_identity()
                .and_then(|identity| identity.task_id)
                .unwrap_or_default();
            tracing::warn!(
                account_id = account.account_id.as_str(),
                "codex usage request rejected stale agent identity task"
            );
            store
                .recover_agent_identity_task(&account.account_id, expected_task_id)
                .await?;
            let refreshed = store.load_account(&account.account_id).await?;
            let authorization = store.authorization_header(&account.account_id).await?;
            let proxy_url = store.effective_proxy_url(None).await;
            let response = request_usage(
                usage_endpoint,
                &authorization,
                refreshed.account_id.as_deref(),
                proxy_url.as_deref(),
            )
            .await
            .map_err(|retry_err| retry_err.message)?;
            effective_record = refreshed;
            response
        }
        Err(err) if err.relogin_required && record.oauth().is_some() => {
            tracing::warn!(
                account_id = account.account_id.as_str(),
                error = %err.message,
                "codex usage request requires account refresh"
            );
            store
                .refresh_account(&account.account_id)
                .await
                .map_err(|refresh_err| {
                    format!("Codex usage request failed after token refresh failed: {refresh_err}")
                })?;
            let refreshed = store.load_account(&account.account_id).await?;
            let authorization = store.authorization_header(&account.account_id).await?;
            let proxy_url = store.effective_proxy_url(None).await;
            let response = request_usage(
                usage_endpoint,
                &authorization,
                refreshed.account_id.as_deref(),
                proxy_url.as_deref(),
            )
            .await
            .map_err(|retry_err| retry_err.message)?;
            effective_record = refreshed;
            response
        }
        Err(err) => return Err(err.message),
    };
    Ok(CodexQuotaFetchResult {
        summary: map_usage_response(account, response),
        record: effective_record,
    })
}

async fn request_usage(
    usage_endpoint: &str,
    authorization: &str,
    chatgpt_account_id: Option<&str>,
    proxy_url: Option<&str>,
) -> Result<CodexUsageResponse, CodexUsageError> {
    let attempts = build_usage_attempts(proxy_url);
    let mut send_errors = Vec::new();

    for attempt in attempts {
        match request_usage_once(usage_endpoint, authorization, chatgpt_account_id, &attempt).await
        {
            Ok(response) => return Ok(response),
            Err(UsageRequestError::Send(err)) => {
                send_errors.push(format!("{}: {}", attempt.label, format_reqwest_error(&err)));
            }
            Err(err) => {
                let formatted = format_usage_error(err);
                return Err(CodexUsageError {
                    message: format!("Codex usage request failed: {}", formatted.message),
                    relogin_required: formatted.relogin_required,
                    task_invalid: formatted.task_invalid,
                });
            }
        }
    }

    let detail = if send_errors.is_empty() {
        "unknown error".to_string()
    } else {
        send_errors.join(" | ")
    };
    Err(CodexUsageError {
        message: format!("Codex usage request failed: {detail}"),
        relogin_required: false,
        task_invalid: false,
    })
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
    Some(
        value
            .format(&Rfc3339)
            .unwrap_or_else(|_| seconds.to_string()),
    )
}

async fn request_usage_once(
    usage_endpoint: &str,
    authorization: &str,
    chatgpt_account_id: Option<&str>,
    attempt: &UsageAttempt,
) -> Result<CodexUsageResponse, UsageRequestError> {
    let http = build_usage_client(attempt.proxy_url.as_deref(), attempt.http1_only)
        .map_err(UsageRequestError::Build)?;
    let mut request = http
        .get(usage_endpoint)
        .header("Authorization", authorization)
        .header("Accept", "application/json")
        // 配额探针与代理流量复用同一身份，避免版本漂移触发边缘过滤。
        .header("User-Agent", super::USER_AGENT);
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
        let proxy =
            Proxy::all(proxy_url).map_err(|_| "app_proxy_url is not a valid URL.".to_string())?;
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

    // 账户 quota 只走 app proxy；socks5h 升级与 http1 回退复用同一 proxy_url 合同。
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

fn format_usage_error(err: UsageRequestError) -> FormattedUsageError {
    match err {
        UsageRequestError::Build(message) => FormattedUsageError {
            message,
            relogin_required: false,
            task_invalid: false,
        },
        UsageRequestError::Send(err) => FormattedUsageError {
            message: format_reqwest_error(&err),
            relogin_required: false,
            task_invalid: false,
        },
        UsageRequestError::Status(status, body) => {
            let task_invalid =
                CodexAccountStore::is_agent_identity_task_invalid(status, body.as_bytes());
            FormattedUsageError {
                message: if task_invalid {
                    "Agent Identity task is invalid.".to_string()
                } else {
                    format_usage_status_error(status, &body)
                },
                relogin_required: !task_invalid && usage_status_requires_relogin(&body),
                task_invalid,
            }
        }
        UsageRequestError::Decode(message) => FormattedUsageError {
            message,
            relogin_required: false,
            task_invalid: false,
        },
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

struct CodexUsageError {
    message: String,
    relogin_required: bool,
    task_invalid: bool,
}

struct FormattedUsageError {
    message: String,
    relogin_required: bool,
    task_invalid: bool,
}

struct CodexQuotaFetchResult {
    summary: CodexQuotaSummary,
    record: CodexTokenRecord,
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
