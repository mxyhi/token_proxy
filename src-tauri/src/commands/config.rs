use std::sync::Arc;
use std::time::Instant;

use tauri::Manager;

use crate::{app_proxy, client_config, logging, proxy, tray};

#[tauri::command]
pub async fn read_proxy_config(
    app: tauri::AppHandle,
) -> Result<proxy::config::ConfigResponse, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    proxy::config::read_config(paths.inner().as_ref()).await
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
pub async fn write_opencode_config(
    app: tauri::AppHandle,
) -> Result<client_config::ClientConfigWriteResult, String> {
    client_config::write_opencode_config(app).await
}

#[tauri::command]
pub async fn save_proxy_config(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
    logging_state: tauri::State<'_, logging::LoggingState>,
    app_proxy_state: tauri::State<'_, app_proxy::AppProxyState>,
    config: proxy::config::ProxyConfigFile,
) -> Result<proxy::service::ProxyConfigSaveResult, String> {
    tracing::debug!("save_proxy_config start");
    let start = Instant::now();
    tracing::debug!("save_proxy_config apply_config start");
    let apply_start = Instant::now();
    tray_state.apply_config(&config.tray_token_rate).await;
    tracing::debug!(
        elapsed_ms = apply_start.elapsed().as_millis(),
        "save_proxy_config apply_config done"
    );
    let log_level = config.log_level;
    let app_proxy_url = proxy::config::app_proxy_url_from_config(&config)
        .ok()
        .flatten();
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    if let Err(err) = proxy::config::write_config(paths.inner().as_ref(), config).await {
        tracing::error!(error = %err, "save_proxy_config save failed");
        tray_state.apply_error("保存失败", &err);
        return Err(err);
    }
    tracing::debug!(
        elapsed_ms = start.elapsed().as_millis(),
        "save_proxy_config saved"
    );
    logging_state.apply_level(log_level);
    app_proxy::set(&app_proxy_state, app_proxy_url).await;
    let proxy_context = app.state::<proxy::service::ProxyContext>();
    let result = proxy_service
        .apply_saved_config(proxy_context.inner())
        .await;
    tray_state.apply_status(&result.status);
    if let Some(error) = result.apply_error.as_deref() {
        tray_state.apply_error("应用失败", error);
    }
    Ok(result)
}
