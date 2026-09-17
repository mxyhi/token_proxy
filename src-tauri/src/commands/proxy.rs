use crate::tray;
use serde_json::Value;
use token_proxy_app::app::{ProxyServiceStatus, TokenProxyApp};
use url::Url;

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

#[tauri::command]
pub async fn fetch_upstream_models(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    provider: String,
    base_url: String,
    api_key: String,
    account_id: Option<String>,
    proxy_url: Option<String>,
) -> Result<Vec<String>, String> {
    if matches!(provider.as_str(), "kiro" | "codex" | "xai") {
        // 账户渠道走固定身份与官方目录，不能把 OAuth 凭据用于表单中的任意地址。
        return token_proxy_app
            .fetch_account_models(
                &provider,
                account_id.as_deref().unwrap_or(""),
                proxy_url.as_deref(),
            )
            .await;
    }
    let api_key = api_key.trim();
    let url = build_model_catalog_url(&provider, &base_url, api_key)?;
    tracing::debug!(provider = %provider, "fetching upstream model catalog");

    let client = reqwest::Client::new();
    let request = apply_model_catalog_auth(client.get(url), &provider, api_key);

    let response = request
        .send()
        .await
        .map_err(|err| {
            // Gemini key 位于 query 中；移除 URL，避免错误文本和日志泄漏凭证。
            let err = err.without_url();
            tracing::warn!(provider = %provider, error = %err, "failed to fetch upstream model catalog");
            format!("请求渠道模型列表失败: {err}")
        })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::warn!(provider = %provider, %status, "upstream model catalog returned an error");
        return Err(format!("渠道模型列表返回错误: {status}"));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|err| format!("解析渠道模型列表失败: {err}"))?;
    let models = extract_upstream_model_ids(&body);

    if models.is_empty() {
        return Err("渠道未返回可用模型。".to_string());
    }

    tracing::info!(provider = %provider, model_count = models.len(), "fetched upstream model catalog");
    Ok(models)
}

fn build_model_catalog_url(provider: &str, base_url: &str, api_key: &str) -> Result<Url, String> {
    let target_root = match provider {
        "openai" | "openai-response" | "anthropic" => "/v1/models",
        "gemini" => "/v1beta/models",
        _ => return Err(format!("不支持的 provider: {provider}")),
    };
    let mut url = Url::parse(base_url.trim()).map_err(|_| "Base URL 无效。".to_string())?;
    let base_path = url.path().trim_end_matches('/');
    let version_root = target_root
        .strip_suffix("/models")
        .expect("model catalog root must end with /models");
    let next_path = if base_path.ends_with(target_root) {
        base_path.to_string()
    } else if base_path.ends_with(version_root) {
        format!("{base_path}/models")
    } else if base_path.is_empty() {
        target_root.to_string()
    } else {
        format!("{base_path}{target_root}")
    };
    url.set_path(&next_path);
    if provider == "gemini" && !api_key.is_empty() {
        url.query_pairs_mut().append_pair("key", api_key);
    }
    Ok(url)
}

fn apply_model_catalog_auth(
    request: reqwest::RequestBuilder,
    provider: &str,
    api_key: &str,
) -> reqwest::RequestBuilder {
    if api_key.is_empty() || provider == "gemini" {
        return request;
    }
    if provider == "anthropic" {
        return request
            .header("x-api-key", api_key)
            .header("anthropic-version", DEFAULT_ANTHROPIC_VERSION);
    }
    request.bearer_auth(api_key)
}

fn extract_upstream_model_ids(body: &Value) -> Vec<String> {
    let mut models = Vec::new();
    if let Some(items) = body.get("data").and_then(Value::as_array) {
        models.extend(
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str)),
        );
    }
    if let Some(items) = body.get("models").and_then(Value::as_array) {
        models.extend(items.iter().filter_map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("name").and_then(Value::as_str))
        }));
    }
    if let Some(items) = body.get("modelNames").and_then(Value::as_array) {
        models.extend(items.iter().filter_map(Value::as_str));
    }
    let mut models = models
        .into_iter()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| model.trim_start_matches("models/").to_string())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

#[tauri::command]
pub async fn proxy_status(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    let status = token_proxy_app.proxy_status().await;
    tray_state.apply_status(&status);
    Ok(status)
}

#[tauri::command]
pub async fn proxy_start(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match token_proxy_app.start_proxy().await {
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
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match token_proxy_app.stop_proxy().await {
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
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<(), String> {
    tray_state.mark_quit();
    token_proxy_app.stop_proxy().await.map(|_| ())
}

#[tauri::command]
pub async fn proxy_restart(
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match token_proxy_app.restart_proxy().await {
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
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    tray_state: tauri::State<'_, tray::TrayState>,
) -> Result<ProxyServiceStatus, String> {
    match token_proxy_app.reload_proxy().await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_catalog_url_preserves_existing_version_root() {
        let url = build_model_catalog_url("openai", "https://example.com/openai/v1", "")
            .expect("model catalog url");

        assert_eq!(url.as_str(), "https://example.com/openai/v1/models");
    }

    #[test]
    fn gemini_model_catalog_url_uses_query_key() {
        let url = build_model_catalog_url("gemini", "https://example.com", "secret-key")
            .expect("model catalog url");

        assert_eq!(url.path(), "/v1beta/models");
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "key").unwrap().1,
            "secret-key"
        );
    }

    #[test]
    fn model_catalog_parser_normalizes_all_supported_shapes() {
        let body = json!({
            "data": [{ "id": "gpt-5.4" }],
            "models": [
                { "name": "models/gemini-3.1-pro" },
                { "id": " gpt-5.4 " }
            ],
            "modelNames": ["claude-sonnet-4.6"]
        });

        assert_eq!(
            extract_upstream_model_ids(&body),
            vec!["claude-sonnet-4.6", "gemini-3.1-pro", "gpt-5.4"]
        );
    }
}
