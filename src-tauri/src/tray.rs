use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
#[cfg(target_os = "macos")]
use std::time::Duration;
use std::time::Instant;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager};

use crate::proxy::config::{TrayTokenRateConfig, TrayTokenRateFormat};
use crate::proxy::service::{ProxyServiceHandle, ProxyServiceState, ProxyServiceStatus};
#[cfg(target_os = "macos")]
use crate::proxy::token_rate::TokenRateSnapshot;
use crate::proxy::token_rate::TokenRateTracker;

type AppMenuItem = MenuItem<tauri::Wry>;
type AppTrayIcon = TrayIcon<tauri::Wry>;

const TRAY_ID: &str = "token-proxy-tray";
const MENU_SHOW: &str = "tray_show_window";
const MENU_START: &str = "tray_start_proxy";
const MENU_STOP: &str = "tray_stop_proxy";
const MENU_RESTART: &str = "tray_restart_proxy";
const MENU_STATUS: &str = "tray_status";
const MENU_QUIT: &str = "tray_quit";
const SHOW_MAIN_WINDOW_TEXT: &str = "显示主窗口";
const HIDE_MAIN_WINDOW_TEXT: &str = "隐藏主窗口";

#[derive(Clone)]
pub(crate) struct TrayState {
    inner: Arc<TrayStateInner>,
}

struct TrayStateInner {
    tray: AppTrayIcon,
    show_item: AppMenuItem,
    start_item: AppMenuItem,
    stop_item: AppMenuItem,
    restart_item: AppMenuItem,
    status_item: AppMenuItem,
    token_rate: Arc<TokenRateTracker>,
    token_rate_config: RwLock<TrayTokenRateConfig>,
    last_title: RwLock<Option<String>>,
    should_quit: AtomicBool,
    // 0 表示无循环，非 0 表示正在运行的 token 速率循环 id。
    token_rate_loop_active: AtomicU64,
    token_rate_loop_counter: AtomicU64,
}

impl TrayState {
    pub(crate) fn should_quit(&self) -> bool {
        self.inner.should_quit.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_quit(&self) {
        self.inner.should_quit.store(true, Ordering::SeqCst);
    }

    pub(crate) async fn apply_config(&self, config: &TrayTokenRateConfig) {
        tracing::debug!("tray apply_config start");
        let start = Instant::now();
        tracing::debug!("tray apply_config acquiring token_rate_config write lock");
        let enabled = {
            let mut guard = self
                .inner
                .token_rate_config
                .write()
                .expect("tray token rate config lock poisoned");
            tracing::debug!(
                elapsed_ms = start.elapsed().as_millis(),
                "tray apply_config token_rate_config locked"
            );
            *guard = config.clone();
            config.enabled
        };
        tracing::debug!(
            enabled,
            elapsed_ms = start.elapsed().as_millis(),
            "tray apply_config set_enabled start"
        );
        self.inner.token_rate.set_enabled(enabled).await;
        tracing::debug!(
            enabled,
            elapsed_ms = start.elapsed().as_millis(),
            "tray apply_config set_enabled done"
        );
        #[cfg(target_os = "macos")]
        {
            if enabled {
                self.ensure_token_rate_loop();
            } else {
                self.stop_token_rate_loop();
                self.clear_title();
            }
        }
        // 配置变化也唤醒托盘刷新，避免空闲等待时错过更新。
        tracing::debug!(
            elapsed_ms = start.elapsed().as_millis(),
            "tray apply_config notify_activity"
        );
        self.inner.token_rate.notify_activity();
        tracing::debug!(
            elapsed_ms = start.elapsed().as_millis(),
            "tray apply_config done"
        );
    }

    pub(crate) fn apply_status(&self, status: &ProxyServiceStatus) {
        let text = format_status_text(status);
        let _ = self.inner.status_item.set_text(text);
        let _ = self.inner.status_item.set_enabled(false);

        match status.state {
            ProxyServiceState::Running => {
                let _ = self.inner.start_item.set_enabled(false);
                let _ = self.inner.stop_item.set_enabled(true);
                let _ = self.inner.restart_item.set_enabled(true);
            }
            ProxyServiceState::Stopped => {
                let _ = self.inner.start_item.set_enabled(true);
                let _ = self.inner.stop_item.set_enabled(false);
                let _ = self.inner.restart_item.set_enabled(false);
            }
        }
    }

    pub(crate) fn apply_error(&self, title: &str, err: &str) {
        let message = format!("{title} · {}", compact_error(err));
        let _ = self.inner.status_item.set_text(message);
        let _ = self.inner.status_item.set_enabled(false);
    }

    pub(crate) fn sync_main_window_menu_item(&self, app: &AppHandle) {
        let visible = app
            .get_webview_window(crate::MAIN_WINDOW_LABEL)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        let _ = self
            .inner
            .show_item
            .set_text(main_window_menu_text(visible));
    }

    #[cfg(target_os = "macos")]
    async fn update_token_rate_title(&self) {
        let config = {
            self.inner
                .token_rate_config
                .read()
                .expect("tray token rate config lock poisoned")
                .clone()
        };
        if !config.enabled {
            self.clear_title();
            return;
        }
        // 启用后始终显示速率；无 token 时展示并发请求数。
        let snapshot = self.inner.token_rate.snapshot().await;
        let title = format_rate_title(snapshot, config.format);
        self.set_title(Some(title));
    }

    #[cfg(target_os = "macos")]
    fn set_title(&self, title: Option<String>) {
        let mut last_title = self
            .inner
            .last_title
            .write()
            .expect("tray title lock poisoned");
        if *last_title == title {
            return;
        }
        let _ = self.inner.tray.set_title(title.as_deref());
        *last_title = title;
    }

    #[cfg(target_os = "macos")]
    fn clear_title(&self) {
        // Tauri on macOS may not clear title with None; empty string is more reliable.
        self.set_title(Some(String::new()));
    }

    #[cfg(target_os = "macos")]
    fn ensure_token_rate_loop(&self) {
        if !self.is_token_rate_enabled() {
            return;
        }
        let current = self.inner.token_rate_loop_active.load(Ordering::SeqCst);
        if current != 0 {
            return;
        }
        let loop_id = self
            .inner
            .token_rate_loop_counter
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if self
            .inner
            .token_rate_loop_active
            .compare_exchange(0, loop_id, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            start_token_rate_loop(self.clone(), loop_id);
        }
    }

    #[cfg(target_os = "macos")]
    fn stop_token_rate_loop(&self) {
        self.inner.token_rate_loop_active.store(0, Ordering::SeqCst);
    }

    #[cfg(target_os = "macos")]
    fn is_token_rate_enabled(&self) -> bool {
        self.inner
            .token_rate_config
            .read()
            .expect("tray token rate config lock poisoned")
            .enabled
    }

    #[cfg(target_os = "macos")]
    fn should_keep_token_rate_loop(&self, loop_id: u64) -> bool {
        if self.should_quit() {
            return false;
        }
        self.inner.token_rate_loop_active.load(Ordering::SeqCst) == loop_id
    }

    #[cfg(target_os = "macos")]
    fn finish_token_rate_loop(&self, loop_id: u64) {
        let active = self.inner.token_rate_loop_active.load(Ordering::SeqCst);
        if active == loop_id {
            self.inner.token_rate_loop_active.store(0, Ordering::SeqCst);
        }
    }
}

pub(crate) fn init_tray(
    app: &AppHandle,
    proxy_service: ProxyServiceHandle,
) -> Result<TrayState, Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(
        app,
        MENU_SHOW,
        main_window_menu_text(false),
        true,
        None::<&str>,
    )?;
    let start_item = MenuItem::with_id(app, MENU_START, "启动代理", true, None::<&str>)?;
    let stop_item = MenuItem::with_id(app, MENU_STOP, "停止代理", false, None::<&str>)?;
    let restart_item = MenuItem::with_id(app, MENU_RESTART, "重启代理", false, None::<&str>)?;
    let status_item = MenuItem::with_id(app, MENU_STATUS, "状态：启动中...", false, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;

    let menu = Menu::new(app)?;
    menu.append_items(&[
        &show_item,
        &PredefinedMenuItem::separator(app)?,
        &start_item,
        &stop_item,
        &restart_item,
        &PredefinedMenuItem::separator(app)?,
        &status_item,
        &PredefinedMenuItem::separator(app)?,
        &quit_item,
    ])?;

    // 开发环境显示原色，生产环境使用模板图标以适配系统主题。
    let is_template = !cfg!(debug_assertions);
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(load_tray_icon()?)
        .tooltip("Token Proxy")
        .show_menu_on_left_click(true)
        .icon_as_template(is_template)
        .menu(&menu)
        .build(app)?;

    let token_rate = app.state::<Arc<TokenRateTracker>>().inner().clone();
    let tray_state = TrayState {
        inner: Arc::new(TrayStateInner {
            tray,
            show_item: show_item.clone(),
            start_item: start_item.clone(),
            stop_item: stop_item.clone(),
            restart_item: restart_item.clone(),
            status_item: status_item.clone(),
            token_rate,
            token_rate_config: RwLock::new(TrayTokenRateConfig::default()),
            last_title: RwLock::new(None),
            should_quit: AtomicBool::new(false),
            token_rate_loop_active: AtomicU64::new(0),
            token_rate_loop_counter: AtomicU64::new(0),
        }),
    };

    let tray_state_for_menu = tray_state.clone();
    let proxy_for_menu = proxy_service.clone();
    tray_state.inner.tray.on_menu_event(move |app, event| {
        let id = event.id().as_ref();
        match id {
            MENU_SHOW => {
                crate::toggle_main_window(app);
            }
            MENU_START => {
                let app = app.clone();
                let tray_state = tray_state_for_menu.clone();
                let proxy_service = proxy_for_menu.clone();
                tauri::async_runtime::spawn(async move {
                    let proxy_context = app
                        .state::<crate::proxy::service::ProxyContext>()
                        .inner()
                        .clone();
                    match proxy_service.start(&proxy_context).await {
                        Ok(status) => tray_state.apply_status(&status),
                        Err(err) => tray_state.apply_error("启动失败", &err),
                    }
                });
            }
            MENU_STOP => {
                let tray_state = tray_state_for_menu.clone();
                let proxy_service = proxy_for_menu.clone();
                tauri::async_runtime::spawn(async move {
                    match proxy_service.stop().await {
                        Ok(status) => tray_state.apply_status(&status),
                        Err(err) => tray_state.apply_error("停止失败", &err),
                    }
                });
            }
            MENU_RESTART => {
                let app = app.clone();
                let tray_state = tray_state_for_menu.clone();
                let proxy_service = proxy_for_menu.clone();
                tauri::async_runtime::spawn(async move {
                    let proxy_context = app
                        .state::<crate::proxy::service::ProxyContext>()
                        .inner()
                        .clone();
                    match proxy_service.restart(&proxy_context).await {
                        Ok(status) => tray_state.apply_status(&status),
                        Err(err) => tray_state.apply_error("重启失败", &err),
                    }
                });
            }
            MENU_QUIT => {
                tray_state_for_menu.mark_quit();
                app.exit(0);
            }
            _ => {}
        }
    });

    #[cfg(target_os = "macos")]
    tray_state.ensure_token_rate_loop();
    tray_state.sync_main_window_menu_item(app);

    Ok(tray_state)
}

#[cfg(target_os = "macos")]
fn start_token_rate_loop(tray_state: TrayState, loop_id: u64) {
    let token_rate = tray_state.inner.token_rate.clone();
    tauri::async_runtime::spawn(async move {
        let mut activity_rx = token_rate.subscribe_activity();
        'main: loop {
            if !tray_state.should_keep_token_rate_loop(loop_id) {
                break 'main;
            }
            if token_rate.has_active_requests() {
                let mut interval = tokio::time::interval(Duration::from_millis(333));
                loop {
                    interval.tick().await;
                    if !tray_state.should_keep_token_rate_loop(loop_id) {
                        break 'main;
                    }
                    tray_state.update_token_rate_title().await;
                    if !token_rate.has_active_requests() {
                        break;
                    }
                }
                continue;
            }

            tray_state.update_token_rate_title().await;
            // 空闲时不轮询，等待新请求或配置变化唤醒。
            if activity_rx.changed().await.is_err() {
                break 'main;
            }
        }
        tray_state.finish_token_rate_loop(loop_id);
    });
}

#[cfg(target_os = "macos")]
fn format_rate_title(snapshot: TokenRateSnapshot, format: TrayTokenRateFormat) -> String {
    let has_input = snapshot.input > 0;
    let has_output = snapshot.output > 0;
    let has_tokens = has_input || has_output;
    // ↑ 显示 input（有 input 时）或连接数（无 input 时）
    let input_display = if has_input {
        snapshot.input
    } else {
        snapshot.connections
    };
    // ↓ 始终显示 output
    let output_display = snapshot.output;
    // total 显示总 token 数（有 token 时）或连接数（无 token 时）
    let total_display = if has_tokens {
        snapshot.total
    } else {
        snapshot.connections
    };
    match format {
        TrayTokenRateFormat::Combined => format!("{total_display}"),
        TrayTokenRateFormat::Split => format!("↑{input_display} ↓{output_display}"),
        TrayTokenRateFormat::Both => {
            format!("{total_display} | ↑{input_display} ↓{output_display}")
        }
    }
}

fn format_status_text(status: &ProxyServiceStatus) -> String {
    match status.state {
        ProxyServiceState::Running => {
            let addr = status.addr.clone().unwrap_or_default();
            if let Some(err) = status.last_error.as_ref() {
                format!("运行中 · {addr} · 上次错误：{}", compact_error(err))
            } else {
                format!("运行中 · {addr}")
            }
        }
        ProxyServiceState::Stopped => match status.last_error.as_ref() {
            Some(err) => format!("启动失败 · {}", compact_error(err)),
            None => "已停止".to_string(),
        },
    }
}

fn compact_error(err: &str) -> String {
    let trimmed = err.trim();
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    let mut output = String::new();
    for ch in first_line.chars().take(80) {
        output.push(ch);
    }
    if first_line.chars().count() > 80 {
        output.push_str("...");
    }
    output
}

fn main_window_menu_text(visible: bool) -> &'static str {
    if visible {
        HIDE_MAIN_WINDOW_TEXT
    } else {
        SHOW_MAIN_WINDOW_TEXT
    }
}

fn load_tray_icon() -> Result<Image<'static>, Box<dyn std::error::Error>> {
    let bytes: &[u8] = if cfg!(debug_assertions) {
        &include_bytes!("../icons/icon-state.dev.png")[..]
    } else {
        &include_bytes!("../icons/icon-state.png")[..]
    };
    Ok(Image::from_bytes(bytes)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_window_menu_text_reflects_visibility() {
        assert_eq!(super::main_window_menu_text(false), "显示主窗口");
        assert_eq!(super::main_window_menu_text(true), "隐藏主窗口");
    }
}
