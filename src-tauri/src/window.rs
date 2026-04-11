use tauri::Manager;

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
        sync_main_window_menu_item(app);
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
                let window = builder.build()?;
                set_main_window_visibility(&app_handle, true);
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                Ok(())
            });
        if let Err(err) = result {
            tracing::warn!(error = %err, "create main window failed");
            return;
        }
        sync_main_window_menu_item(&app_handle);
    });
}

pub(crate) fn hide_main_window(app: &tauri::AppHandle) {
    set_main_window_visibility(app, false);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.hide();
        if let Err(err) = window.destroy() {
            tracing::warn!(error = %err, "destroy window failed");
        }
    }

    if let Some(tray_state) = app.try_state::<crate::tray::TrayState>() {
        tray_state.mark_main_window_hidden();
    } else {
        sync_main_window_menu_item(app);
    }
}

pub(crate) fn toggle_main_window(app: &tauri::AppHandle) {
    let visible = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_main_window(app);
    } else {
        show_or_create_main_window(app);
    }
}

pub(crate) fn is_autostart_launch() -> bool {
    std::env::args().any(|arg| arg == "--autostart")
}

fn sync_main_window_menu_item(app: &tauri::AppHandle) {
    if let Some(tray_state) = app.try_state::<crate::tray::TrayState>() {
        tray_state.sync_main_window_menu_item(app);
    }
}
