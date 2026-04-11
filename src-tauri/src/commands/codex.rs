use std::path::PathBuf;
use std::sync::Arc;

use crate::codex;
use crate::commands::parse_manual_account_status;

#[tauri::command]
pub async fn codex_list_accounts(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    codex_store.list_accounts().await
}

#[tauri::command]
pub async fn codex_import_file(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    path: String,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Import path is required.".to_string());
    }
    codex_store.import_file(PathBuf::from(trimmed)).await
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

#[tauri::command]
pub async fn codex_set_status(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_id: String,
    status: String,
) -> Result<codex::CodexAccountSummary, String> {
    let status = parse_manual_account_status(&status)?;
    codex_store.set_status(&account_id, status.into()).await
}

#[tauri::command]
pub async fn codex_set_proxy_url(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_id: String,
    proxy_url: Option<String>,
) -> Result<codex::CodexAccountSummary, String> {
    codex_store
        .set_proxy_url(&account_id, proxy_url.as_deref())
        .await
}

#[tauri::command]
pub async fn codex_set_priority(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
    account_id: String,
    priority: i32,
) -> Result<codex::CodexAccountSummary, String> {
    codex_store.set_priority(&account_id, priority).await
}

#[tauri::command]
pub async fn codex_start_login(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
) -> Result<codex::CodexLoginStartResponse, String> {
    codex_login.start_login().await
}

#[tauri::command]
pub async fn codex_poll_login(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
    state: String,
) -> Result<codex::CodexLoginPollResponse, String> {
    codex_login.poll_login(&state).await
}

#[tauri::command]
pub async fn codex_logout(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
    account_id: String,
) -> Result<(), String> {
    codex_login.logout(&account_id).await
}
