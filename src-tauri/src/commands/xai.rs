//! xAI OAuth 账户的 Tauri command 边界。
//!
//! 凭证解析、网络 I/O 与持久化均由 core Store/LoginManager 负责；这里仅做参数校验、
//! 状态注入和不含敏感信息的操作日志，避免 token 进入桌面端日志。

use std::path::PathBuf;
use std::sync::Arc;

use token_proxy_app::app::TokenProxyApp;

use crate::xai;

#[tauri::command]
pub async fn xai_list_accounts(
    xai_store: tauri::State<'_, Arc<xai::XaiAccountStore>>,
) -> Result<Vec<xai::XaiAccountSummary>, String> {
    xai_store.list_accounts().await
}

#[tauri::command]
pub async fn xai_import_file(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    path: String,
) -> Result<Vec<xai::XaiAccountSummary>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Import path is required.".to_string());
    }
    let accounts = token_proxy_app
        .xai_import_file(PathBuf::from(trimmed))
        .await?;
    tracing::info!(
        imported = accounts.len(),
        "xai account file import command completed"
    );
    Ok(accounts)
}

#[tauri::command]
pub async fn xai_import_text(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    contents: String,
) -> Result<Vec<xai::XaiAccountSummary>, String> {
    let accounts = token_proxy_app.xai_import_text(&contents).await?;
    tracing::info!(
        imported = accounts.len(),
        "xai account text import command completed"
    );
    Ok(accounts)
}

#[tauri::command]
pub async fn xai_import_refresh_tokens(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    contents: String,
) -> Result<Vec<xai::XaiAccountSummary>, String> {
    let accounts = token_proxy_app.xai_import_refresh_tokens(&contents).await?;
    tracing::info!(
        imported = accounts.len(),
        "xai refresh token import command completed"
    );
    Ok(accounts)
}

#[tauri::command]
pub async fn xai_fetch_quotas(
    xai_store: tauri::State<'_, Arc<xai::XaiAccountStore>>,
) -> Result<Vec<xai::XaiQuotaSummary>, String> {
    xai::fetch_quotas(xai_store.as_ref()).await
}

#[tauri::command]
pub async fn xai_refresh_quota_cache(
    xai_store: tauri::State<'_, Arc<xai::XaiAccountStore>>,
    account_ids: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    xai_store.refresh_quota_cache(account_ids.as_deref()).await
}

#[tauri::command]
pub async fn xai_refresh_quota_now(
    xai_store: tauri::State<'_, Arc<xai::XaiAccountStore>>,
    account_id: String,
) -> Result<(), String> {
    xai_store.refresh_quota_cache_now(&account_id).await?;
    tracing::info!(account_id, "xai quota refresh command completed");
    Ok(())
}

#[tauri::command]
pub async fn xai_refresh_account(
    xai_store: tauri::State<'_, Arc<xai::XaiAccountStore>>,
    account_id: String,
) -> Result<(), String> {
    xai_store.refresh_account(&account_id).await?;
    tracing::info!(account_id, "xai account refresh command completed");
    Ok(())
}

#[tauri::command]
pub async fn xai_set_auto_refresh(
    xai_store: tauri::State<'_, Arc<xai::XaiAccountStore>>,
    account_id: String,
    enabled: bool,
) -> Result<xai::XaiAccountSummary, String> {
    let account = xai_store.set_auto_refresh(&account_id, enabled).await?;
    tracing::info!(account_id, enabled, "xai account auto refresh updated");
    Ok(account)
}

// Phase B: 账户级 priority / proxy_url / 人工 Disabled 已删除；只保留 auto_refresh。

#[tauri::command]
pub async fn xai_start_login(
    xai_login: tauri::State<'_, Arc<xai::XaiLoginManager>>,
) -> Result<xai::XaiLoginStartResponse, String> {
    xai_login.start_login().await
}

#[tauri::command]
pub async fn xai_poll_login(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    state: String,
) -> Result<xai::XaiLoginPollResponse, String> {
    token_proxy_app.xai_poll_login(&state).await
}

#[tauri::command]
pub async fn xai_cancel_login(
    xai_login: tauri::State<'_, Arc<xai::XaiLoginManager>>,
    state: String,
) -> Result<(), String> {
    let state = state.trim();
    if state.is_empty() {
        return Err("Login state is required.".to_string());
    }
    xai_login.cancel_login(state).await?;
    tracing::info!("xai device login cancel command completed");
    Ok(())
}

// Phase C2: xai_logout Tauri 删除路径已移除；删除只能由 Upstream 级联触发。
