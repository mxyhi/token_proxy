use std::path::PathBuf;
use std::sync::Arc;

use crate::commands::parse_manual_account_status;
use crate::kiro;

#[tauri::command]
pub async fn kiro_list_accounts(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    kiro_store.list_accounts().await
}

#[tauri::command]
pub async fn kiro_import_ide(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    directory: String,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    let trimmed = directory.trim();
    if trimmed.is_empty() {
        return Err("Directory is required.".to_string());
    }
    kiro_store.import_ide_tokens(PathBuf::from(trimmed)).await
}

#[tauri::command]
pub async fn kiro_import_kam(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    path: String,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("File path is required.".to_string());
    }
    kiro_store.import_kam_export(PathBuf::from(trimmed)).await
}

#[tauri::command]
pub async fn kiro_start_login(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    method: String,
) -> Result<kiro::KiroLoginStartResponse, String> {
    let parsed = method.parse::<kiro::KiroLoginMethod>()?;
    kiro_login.start_login(parsed).await
}

#[tauri::command]
pub async fn kiro_poll_login(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    state: String,
) -> Result<kiro::KiroLoginPollResponse, String> {
    kiro_login.poll_login(&state).await
}

#[tauri::command]
pub async fn kiro_logout(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    account_id: String,
) -> Result<(), String> {
    kiro_login.logout(&account_id).await
}

#[tauri::command]
pub async fn kiro_handle_callback(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    url: String,
) -> Result<(), String> {
    kiro_login.handle_callback_url(&url).await
}

#[tauri::command]
pub async fn kiro_fetch_quotas(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
) -> Result<Vec<kiro::KiroQuotaSummary>, String> {
    kiro::fetch_quotas(kiro_store.as_ref()).await
}

#[tauri::command]
pub async fn kiro_refresh_quota_cache(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    account_ids: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    kiro_store.refresh_quota_cache(account_ids.as_deref()).await
}

#[tauri::command]
pub async fn kiro_refresh_quota_now(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    account_id: String,
) -> Result<(), String> {
    kiro_store.refresh_quota_cache_now(&account_id).await
}

#[tauri::command]
pub async fn kiro_set_status(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    account_id: String,
    status: String,
) -> Result<kiro::KiroAccountSummary, String> {
    let status = parse_manual_account_status(&status)?;
    kiro_store.set_status(&account_id, status.into()).await
}

#[tauri::command]
pub async fn kiro_set_proxy_url(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    account_id: String,
    proxy_url: Option<String>,
) -> Result<kiro::KiroAccountSummary, String> {
    kiro_store
        .set_proxy_url(&account_id, proxy_url.as_deref())
        .await
}

#[tauri::command]
pub async fn kiro_set_priority(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    account_id: String,
    priority: i32,
) -> Result<kiro::KiroAccountSummary, String> {
    kiro_store.set_priority(&account_id, priority).await
}
