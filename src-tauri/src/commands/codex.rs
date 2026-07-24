use std::path::PathBuf;
use std::sync::Arc;

use token_proxy_app::app::TokenProxyApp;

use crate::codex;

#[tauri::command]
pub async fn codex_list_accounts(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    codex_store.list_accounts().await
}

#[tauri::command]
pub async fn codex_import_file(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    path: String,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Import path is required.".to_string());
    }
    token_proxy_app
        .codex_import_file(PathBuf::from(trimmed))
        .await
}

#[tauri::command]
pub async fn codex_import_text(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    contents: String,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    token_proxy_app.codex_import_text(&contents).await
}

#[tauri::command]
pub async fn codex_import_refresh_tokens(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    contents: String,
    client_kind: String,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    let client = parse_codex_refresh_token_client(&client_kind)?;
    token_proxy_app
        .codex_import_refresh_tokens(&contents, client)
        .await
}

#[tauri::command]
pub async fn codex_fetch_quotas(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
) -> Result<Vec<codex::CodexQuotaSummary>, String> {
    codex::fetch_quotas(codex_store.as_ref()).await
}

#[tauri::command]
pub async fn codex_refresh_quota_cache(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_ids: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    codex_store
        .refresh_quota_cache(account_ids.as_deref())
        .await
}

#[tauri::command]
pub async fn codex_refresh_quota_now(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_id: String,
) -> Result<(), String> {
    codex_store.refresh_quota_cache_now(&account_id).await
}

#[tauri::command]
pub async fn codex_refresh_account(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_id: String,
) -> Result<(), String> {
    codex_store.refresh_account(&account_id).await
}

#[tauri::command]
pub async fn codex_set_auto_refresh(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_id: String,
    enabled: bool,
) -> Result<codex::CodexAccountSummary, String> {
    codex_store.set_auto_refresh(&account_id, enabled).await
}

// Phase B: 账户级 priority / proxy_url / 人工 Disabled 已删除；只保留 auto_refresh。

#[tauri::command]
pub async fn codex_start_login(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
) -> Result<codex::CodexLoginStartResponse, String> {
    codex_login.start_login().await
}

#[tauri::command]
pub async fn codex_poll_login(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    state: String,
) -> Result<codex::CodexLoginPollResponse, String> {
    token_proxy_app.codex_poll_login(&state).await
}

// Phase C2: codex_logout Tauri 删除路径已移除；删除只能由 Upstream 级联触发。

fn parse_codex_refresh_token_client(value: &str) -> Result<codex::CodexRefreshTokenClient, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "codex" => Ok(codex::CodexRefreshTokenClient::Codex),
        "mobile" => Ok(codex::CodexRefreshTokenClient::Mobile),
        other => Err(format!("Unsupported Codex refresh token client: {other}")),
    }
}
