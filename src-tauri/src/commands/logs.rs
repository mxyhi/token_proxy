use token_proxy_app::app::{
    RequestDetailCaptureState, RequestLogBodyPage, RequestLogDetail, TokenProxyApp,
};

#[tauri::command]
pub async fn read_request_log_detail(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    id: u64,
) -> Result<RequestLogDetail, String> {
    token_proxy_app.read_request_log_detail(id).await
}

#[tauri::command]
pub fn read_request_detail_capture(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
) -> RequestDetailCaptureState {
    token_proxy_app.request_detail_capture()
}

#[tauri::command]
pub fn set_request_detail_capture(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    enabled: bool,
) -> RequestDetailCaptureState {
    token_proxy_app.set_request_detail_capture(enabled)
}

#[tauri::command]
pub async fn read_request_log_body_page(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    id: u64,
    offset: u64,
) -> Result<RequestLogBodyPage, String> {
    token_proxy_app.read_request_log_body_page(id, offset).await
}
