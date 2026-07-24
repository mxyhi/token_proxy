use std::path::PathBuf;
use std::sync::Arc;

use token_proxy_app::app::TokenProxyApp;

use crate::kiro;

#[tauri::command]
pub async fn kiro_list_accounts(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    kiro_store.list_accounts().await
}

#[tauri::command]
pub async fn kiro_import_ide(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    directory: String,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    let trimmed = directory.trim();
    if trimmed.is_empty() {
        return Err("Directory is required.".to_string());
    }
    // 导入 + 自动补齐 Upstream 由 app 编排层串行处理。
    token_proxy_app
        .kiro_import_ide(PathBuf::from(trimmed))
        .await
}

#[tauri::command]
pub async fn kiro_import_kam(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    path: String,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("File path is required.".to_string());
    }
    token_proxy_app
        .kiro_import_kam(PathBuf::from(trimmed))
        .await
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
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    state: String,
) -> Result<kiro::KiroLoginPollResponse, String> {
    // 登录成功后自动创建缺失 Upstream；quota 路径不经过此编排。
    token_proxy_app.kiro_poll_login(&state).await
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

// Phase C2: 账户删除只能由删除 Upstream 级联触发；kiro_logout Tauri 路径已移除。
// Phase B: 账户级 priority / proxy_url / 人工 Disabled 已删除。
