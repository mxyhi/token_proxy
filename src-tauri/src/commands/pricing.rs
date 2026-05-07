use std::sync::Arc;

use tauri::Manager;

use crate::proxy;

#[tauri::command]
pub async fn read_model_pricing_settings(
    app: tauri::AppHandle,
) -> Result<proxy::pricing::ModelPricingSettingsSnapshot, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_read_pool(paths.inner().as_ref()).await?;
    proxy::pricing::read_model_pricing_settings_snapshot(&pool).await
}

#[tauri::command]
pub async fn save_model_pricing_settings(
    app: tauri::AppHandle,
    settings: proxy::pricing::ModelPricingSettingsInput,
) -> Result<proxy::pricing::ModelPricingSettingsSnapshot, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_write_pool(paths.inner().as_ref()).await?;
    proxy::pricing::save_model_pricing_settings(&pool, settings).await
}

#[tauri::command]
pub async fn reset_model_pricing_settings(
    app: tauri::AppHandle,
) -> Result<proxy::pricing::ModelPricingSettingsSnapshot, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    let pool = proxy::sqlite::open_write_pool(paths.inner().as_ref()).await?;
    proxy::pricing::reset_model_pricing_settings(&pool).await
}
