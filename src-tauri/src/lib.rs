// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod app_proxy;
mod client_config;
mod codex;
mod commands;
mod jsonc;
mod kiro;
mod logging;
mod proxy;
mod tray;
mod window;

use std::sync::Arc;
use tauri::{Emitter, Manager};

use commands::{
    codex_fetch_quotas, codex_import_file, codex_list_accounts, codex_logout, codex_poll_login,
    codex_refresh_account, codex_refresh_quota_cache, codex_refresh_quota_now,
    codex_set_auto_refresh, codex_set_priority, codex_set_proxy_url, codex_set_status,
    codex_start_login, kiro_fetch_quotas, kiro_handle_callback, kiro_import_ide, kiro_import_kam,
    kiro_list_accounts, kiro_logout, kiro_poll_login, kiro_refresh_quota_cache,
    kiro_refresh_quota_now, kiro_set_priority, kiro_set_proxy_url, kiro_set_status,
    kiro_start_login, prepare_relaunch, preview_client_setup, providers_delete_accounts,
    providers_list_accounts_page, proxy_reload, proxy_restart, proxy_start, proxy_status,
    proxy_stop, read_dashboard_snapshot, read_proxy_config, read_request_detail_capture,
    read_request_log_detail, save_proxy_config, set_request_detail_capture,
    write_claude_code_settings, write_codex_config, write_opencode_config,
};

type ProxyServiceHandle = proxy::service::ProxyServiceHandle;
type LogLevel = logging::LogLevel;

const REQUEST_DETAIL_CAPTURE_EVENT: &str = "request-detail-capture-changed";

type RequestDetailCaptureEvent = proxy::request_detail::RequestDetailCaptureState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 默认 silent；后续加载配置后按需调整。
    let logging_state = logging::LoggingState::init(LogLevel::Silent);
    tracing::info!("starting token_proxy application");
    let autostart_launch = window::is_autostart_launch();

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init());
    #[cfg(desktop)]
    {
        builder = builder.plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--autostart"])
                .build(),
        );
        // 二次启动时唤起并聚焦已有主窗口，避免多实例托盘图标。
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(login) = app.try_state::<Arc<kiro::KiroLoginManager>>() {
                for arg in &args {
                    if arg.starts_with("kiro://") {
                        let url = arg.clone();
                        let login = login.inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = login.handle_callback_url(&url).await;
                        });
                        break;
                    }
                }
            }
            window::show_or_create_main_window(app);
        }));
    }

    let app = builder
        .setup(move |app| {
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_process::init())?;
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
            }

            let data_dir = app
                .handle()
                .path()
                .app_config_dir()
                .map_err(|err| format!("Failed to resolve app config dir: {err}"))?;
            let paths = Arc::new(token_proxy_core::paths::TokenProxyPaths::from_app_data_dir(
                data_dir,
            )?);
            app.manage(paths.clone());

            let token_rate = proxy::token_rate::TokenRateTracker::new();
            app.manage(token_rate.clone());
            let app_handle_for_request_detail = app.handle().clone();
            let on_request_detail_change = Arc::new(move |state: RequestDetailCaptureEvent| {
                let _ = app_handle_for_request_detail.emit(REQUEST_DETAIL_CAPTURE_EVENT, state);
            });
            let request_detail = Arc::new(proxy::request_detail::RequestDetailCapture::new(Some(
                on_request_detail_change,
            )));
            app.manage(request_detail.clone());
            let proxy_service = ProxyServiceHandle::new();
            app.manage(proxy_service.clone());
            app.manage(logging_state.clone());
            let app_proxy_state = app_proxy::new_state();
            app.manage(app_proxy_state.clone());
            let app_handle = app.handle().clone();
            let kiro_store = Arc::new(kiro::KiroAccountStore::new(
                paths.as_ref(),
                app_proxy_state.clone(),
            )?);
            app.manage(kiro_store.clone());
            let kiro_login = Arc::new(kiro::KiroLoginManager::new(
                kiro_store.clone(),
                app_proxy_state.clone(),
            ));
            app.manage(kiro_login);
            let codex_store = Arc::new(codex::CodexAccountStore::new(
                paths.as_ref(),
                app_proxy_state.clone(),
            )?);
            app.manage(codex_store.clone());
            let codex_login = Arc::new(codex::CodexLoginManager::new(
                codex_store.clone(),
                app_proxy_state.clone(),
            ));
            app.manage(codex_login);

            let proxy_context = proxy::service::ProxyContext {
                paths: paths.clone(),
                logging: logging_state.clone(),
                request_detail: request_detail.clone(),
                token_rate: token_rate.clone(),
                kiro_accounts: kiro_store.clone(),
                codex_accounts: codex_store.clone(),
            };
            app.manage(proxy_context.clone());
            let tray_state = tray::init_tray(&app_handle, proxy_service.clone())?;
            app.manage(tray_state.clone());

            let tray_state_for_config = tray_state.clone();
            let paths_for_config = paths.clone();
            let app_proxy_for_config = app_proxy_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(response) = proxy::config::read_config(paths_for_config.as_ref()).await {
                    logging_state.apply_level(response.config.log_level);
                    tray_state_for_config
                        .apply_config(&response.config.tray_token_rate)
                        .await;
                    if let Ok(proxy_url) =
                        proxy::config::app_proxy_url_from_config(&response.config)
                    {
                        app_proxy::set(&app_proxy_for_config, proxy_url).await;
                    }
                }
            });

            let tray_state_for_start = tray_state.clone();
            let proxy_for_start = proxy_service.clone();
            let proxy_context_for_start = proxy_context.clone();
            tauri::async_runtime::spawn(async move {
                match proxy_for_start.start(&proxy_context_for_start).await {
                    Ok(status) => tray_state_for_start.apply_status(&status),
                    Err(err) => {
                        tray_state_for_start.apply_error("启动失败", &err);
                        tracing::error!(error = %err, "proxy start failed");
                    }
                }
            });

            if autostart_launch {
                window::set_main_window_visibility(&app_handle, false);
                if let Some(window) = app_handle.get_webview_window(window::MAIN_WINDOW_LABEL) {
                    let _ = window.hide();
                }
                tray_state.sync_main_window_menu_item(&app_handle);
            } else {
                window::show_or_create_main_window(&app_handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(true) => {
                if window.label() == window::MAIN_WINDOW_LABEL {
                    crate::window::set_main_window_visibility(window.app_handle(), true);
                    if let Some(tray_state) = window.app_handle().try_state::<tray::TrayState>() {
                        tray_state.sync_main_window_menu_item(window.app_handle());
                    }
                }
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let tray_state = window.app_handle().try_state::<tray::TrayState>();
                if tray_state
                    .as_ref()
                    .map(|state| state.should_quit())
                    .unwrap_or(false)
                {
                    return;
                }
                // 关闭即销毁 WebView，后台核心继续运行。
                api.prevent_close();
                if window.label() == window::MAIN_WINDOW_LABEL {
                    crate::window::hide_main_window(window.app_handle());
                    return;
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
            save_proxy_config,
            read_dashboard_snapshot,
            read_request_log_detail,
            read_request_detail_capture,
            set_request_detail_capture,
            kiro_list_accounts,
            kiro_import_ide,
            kiro_import_kam,
            kiro_start_login,
            kiro_poll_login,
            kiro_logout,
            kiro_handle_callback,
            kiro_fetch_quotas,
            kiro_refresh_quota_cache,
            kiro_refresh_quota_now,
            kiro_set_status,
            kiro_set_proxy_url,
            kiro_set_priority,
            codex_list_accounts,
            codex_import_file,
            codex_fetch_quotas,
            codex_refresh_quota_cache,
            codex_refresh_quota_now,
            codex_refresh_account,
            codex_set_auto_refresh,
            codex_set_status,
            codex_set_proxy_url,
            codex_set_priority,
            codex_start_login,
            codex_poll_login,
            codex_logout,
            providers_list_accounts_page,
            providers_delete_accounts,
            proxy_status,
            proxy_start,
            proxy_stop,
            prepare_relaunch,
            proxy_restart,
            proxy_reload,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            let tray_state = app_handle.try_state::<tray::TrayState>();
            if tray_state
                .as_ref()
                .map(|state| state.should_quit())
                .unwrap_or(false)
            {
                return;
            }
            // 仅关闭窗口时阻止退出，允许托盘“退出”彻底结束进程。
            api.prevent_exit();
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen {
            has_visible_windows,
            ..
        } => {
            // 点击 Dock 重新打开时，恢复主窗口。
            if !has_visible_windows {
                window::show_or_create_main_window(app_handle);
            }
        }
        _ => {}
    });
}
