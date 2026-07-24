// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod agent_node_service;
mod app_proxy;
mod client_config;
mod codex;
mod commands;
mod kiro;
mod logging;
mod tray;
mod window;
mod xai;

use std::{future::Future, sync::Arc};
use tauri::{Emitter, Manager};

use commands::{
    agent_node_read_config, agent_node_restart, agent_node_save_config, agent_node_start,
    agent_node_status, agent_node_stop, codex_fetch_quotas, codex_import_file,
    codex_import_refresh_tokens, codex_import_text, codex_list_accounts, codex_poll_login,
    codex_refresh_account, codex_refresh_quota_cache, codex_refresh_quota_now,
    codex_set_auto_refresh, codex_start_login, fetch_upstream_models, kiro_fetch_quotas,
    kiro_handle_callback, kiro_import_ide, kiro_import_kam, kiro_list_accounts, kiro_poll_login,
    kiro_refresh_quota_cache, kiro_refresh_quota_now, kiro_start_login, prepare_relaunch,
    preview_client_setup, providers_list_accounts_page, proxy_reload, proxy_restart, proxy_start,
    proxy_status, proxy_stop, read_dashboard_snapshot, read_data_storage_usage,
    read_default_hot_model_mappings, read_model_pricing_settings, read_proxy_config,
    read_request_detail_capture, read_request_log_detail, refresh_dashboard_model_discovery,
    reset_model_pricing_settings, save_model_pricing_settings, save_proxy_config,
    set_request_detail_capture, write_claude_code_settings, write_codex_config, xai_cancel_login,
    xai_fetch_quotas, xai_import_file, xai_import_refresh_tokens, xai_import_text,
    xai_list_accounts, xai_poll_login, xai_refresh_account, xai_refresh_quota_cache,
    xai_refresh_quota_now, xai_set_auto_refresh, xai_start_login,
};

type LogLevel = logging::LogLevel;

const REQUEST_DETAIL_CAPTURE_EVENT: &str = "request-detail-capture-changed";

type RequestDetailCaptureEvent = token_proxy_app::app::RequestDetailCaptureState;

/// 统一启动前置顺序：配置读取和代理状态写入完成后，才允许执行启动动作。
///
/// xAI 到期刷新任务会在 proxy 启动时立即读取该状态；把写入和启动收拢到同一条
/// future 链中，避免并发 task 让首轮 OAuth refresh 误走直连。
async fn run_after_app_proxy_initialized<T, Action, ActionFuture>(
    paths: &token_proxy_account_store::paths::TokenProxyPaths,
    app_proxy_state: &app_proxy::AppProxyState,
    action: Action,
) -> Result<T, String>
where
    Action: FnOnce(token_proxy_config::ConfigResponse, Option<String>) -> ActionFuture,
    ActionFuture: Future<Output = T>,
{
    let response = token_proxy_config::read_config(paths).await?;
    let proxy_url = token_proxy_config::app_proxy_url_from_config(&response.config)?;
    app_proxy::set(app_proxy_state, proxy_url.clone()).await;
    tracing::debug!(
        proxy_enabled = proxy_url.is_some(),
        "application proxy initialized before startup action"
    );
    Ok(action(response, proxy_url).await)
}

async fn refresh_model_pricing_catalog(
    token_proxy_app: token_proxy_app::app::TokenProxyApp,
    proxy_url: Option<String>,
) {
    if let Err(err) = token_proxy_app
        .refresh_model_pricing_catalog(proxy_url.as_deref())
        .await
    {
        tracing::warn!(
            error = %err,
            "model pricing catalog refresh failed; cached or bundled catalog remains active"
        );
    }
}

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
            let app_handle_for_request_detail = app.handle().clone();
            let on_request_detail_change = Arc::new(move |state: RequestDetailCaptureEvent| {
                let _ = app_handle_for_request_detail.emit(REQUEST_DETAIL_CAPTURE_EVENT, state);
            });
            let token_proxy_app = token_proxy_app::app::TokenProxyApp::open(
                token_proxy_account_store::paths::TokenProxyPaths::from_app_data_dir(data_dir)?,
                logging_state.clone(),
                Some(on_request_detail_change),
            )?;
            let paths = token_proxy_app.paths();
            let app_proxy_state = token_proxy_app.app_proxy();
            let kiro_store = token_proxy_app.kiro_accounts();
            let codex_store = token_proxy_app.codex_accounts();
            let xai_store = token_proxy_app.xai_accounts();

            app.manage(paths.clone());
            let agent_node_service = agent_node_service::AgentNodeServiceHandle::new();
            app.manage(agent_node_service.clone());
            app.manage(logging_state.clone());
            app.manage(app_proxy_state.clone());
            app.manage(kiro_store.clone());
            app.manage(token_proxy_app.kiro_login());
            app.manage(codex_store.clone());
            app.manage(token_proxy_app.codex_login());
            app.manage(xai_store.clone());
            app.manage(token_proxy_app.xai_login());
            let app_handle = app.handle().clone();
            let tray_state = tray::init_tray(&app_handle, token_proxy_app.clone())?;
            app.manage(tray_state.clone());
            app.manage(token_proxy_app.clone());

            let paths_for_startup = paths.clone();
            let app_proxy_for_startup = app_proxy_state.clone();
            let logging_for_startup = logging_state.clone();
            let tray_state_for_startup = tray_state.clone();
            let tray_state_for_startup_error = tray_state.clone();
            let token_proxy_app_for_startup = token_proxy_app.clone();
            tauri::async_runtime::spawn(async move {
                let startup_result = run_after_app_proxy_initialized(
                    paths_for_startup.as_ref(),
                    &app_proxy_for_startup,
                    move |response, proxy_url| async move {
                        // 此时共享代理状态已经就绪；pricing 网络任务独立执行，不阻塞 proxy 启动。
                        tauri::async_runtime::spawn(refresh_model_pricing_catalog(
                            token_proxy_app_for_startup.clone(),
                            proxy_url,
                        ));

                        logging_for_startup.apply_level(response.config.log_level);
                        tray_state_for_startup
                            .apply_config(&response.config.tray_token_rate)
                            .await;
                        match token_proxy_app_for_startup.start_proxy().await {
                            Ok(status) => tray_state_for_startup.apply_status(&status),
                            Err(err) => {
                                tray_state_for_startup.apply_error("启动失败", &err);
                                tracing::error!(error = %err, "proxy start failed");
                            }
                        }
                    },
                )
                .await;
                if let Err(err) = startup_result {
                    tray_state_for_startup_error.apply_error("启动失败", &err);
                    tracing::error!(error = %err, "proxy startup initialization failed");
                }
            });

            let paths_for_agent_node = paths.clone();
            let agent_node_for_start = agent_node_service.clone();
            tauri::async_runtime::spawn(async move {
                match agent_node_service::read_agent_node_config(paths_for_agent_node.as_ref())
                    .await
                {
                    Ok(config) if config.enabled => {
                        if let Err(err) = agent_node_for_start.start_with_config(config).await {
                            tracing::error!(error = %err, "agent node autostart failed");
                        }
                    }
                    Ok(_) => {}
                    Err(err) => tracing::warn!(error = %err, "agent node config read failed"),
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
            read_data_storage_usage,
            read_default_hot_model_mappings,
            preview_client_setup,
            write_claude_code_settings,
            write_codex_config,
            save_proxy_config,
            read_model_pricing_settings,
            save_model_pricing_settings,
            reset_model_pricing_settings,
            read_dashboard_snapshot,
            refresh_dashboard_model_discovery,
            read_request_log_detail,
            read_request_detail_capture,
            set_request_detail_capture,
            kiro_list_accounts,
            kiro_import_ide,
            kiro_import_kam,
            kiro_start_login,
            kiro_poll_login,
            kiro_handle_callback,
            kiro_fetch_quotas,
            kiro_refresh_quota_cache,
            kiro_refresh_quota_now,
            codex_list_accounts,
            codex_import_file,
            codex_import_text,
            codex_import_refresh_tokens,
            codex_fetch_quotas,
            codex_refresh_quota_cache,
            codex_refresh_quota_now,
            codex_refresh_account,
            codex_set_auto_refresh,
            codex_start_login,
            codex_poll_login,
            xai_list_accounts,
            xai_import_file,
            xai_import_text,
            xai_import_refresh_tokens,
            xai_fetch_quotas,
            xai_refresh_quota_cache,
            xai_refresh_quota_now,
            xai_refresh_account,
            xai_set_auto_refresh,
            xai_start_login,
            xai_poll_login,
            xai_cancel_login,
            providers_list_accounts_page,
            proxy_status,
            proxy_start,
            proxy_stop,
            prepare_relaunch,
            proxy_restart,
            proxy_reload,
            fetch_upstream_models,
            agent_node_read_config,
            agent_node_save_config,
            agent_node_status,
            agent_node_start,
            agent_node_stop,
            agent_node_restart,
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::run_after_app_proxy_initialized;

    fn test_data_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "token-proxy-tauri-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
    }

    #[test]
    fn startup_action_observes_initialized_app_proxy() {
        let data_dir = test_data_dir("startup-proxy-order");
        fs::create_dir_all(&data_dir).expect("create test data dir");
        let paths =
            token_proxy_account_store::paths::TokenProxyPaths::from_app_data_dir(data_dir.clone())
                .expect("test paths");
        let app_proxy_state = crate::app_proxy::new_state();
        let expected_proxy = "http://127.0.0.1:7890".to_string();

        let observed_proxy = test_runtime().block_on(async {
            let mut config = token_proxy_config::ProxyConfigFile::default();
            config.app_proxy_url = Some(expected_proxy.clone());
            token_proxy_config::write_config(&paths, config)
                .await
                .expect("write test config");

            let state_for_action = app_proxy_state.clone();
            let expected_for_action = expected_proxy.clone();
            run_after_app_proxy_initialized(
                &paths,
                &app_proxy_state,
                move |_response, resolved_proxy| async move {
                    assert_eq!(
                        resolved_proxy.as_deref(),
                        Some(expected_for_action.as_str())
                    );
                    state_for_action.read().await.clone()
                },
            )
            .await
            .expect("initialize startup proxy")
        });

        assert_eq!(observed_proxy, Some(expected_proxy));
        fs::remove_dir_all(data_dir).expect("remove test data dir");
    }

    #[test]
    fn startup_initialization_clears_stale_app_proxy() {
        let data_dir = test_data_dir("startup-proxy-clear");
        fs::create_dir_all(&data_dir).expect("create test data dir");
        let paths =
            token_proxy_account_store::paths::TokenProxyPaths::from_app_data_dir(data_dir.clone())
                .expect("test paths");
        let app_proxy_state = crate::app_proxy::new_state();

        let observed_proxy = test_runtime().block_on(async {
            crate::app_proxy::set(&app_proxy_state, Some("http://127.0.0.1:8888".to_string()))
                .await;
            token_proxy_config::write_config(
                &paths,
                token_proxy_config::ProxyConfigFile::default(),
            )
            .await
            .expect("write test config");

            let state_for_action = app_proxy_state.clone();
            run_after_app_proxy_initialized(
                &paths,
                &app_proxy_state,
                move |_response, resolved_proxy| async move {
                    assert_eq!(resolved_proxy, None);
                    state_for_action.read().await.clone()
                },
            )
            .await
            .expect("initialize startup proxy")
        });

        assert_eq!(observed_proxy, None);
        fs::remove_dir_all(data_dir).expect("remove test data dir");
    }
}
