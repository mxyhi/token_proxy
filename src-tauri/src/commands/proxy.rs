use std::{error::Error, time::Duration};

use crate::tray;
use serde_json::Value;
use token_proxy_account_store::oauth_util::normalize_proxy_url;
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
    let app_proxy = token_proxy_app.app_proxy().read().await.clone();
    fetch_model_catalog(
        &provider,
        &base_url,
        &api_key,
        proxy_url.as_deref(),
        app_proxy.as_deref(),
    )
    .await
}

async fn fetch_model_catalog(
    provider: &str,
    base_url: &str,
    api_key: &str,
    proxy_url: Option<&str>,
    app_proxy: Option<&str>,
) -> Result<Vec<String>, String> {
    let api_key = api_key.trim();
    let url = build_model_catalog_url(&provider, &base_url, api_key)?;
    tracing::debug!(provider = %provider, "fetching upstream model catalog");

    let proxy_url = resolve_catalog_proxy(proxy_url, app_proxy)?;
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(30));
    if let Some(proxy_url) = &proxy_url {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy_url).map_err(|_| "渠道代理 URL 无效。".to_string())?);
    } else {
        // 与实际转发保持一致：渠道代理留空即直连，不继承系统代理。
        builder = builder.no_proxy();
    }
    let client = builder
        .build()
        .map_err(|_| "创建模型列表 HTTP 客户端失败。".to_string())?;
    let request = apply_model_catalog_auth(client.get(url), &provider, api_key);

    let response = request
        .send()
        .await
        .map_err(|err| {
            // Gemini key 位于 query 中，只展示脱敏后的错误分类。
            let message = model_catalog_request_error(&err);
            tracing::warn!(provider = %provider, error = %message, proxy_enabled = proxy_url.is_some(), "failed to fetch upstream model catalog");
            message
        })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::warn!(provider = %provider, %status, "upstream model catalog returned an error");
        return Err(format!("渠道模型列表返回错误: {status}"));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|err| format!("解析渠道模型列表失败: {}", err.without_url()))?;
    let models = extract_upstream_model_ids(&body);

    if models.is_empty() {
        return Err("渠道未返回可用模型。".to_string());
    }

    tracing::info!(provider = %provider, model_count = models.len(), "fetched upstream model catalog");
    Ok(models)
}

fn resolve_catalog_proxy(
    proxy_url: Option<&str>,
    app_proxy: Option<&str>,
) -> Result<Option<String>, String> {
    let proxy_url = proxy_url.map(str::trim).filter(|value| !value.is_empty());
    let effective = match proxy_url {
        Some("$app_proxy_url") => Some(
            app_proxy
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "渠道使用应用代理，但应用代理尚未配置。".to_string())?,
        ),
        explicit => explicit,
    };
    normalize_proxy_url(effective).map_err(|_| "渠道代理 URL 无效或协议不受支持。".to_string())
}

fn model_catalog_request_error(error: &reqwest::Error) -> String {
    // 只用底层错误分类，不能回显包含 Gemini query key 或代理凭据的错误链。
    let mut source = error.source();
    let mut tls = false;
    let mut dns = false;
    while let Some(cause) = source {
        let message = cause.to_string().to_ascii_lowercase();
        tls |= ["tls", "ssl", "certificate"]
            .iter()
            .any(|marker| message.contains(marker));
        dns |= ["dns", "name resolution"]
            .iter()
            .any(|marker| message.contains(marker));
        source = cause.source();
    }
    let reason = if error.is_timeout() {
        "请求超时，请检查渠道地址和代理连通性。"
    } else if tls {
        "TLS 握手或证书校验失败，请检查渠道地址和代理。"
    } else if dns {
        "DNS 解析失败，请检查渠道域名和代理。"
    } else if error.is_connect() {
        "连接失败，请检查渠道地址与代理是否可用。"
    } else if error.is_builder() {
        "请求配置无效，请检查渠道地址和 API Key 格式。"
    } else {
        "网络请求失败，请检查渠道地址和代理。"
    };
    format!("请求渠道模型列表失败: {reason}")
}

fn build_model_catalog_url(provider: &str, base_url: &str, api_key: &str) -> Result<Url, String> {
    let target_root = match provider {
        "openai" | "openai-response" | "azuregpt" | "anthropic" => "/v1/models",
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
    fn model_catalog_proxy_matches_channel_configuration() {
        let app_proxy = Some(" socks5h://127.0.0.1:7891 ");
        for direct in [None, Some(""), Some(" ")] {
            assert_eq!(resolve_catalog_proxy(direct, app_proxy).unwrap(), None);
        }
        assert_eq!(
            resolve_catalog_proxy(Some(" $app_proxy_url "), app_proxy).unwrap(),
            Some("socks5h://127.0.0.1:7891".to_string())
        );
        assert_eq!(
            resolve_catalog_proxy(Some(" http://127.0.0.1:7890 "), app_proxy).unwrap(),
            Some("http://127.0.0.1:7890".to_string())
        );
        for missing in [None, Some(""), Some(" ")] {
            assert!(resolve_catalog_proxy(Some("$app_proxy_url"), missing).is_err());
        }
        for invalid in ["invalid", "file:///tmp/proxy"] {
            assert!(resolve_catalog_proxy(Some(invalid), app_proxy).is_err());
        }
    }

    #[tokio::test]
    async fn model_catalog_connection_error_does_not_leak_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let proxy = format!("http://proxy-user:proxy-password@{address}");
        let error = fetch_model_catalog(
            "gemini",
            "https://catalog.invalid",
            "secret-key",
            Some(&proxy),
            None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("连接失败"), "{error}");
        for secret in [
            "secret-key",
            "proxy-user",
            "proxy-password",
            "catalog.invalid",
        ] {
            assert!(!error.contains(secret), "{error}");
        }
    }

    #[tokio::test]
    async fn model_catalog_uses_channel_proxy() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for configured in ["$app_proxy_url", "explicit"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let size = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]);
                assert!(request.starts_with("GET http://catalog.invalid/v1/models HTTP/1.1"));
                assert!(request
                    .to_lowercase()
                    .contains("authorization: bearer test-key"));
                let body = r#"{"data":[{"id":"proxy-model"}]}"#;
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            });
            let channel_proxy = if configured == "explicit" {
                &proxy
            } else {
                configured
            };
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                fetch_model_catalog(
                    "openai",
                    "http://catalog.invalid",
                    "test-key",
                    Some(channel_proxy),
                    Some(&proxy),
                ),
            )
            .await
            .expect("catalog request timed out");
            assert_eq!(result.unwrap(), vec!["proxy-model"]);
            server.await.unwrap();
        }
    }

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
