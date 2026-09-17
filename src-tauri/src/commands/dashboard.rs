use token_proxy_app::app::{DashboardRange, DashboardSnapshot, TokenProxyApp};

#[tauri::command]
pub async fn read_dashboard_snapshot(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    range: DashboardRange,
    offset: Option<u32>,
    upstream_id: Option<String>,
    model: Option<String>,
) -> Result<DashboardSnapshot, String> {
    tracing::debug!(
        upstream_id = upstream_id.as_deref(),
        model = model.as_deref(),
        "read_dashboard_snapshot invoked"
    );
    token_proxy_app
        .read_dashboard_snapshot(range, offset, upstream_id, model)
        .await
}

#[tauri::command]
pub async fn refresh_dashboard_model_discovery(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
) -> Result<(), String> {
    let _ = token_proxy_app.refresh_model_discovery().await;
    Ok(())
}
