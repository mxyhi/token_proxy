use std::sync::Arc;

use tauri::Manager;

use crate::proxy;

#[tauri::command]
pub async fn read_request_log_detail(
    app: tauri::AppHandle,
    id: u64,
) -> Result<proxy::logs::RequestLogDetail, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_read_pool(paths.inner().as_ref()).await?;
    proxy::logs::read_request_log_detail(&pool, id).await
}

#[tauri::command]
pub fn read_request_detail_capture(
    capture_state: tauri::State<'_, Arc<proxy::request_detail::RequestDetailCapture>>,
) -> proxy::request_detail::RequestDetailCaptureState {
    capture_state.snapshot()
}

#[tauri::command]
pub fn set_request_detail_capture(
    capture_state: tauri::State<'_, Arc<proxy::request_detail::RequestDetailCapture>>,
    enabled: bool,
) -> proxy::request_detail::RequestDetailCaptureState {
    if enabled {
        capture_state.arm()
    } else {
        capture_state.disarm()
    }
}
