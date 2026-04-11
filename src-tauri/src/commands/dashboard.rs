use std::sync::Arc;

use tauri::Manager;

use crate::proxy;

#[tauri::command]
pub async fn read_dashboard_snapshot(
    app: tauri::AppHandle,
    range: proxy::dashboard::DashboardRange,
    offset: Option<u32>,
    upstream_id: Option<String>,
) -> Result<proxy::dashboard::DashboardSnapshot, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_read_pool(paths.inner().as_ref()).await?;
    proxy::dashboard::read_snapshot(&pool, range, offset, upstream_id).await
}
