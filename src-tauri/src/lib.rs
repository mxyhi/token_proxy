// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod client_config;
mod jsonc;
mod logging;
mod proxy;
mod tray;

use std::sync::Arc;
use std::time::Instant;
use tauri::Manager;

type ProxyServiceHandle = proxy::service::ProxyServiceHandle;
type ProxyServiceStatus = proxy::service::ProxyServiceStatus;
type LogLevel = logging::LogLevel;

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

// 主窗口显示/销毁时同步 Dock/任务栏展示状态。
pub(crate) fn set_main_window_visibility(app: &tauri::AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if visible {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        if let Err(err) = app.set_activation_policy(policy) {
            tracing::warn!(error = %err, visible, "set activation policy failed");
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return;
        };
        if let Err(err) = window.set_skip_taskbar(!visible) {
            tracing::warn!(error = %err, visible, "set skip taskbar failed");
        }
    }
}

pub(crate) fn show_or_create_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        set_main_window_visibility(app, true);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let Some(config) = app.config().app.windows.get(0).cloned() else {
        tracing::warn!("main window config not found");
        return;
    };

    // Windows 同步创建可能死锁，放到独立线程中。
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let result =
            tauri::WebviewWindowBuilder::from_config(&app_handle, &config).and_then(|builder| {
                builder.build()?;
                Ok(())
            });
        if let Err(err) = result {
            tracing::warn!(error = %err, "create main window failed");
            return;
        }
        set_main_window_visibility(&app_handle, true);
    });
}

#[tauri::command]
async fn read_proxy_config(app: tauri::AppHandle) -> Result<proxy::config::ConfigResponse, String> {
    proxy::config::read_config(app).await
}

#[tauri::command]
async fn preview_client_setup(app: tauri::AppHandle) -> Result<client_config::ClientSetupInfo, String> {
    client_config::preview(app).await
}

#[tauri::command]
async fn write_claude_code_settings(
    app: tauri::AppHandle,
) -> Result<client_config::ClientConfigWriteResult, String> {
    client_config::write_claude_code_settings(app).await
}

#[tauri::command]
async fn write_codex_config(app: tauri::AppHandle) -> Result<client_config::ClientConfigWriteResult, String> {
    client_config::write_codex_config(app).await
}

#[tauri::command]
async fn write_opencode_config(
    app: tauri::AppHandle,
) -> Result<client_config::ClientConfigWriteResult, String> {
    client_config::write_opencode_config(app).await
}

#[tauri::command]
async fn write_proxy_config(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
    logging_state: tauri::State<'_, logging::LoggingState>,
    config: proxy::config::ProxyConfigFile,
) -> Result<ProxyServiceStatus, String> {
    tracing::debug!("write_proxy_config start");
    let start = Instant::now();
    tracing::debug!("write_proxy_config apply_config start");
    let apply_start = Instant::now();
    tray_state.apply_config(&config.tray_token_rate).await;
    tracing::debug!(
        elapsed_ms = apply_start.elapsed().as_millis(),
        "write_proxy_config apply_config done"
    );
    let log_level = config.log_level;
    if let Err(err) = proxy::config::write_config(app.clone(), config).await {
        tracing::error!(error = %err, "write_proxy_config save failed");
        tray_state.apply_error("保存失败", &err);
        return Err(err);
    }
    tracing::debug!(elapsed_ms = start.elapsed().as_millis(), "write_proxy_config saved");
    let reload_start = Instant::now();
    logging_state.apply_level(log_level);
    match proxy_service.reload(app).await {
        Ok(status) => {
            tracing::debug!(
                elapsed_ms = reload_start.elapsed().as_millis(),
                state = ?status.state,
                "write_proxy_config reloaded"
            );
            tray_state.apply_status(&status);
            Ok(status)
        }
        Err(err) => {
            tracing::error!(
                elapsed_ms = reload_start.elapsed().as_millis(),
                error = %err,
                "write_proxy_config reload failed"
            );
            tray_state.apply_error("重载失败", &err);
            Err(err)
        }
    }
}

#[tauri::command]
async fn read_dashboard_snapshot(
    app: tauri::AppHandle,
    range: proxy::dashboard::DashboardRange,
    offset: Option<u32>,
) -> Result<proxy::dashboard::DashboardSnapshot, String> {
    proxy::dashboard::read_snapshot(app, range, offset).await
}

#[tauri::command]
async fn read_request_log_detail(
    app: tauri::AppHandle,
    id: u64,
) -> Result<proxy::logs::RequestLogDetail, String> {
    proxy::logs::read_request_log_detail(app, id).await
}

#[tauri::command]
fn read_request_detail_capture(
    capture_state: tauri::State<'_, Arc<proxy::request_detail::RequestDetailCapture>>,
) -> bool {
    capture_state.is_armed()
}

#[tauri::command]
fn set_request_detail_capture(
    capture_state: tauri::State<'_, Arc<proxy::request_detail::RequestDetailCapture>>,
    enabled: bool,
) -> bool {
    if enabled {
        capture_state.arm();
    } else {
        capture_state.disarm();
    }
    capture_state.is_armed()
}

#[tauri::command]
async fn proxy_status(
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    let status = proxy_service.status().await;
    tray_state.apply_status(&status);
    Ok(status)
}

#[tauri::command]
async fn proxy_start(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match proxy_service.start(app).await {
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
async fn proxy_stop(
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
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
async fn proxy_restart(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match proxy_service.restart(app).await {
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
async fn proxy_reload(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match proxy_service.reload(app).await {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 默认 silent；后续加载配置后按需调整。
    let logging_state = logging::LoggingState::init(LogLevel::Silent);
    tracing::info!("starting token_proxy application");

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_process::init())?;
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
            }

            let token_rate = proxy::token_rate::TokenRateTracker::new();
            app.manage(token_rate.clone());
            let request_detail =
                Arc::new(proxy::request_detail::RequestDetailCapture::new(app.handle().clone()));
            app.manage(request_detail);
            let proxy_service = ProxyServiceHandle::new();
            app.manage(proxy_service.clone());
            app.manage(logging_state.clone());
            let app_handle = app.handle().clone();
            let tray_state = tray::init_tray(&app_handle, proxy_service.clone())?;
            app.manage(tray_state.clone());

            let tray_state_for_config = tray_state.clone();
            let app_handle_for_config = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(response) = proxy::config::read_config(app_handle_for_config).await {
                    logging_state.apply_level(response.config.log_level);
                    tray_state_for_config
                        .apply_config(&response.config.tray_token_rate)
                        .await;
                }
            });

            let tray_state_for_start = tray_state.clone();
            let proxy_for_start = proxy_service.clone();
            tauri::async_runtime::spawn(async move {
                match proxy_for_start.start(app_handle).await {
                    Ok(status) => tray_state_for_start.apply_status(&status),
                    Err(err) => {
                        tray_state_for_start.apply_error("启动失败", &err);
                        tracing::error!(error = %err, "proxy start failed");
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(true) => {
                if window.label() == MAIN_WINDOW_LABEL {
                    set_main_window_visibility(window.app_handle(), true);
                }
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let tray_state = window.app_handle().try_state::<tray::TrayState>();
                if tray_state.as_ref().map(|state| state.should_quit()).unwrap_or(false) {
                    return;
                }
                // 关闭即销毁 WebView，后台核心继续运行。
                api.prevent_close();
                if window.label() == MAIN_WINDOW_LABEL {
                    set_main_window_visibility(window.app_handle(), false);
                }
                if let Err(err) = window.destroy() {
                    tracing::warn!(error = %err, "destroy window failed");
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            read_proxy_config,
            preview_client_setup,
            write_claude_code_settings,
            write_codex_config,
            write_opencode_config,
            write_proxy_config,
            read_dashboard_snapshot,
            read_request_log_detail,
            read_request_detail_capture,
            set_request_detail_capture,
            proxy_status,
            proxy_start,
            proxy_stop,
            proxy_restart,
            proxy_reload,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            let tray_state = app_handle.try_state::<tray::TrayState>();
            if tray_state.as_ref().map(|state| state.should_quit()).unwrap_or(false) {
                return;
            }
            // 仅关闭窗口时阻止退出，允许托盘“退出”彻底结束进程。
            api.prevent_exit();
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { has_visible_windows, .. } => {
            // 点击 Dock 重新打开时，恢复主窗口。
            if !has_visible_windows {
                show_or_create_main_window(app_handle);
            }
        }
        _ => {}
    });
}
