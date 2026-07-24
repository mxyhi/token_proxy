use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tauri::Manager;
use token_proxy_app::app::{ProxyConfigSaveResult, TokenProxyApp};
use token_proxy_app::storage_usage::DataStorageUsage;

use crate::{app_proxy, client_config, logging, tray};

#[tauri::command]
pub async fn read_proxy_config(
    app: tauri::AppHandle,
) -> Result<token_proxy_config::ConfigResponse, String> {
    let paths = app.state::<Arc<token_proxy_account_store::paths::TokenProxyPaths>>();
    token_proxy_config::read_config(paths.inner().as_ref()).await
}

/// 返回应用数据目录占用（总大小 + 数据库/配置/其它分项）。
#[tauri::command]
pub async fn read_data_storage_usage(app: tauri::AppHandle) -> Result<DataStorageUsage, String> {
    let paths = app.state::<Arc<token_proxy_account_store::paths::TokenProxyPaths>>();
    let paths = paths.inner().clone();
    // 目录 walk 是同步 IO；放到 blocking 线程避免卡住 async runtime。
    tokio::task::spawn_blocking(move || {
        token_proxy_app::storage_usage::measure_data_storage(&paths)
    })
    .await
    .map_err(|err| format!("Failed to join storage usage task: {err}"))?
}

#[tauri::command]
pub async fn preview_client_setup(
    app: tauri::AppHandle,
) -> Result<client_config::ClientSetupInfo, String> {
    client_config::preview(app).await
}

#[tauri::command]
pub async fn write_claude_code_settings(
    app: tauri::AppHandle,
) -> Result<client_config::ClientConfigWriteResult, String> {
    client_config::write_claude_code_settings(app).await
}

#[tauri::command]
pub async fn write_codex_config(
    app: tauri::AppHandle,
) -> Result<client_config::ClientConfigWriteResult, String> {
    client_config::write_codex_config(app).await
}

#[tauri::command]
pub fn read_default_hot_model_mappings() -> HashMap<String, String> {
    token_proxy_config::default_hot_model_mappings()
}

#[tauri::command]
pub async fn save_proxy_config(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
    logging_state: tauri::State<'_, logging::LoggingState>,
    app_proxy_state: tauri::State<'_, app_proxy::AppProxyState>,
    config: token_proxy_config::ProxyConfigFile,
) -> Result<ProxyConfigSaveResult, String> {
    tracing::debug!("save_proxy_config start");
    let start = Instant::now();
    // 全部 account/config mutation 走 app 编排锁；侧效应仅在成功后应用。
    let outcome = match token_proxy_app.save_proxy_config(config).await {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::error!(error = %err, "save_proxy_config failed");
            tray_state.apply_error("保存失败", &err);
            return Err(err);
        }
    };
    tracing::debug!(
        elapsed_ms = start.elapsed().as_millis(),
        removed_credentials = outcome.removed_credentials,
        "save_proxy_config orchestration done"
    );

    tray_state
        .apply_config(&outcome.side_effects.tray_token_rate)
        .await;
    logging_state.apply_level(outcome.side_effects.log_level);
    app_proxy::set(&app_proxy_state, outcome.side_effects.app_proxy_url.clone()).await;
    tray_state.apply_status(&outcome.proxy.status);
    if let Some(error) = outcome.proxy.apply_error.as_deref() {
        // 编排层已把 apply 失败当作错误回滚；此处仅作防御性展示。
        tray_state.apply_error("应用失败", error);
    }
    Ok(outcome.proxy)
}
