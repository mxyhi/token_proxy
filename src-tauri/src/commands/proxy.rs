use crate::{proxy, tray};
use tauri::Manager;

#[tauri::command]
pub async fn proxy_status(
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
    let status = proxy_service.status().await;
    tray_state.apply_status(&status);
    Ok(status)
}

#[tauri::command]
pub async fn proxy_start(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
    let proxy_context = app.state::<proxy::service::ProxyContext>();
    match proxy_service.start(proxy_context.inner()).await {
        Ok(status) => {
            tray_state.apply_status(&status);
            Ok(status)
        }
        Err(err) => {
            tray_state.apply_error("启动失败", &err);
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn proxy_stop(
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
    match proxy_service.stop().await {
        Ok(status) => {
            tray_state.apply_status(&status);
            Ok(status)
        }
        Err(err) => {
            tray_state.apply_error("停止失败", &err);
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn prepare_relaunch(
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<(), String> {
    tray_state.mark_quit();
    proxy_service.stop().await.map(|_| ())
}

#[tauri::command]
pub async fn proxy_restart(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
    let proxy_context = app.state::<proxy::service::ProxyContext>();
    match proxy_service.restart(proxy_context.inner()).await {
        Ok(status) => {
            tray_state.apply_status(&status);
            Ok(status)
        }
        Err(err) => {
            tray_state.apply_error("重启失败", &err);
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn proxy_reload(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
    let proxy_context = app.state::<proxy::service::ProxyContext>();
    match proxy_service.reload(proxy_context.inner()).await {
        Ok(status) => {
            tray_state.apply_status(&status);
            Ok(status)
        }
        Err(err) => {
            tray_state.apply_error("重载失败", &err);
            Err(err)
        }
    }
}
