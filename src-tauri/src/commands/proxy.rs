use crate::{proxy, tray};
use tauri::Manager;

#[tauri::command]
pub async fn fetch_upstream_models(
    provider: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    let url = match provider.as_str() {
        "openai" | "openai-response" => format!("{}/v1/models", base_url.trim_end_matches('/')),
        "anthropic" => format!("{}/v1/models", base_url.trim_end_matches('/')),
        "gemini" => format!("{}/v1beta/models", base_url.trim_end_matches('/')),
        _ => {
            return Err(format!("不支持的 provider: {}", provider));
        }
    };

    let client = reqwest::Client::new();
    let mut request = client.get(&url);
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = request
        .send()
        .await
        .map_err(|err| format!("请求失败: {}", err))?;

    if !response.status().is_success() {
        return Err(format!("返回错误: {}", response.status()));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("解析失败: {}", err))?;

    let mut models: Vec<String> = Vec::new();

    if let Some(data) = body.get("data").and_then(|v| v.as_array()) {
        for item in data {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                models.push(id.to_string());
            }
        }
    }

    if models.is_empty() {
        if let Some(m) = body.get("models").and_then(|v| v.as_array()) {
            for item in m {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    models.push(id.to_string());
                }
            }
        }
    }

    if models.is_empty() {
        if let Some(m) = body.get("modelNames").and_then(|v| v.as_array()) {
            for item in m {
                if let Some(id) = item.as_str() {
                    models.push(id.to_string());
                }
            }
        }
    }

    if models.is_empty() {
        return Err(format!("返回为空，body: {}", body));
    }

    Ok(models)
}

#[tauri::command]
pub async fn proxy_status(
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
    let status = proxy_service.status().await;
    tray_state.apply_status(&status);
    Ok(status)
}

#[tauri::command]
pub async fn proxy_start(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
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
pub async fn proxy_stop(
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
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
pub async fn prepare_relaunch(
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<(), String> {
    tray_state.mark_quit();
    proxy_service.stop().await.map(|_| ())
}

#[tauri::command]
pub async fn proxy_restart(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
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
pub async fn proxy_reload(
    app: tauri::AppHandle,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<proxy::service::ProxyServiceStatus, String> {
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
