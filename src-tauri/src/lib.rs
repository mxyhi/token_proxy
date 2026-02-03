// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod app_proxy;
mod client_config;
mod jsonc;
mod antigravity;
mod codex;
mod kiro;
mod logging;
mod proxy;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tauri::{Emitter, Manager};

type ProxyServiceHandle = proxy::service::ProxyServiceHandle;
type ProxyServiceStatus = proxy::service::ProxyServiceStatus;
type LogLevel = logging::LogLevel;

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
const REQUEST_DETAIL_CAPTURE_EVENT: &str = "request-detail-capture-changed";

#[derive(Clone, serde::Serialize)]
struct RequestDetailCaptureEvent {
    enabled: bool,
}

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

fn is_autostart_launch() -> bool {
    std::env::args().any(|arg| arg == "--autostart")
}

#[tauri::command]
async fn read_proxy_config(app: tauri::AppHandle) -> Result<proxy::config::ConfigResponse, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    proxy::config::read_config(paths.inner().as_ref()).await
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
    app_proxy_state: tauri::State<'_, app_proxy::AppProxyState>,
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
    let app_proxy_url = proxy::config::app_proxy_url_from_config(&config).ok().flatten();
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    if let Err(err) = proxy::config::write_config(paths.inner().as_ref(), config).await {
        tracing::error!(error = %err, "write_proxy_config save failed");
        tray_state.apply_error("保存失败", &err);
        return Err(err);
    }
    tracing::debug!(elapsed_ms = start.elapsed().as_millis(), "write_proxy_config saved");
    let reload_start = Instant::now();
    logging_state.apply_level(log_level);
    app_proxy::set(&app_proxy_state, app_proxy_url).await;
    let proxy_context = app.state::<proxy::service::ProxyContext>();
    match proxy_service.reload(proxy_context.inner()).await {
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
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_read_pool(paths.inner().as_ref()).await?;
    proxy::dashboard::read_snapshot(&pool, range, offset).await
}

#[tauri::command]
async fn read_request_log_detail(
    app: tauri::AppHandle,
    id: u64,
) -> Result<proxy::logs::RequestLogDetail, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_read_pool(paths.inner().as_ref()).await?;
    proxy::logs::read_request_log_detail(&pool, id).await
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
async fn kiro_list_accounts(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    kiro_store.list_accounts().await
}

#[tauri::command]
async fn kiro_import_ide(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    directory: String,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    let trimmed = directory.trim();
    if trimmed.is_empty() {
        return Err("Directory is required.".to_string());
    }
    kiro_store
        .import_ide_tokens(PathBuf::from(trimmed))
        .await
}

#[tauri::command]
async fn kiro_import_kam(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
    path: String,
) -> Result<Vec<kiro::KiroAccountSummary>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("File path is required.".to_string());
    }
    kiro_store
        .import_kam_export(PathBuf::from(trimmed))
        .await
}

#[tauri::command]
async fn kiro_start_login(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    method: String,
) -> Result<kiro::KiroLoginStartResponse, String> {
    let parsed = method.parse::<kiro::KiroLoginMethod>()?;
    kiro_login.start_login(parsed).await
}

#[tauri::command]
async fn kiro_poll_login(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    state: String,
) -> Result<kiro::KiroLoginPollResponse, String> {
    kiro_login.poll_login(&state).await
}

#[tauri::command]
async fn kiro_logout(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    account_id: String,
) -> Result<(), String> {
    kiro_login.logout(&account_id).await
}

#[tauri::command]
async fn kiro_handle_callback(
    kiro_login: tauri::State<'_, Arc<kiro::KiroLoginManager>>,
    url: String,
) -> Result<(), String> {
    kiro_login.handle_callback_url(&url).await
}

#[tauri::command]
async fn kiro_fetch_quotas(
    kiro_store: tauri::State<'_, Arc<kiro::KiroAccountStore>>,
) -> Result<Vec<kiro::KiroQuotaSummary>, String> {
    kiro::fetch_quotas(kiro_store.as_ref()).await
}

#[tauri::command]
async fn codex_list_accounts(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
) -> Result<Vec<codex::CodexAccountSummary>, String> {
    codex_store.list_accounts().await
}

#[tauri::command]
async fn codex_fetch_quotas(
    codex_store: tauri::State<'_, Arc<codex::CodexAccountStore>>,
) -> Result<Vec<codex::CodexQuotaSummary>, String> {
    codex::fetch_quotas(codex_store.as_ref()).await
}

#[tauri::command]
async fn codex_start_login(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
) -> Result<codex::CodexLoginStartResponse, String> {
    codex_login.start_login().await
}

#[tauri::command]
async fn codex_poll_login(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
    state: String,
) -> Result<codex::CodexLoginPollResponse, String> {
    codex_login.poll_login(&state).await
}

#[tauri::command]
async fn codex_logout(
    codex_login: tauri::State<'_, Arc<codex::CodexLoginManager>>,
    account_id: String,
) -> Result<(), String> {
    codex_login.logout(&account_id).await
}

#[tauri::command]
async fn antigravity_list_accounts(
    store: tauri::State<'_, Arc<antigravity::AntigravityAccountStore>>,
) -> Result<Vec<antigravity::AntigravityAccountSummary>, String> {
    store.list_accounts().await
}

#[tauri::command]
async fn antigravity_fetch_quotas(
    store: tauri::State<'_, Arc<antigravity::AntigravityAccountStore>>,
) -> Result<Vec<antigravity::AntigravityQuotaSummary>, String> {
    antigravity::fetch_quotas(store.as_ref()).await
}

#[tauri::command]
async fn antigravity_start_login(
    login: tauri::State<'_, Arc<antigravity::AntigravityLoginManager>>,
) -> Result<antigravity::AntigravityLoginStartResponse, String> {
    login.start_login().await
}

#[tauri::command]
async fn antigravity_poll_login(
    login: tauri::State<'_, Arc<antigravity::AntigravityLoginManager>>,
    state: String,
) -> Result<antigravity::AntigravityLoginPollResponse, String> {
    login.poll_login(&state).await
}

#[tauri::command]
async fn antigravity_logout(
    login: tauri::State<'_, Arc<antigravity::AntigravityLoginManager>>,
    account_id: String,
) -> Result<(), String> {
    login.logout(&account_id).await
}

#[tauri::command]
async fn antigravity_import_ide(
    app: tauri::AppHandle,
    store: tauri::State<'_, Arc<antigravity::AntigravityAccountStore>>,
    ide_db_path: Option<String>,
) -> Result<Vec<antigravity::AntigravityAccountSummary>, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let path = ide_db_path.map(PathBuf::from);
    antigravity::import_from_ide(paths.inner().as_ref(), store.as_ref(), path).await
}

#[tauri::command]
async fn antigravity_switch_ide_account(
    app: tauri::AppHandle,
    store: tauri::State<'_, Arc<antigravity::AntigravityAccountStore>>,
    account_id: String,
    ide_db_path: Option<String>,
) -> Result<antigravity::AntigravityIdeStatus, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let path = ide_db_path.map(PathBuf::from);
    antigravity::switch_ide_account(paths.inner().as_ref(), store.as_ref(), &account_id, path).await
}

#[tauri::command]
async fn antigravity_ide_status(
    app: tauri::AppHandle,
) -> Result<antigravity::AntigravityIdeStatus, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    antigravity::ide_status(paths.inner().as_ref()).await
}

#[tauri::command]
async fn antigravity_run_warmup(
    scheduler: tauri::State<'_, Arc<antigravity::AntigravityWarmupScheduler>>,
    account_id: String,
    model: String,
    stream: bool,
) -> Result<(), String> {
    scheduler.run_warmup(&account_id, &model, stream).await
}

#[tauri::command]
async fn antigravity_list_warmup_schedules(
    scheduler: tauri::State<'_, Arc<antigravity::AntigravityWarmupScheduler>>,
) -> Result<Vec<antigravity::AntigravityWarmupScheduleSummary>, String> {
    Ok(scheduler.list_schedules().await)
}

#[tauri::command]
async fn antigravity_set_warmup_schedule(
    scheduler: tauri::State<'_, Arc<antigravity::AntigravityWarmupScheduler>>,
    account_id: String,
    model: String,
    interval_minutes: u64,
    enabled: bool,
) -> Result<antigravity::AntigravityWarmupScheduleSummary, String> {
    scheduler
        .set_schedule(account_id, model, interval_minutes, enabled)
        .await
}

#[tauri::command]
async fn antigravity_toggle_warmup_schedule(
    scheduler: tauri::State<'_, Arc<antigravity::AntigravityWarmupScheduler>>,
    account_id: String,
    model: String,
    enabled: bool,
) -> Result<(), String> {
    scheduler.toggle_schedule(account_id, model, enabled).await
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
async fn prepare_relaunch(
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<(), String> {
    // Allow the app to exit even if the window is closed during relaunch.
    tray_state.mark_quit();
    proxy_service.stop().await.map(|_| ())
}

#[tauri::command]
async fn proxy_restart(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
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
async fn proxy_reload(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 默认 silent；后续加载配置后按需调整。
    let logging_state = logging::LoggingState::init(LogLevel::Silent);
    tracing::info!("starting token_proxy application");
    let autostart_launch = is_autostart_launch();

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
            show_or_create_main_window(app);
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
            let on_request_detail_change = Arc::new(move |enabled: bool| {
                let _ = app_handle_for_request_detail.emit(
                    REQUEST_DETAIL_CAPTURE_EVENT,
                    RequestDetailCaptureEvent { enabled },
                );
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
            let antigravity_store = Arc::new(antigravity::AntigravityAccountStore::new(
                paths.as_ref(),
                app_proxy_state.clone(),
            )?);
            app.manage(antigravity_store.clone());
            let antigravity_login = Arc::new(antigravity::AntigravityLoginManager::new(
                antigravity_store.clone(),
                app_proxy_state.clone(),
            ));
            app.manage(antigravity_login);
            let antigravity_warmup = Arc::new(antigravity::AntigravityWarmupScheduler::new(
                antigravity_store.clone(),
                app_proxy_state.clone(),
            ));
            app.manage(antigravity_warmup.clone());
            let antigravity_warmup_for_start = antigravity_warmup.clone();
            tauri::async_runtime::spawn(async move {
                antigravity_warmup_for_start.start().await;
            });

            let proxy_context = proxy::service::ProxyContext {
                paths: paths.clone(),
                logging: logging_state.clone(),
                request_detail: request_detail.clone(),
                token_rate: token_rate.clone(),
                kiro_accounts: kiro_store.clone(),
                codex_accounts: codex_store.clone(),
                antigravity_accounts: antigravity_store.clone(),
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
                set_main_window_visibility(&app_handle, false);
                if let Some(window) = app_handle.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = window.hide();
                }
            } else {
                show_or_create_main_window(&app_handle);
            }
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
            kiro_list_accounts,
            kiro_import_ide,
            kiro_import_kam,
            kiro_start_login,
            kiro_poll_login,
            kiro_logout,
            kiro_handle_callback,
            kiro_fetch_quotas,
            codex_list_accounts,
            codex_fetch_quotas,
            codex_start_login,
            codex_poll_login,
            codex_logout,
            antigravity_list_accounts,
            antigravity_fetch_quotas,
            antigravity_start_login,
            antigravity_poll_login,
            antigravity_logout,
            antigravity_import_ide,
            antigravity_switch_ide_account,
            antigravity_ide_status,
            antigravity_run_warmup,
            antigravity_list_warmup_schedules,
            antigravity_set_warmup_schedule,
            antigravity_toggle_warmup_schedule,
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
