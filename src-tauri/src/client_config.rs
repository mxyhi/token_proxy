use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tauri::AppHandle;
use tauri::Manager;

use token_proxy_config::ProxyConfigFile;

const CODEX_DISABLE_RESPONSE_STORAGE: bool = true;
const CLAUDE_MODEL: &str = "claude-sonnet-4-6";
const CODEX_MODEL: &str = "gpt-5.5";
const CODEX_DEFAULT_MODEL_PROVIDER: &str = "token_proxy";
const CODEX_MODEL_REASONING_EFFORT: &str = "xhigh";
const CODEX_NETWORK_ACCESS: &str = "enabled";
const CODEX_PREFERRED_AUTH_METHOD: &str = "apikey";
const CODEX_PROVIDER_NAME: &str = "token_proxy";
const CODEX_PROVIDER_REQUIRES_OPENAI_AUTH: bool = true;
const CODEX_PROVIDER_WIRE_API: &str = "responses";

#[derive(Clone, Serialize)]
pub(crate) struct ClientSetupInfo {
    pub(crate) proxy_http_base_url: String,
    pub(crate) local_auth_required: bool,
    pub(crate) enabled_api_keys: Vec<ClientKeyOption>,

    pub(crate) claude_settings_path: String,
    pub(crate) claude_base_url: String,
    pub(crate) claude_model: String,
    pub(crate) claude_auth_token_configured: bool,

    pub(crate) codex_config_path: String,
    pub(crate) codex_auth_path: String,

    pub(crate) codex_disable_response_storage: bool,
    pub(crate) codex_model: String,
    pub(crate) codex_model_provider: String,
    pub(crate) codex_model_reasoning_effort: String,
    pub(crate) codex_network_access: String,
    pub(crate) codex_preferred_auth_method: String,
    pub(crate) codex_provider_base_url: String,
    pub(crate) codex_provider_name: String,
    pub(crate) codex_provider_requires_openai_auth: bool,
    pub(crate) codex_provider_wire_api: String,
    pub(crate) codex_api_key_configured: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct ClientKeyOption {
    pub(crate) id: String,
    pub(crate) name: String,
}

/// 先校验 Key，再触碰客户端文件。多 Key 禁止默认取第一条，禁用/失效 ID 禁止替换。
fn selected_local_key(
    config: &ProxyConfigFile,
    key_id: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(id) = key_id {
        return config
            .local_api_keys
            .iter()
            .find(|key| key.id == id && key.enabled)
            .map(|key| Some(key.key.clone()))
            .ok_or_else(|| "所选 API Key 已删除或禁用，请重新选择。".to_string());
    }
    if config.local_api_keys.is_empty() {
        return Ok(None);
    }
    let mut enabled = config.local_api_keys.iter().filter(|key| key.enabled);
    let first = enabled
        .next()
        .ok_or_else(|| "所有 API Key 均已禁用。".to_string())?;
    if enabled.next().is_some() {
        return Err("存在多个启用的 API Key，请明确选择。".to_string());
    }
    Ok(Some(first.key.clone()))
}

#[derive(Clone, Serialize)]
pub(crate) struct ClientConfigWriteResult {
    pub(crate) paths: Vec<String>,
}

pub(crate) async fn preview(app: AppHandle) -> Result<ClientSetupInfo, String> {
    let config = load_proxy_config(&app).await?;
    let proxy_http_base_url = build_proxy_http_base_url(&config)?;
    let claude_settings_path = resolve_claude_settings_path(&app)?;
    let codex_config_path = resolve_codex_config_path(&app)?;
    let codex_auth_path = resolve_codex_auth_path(&app)?;
    let codex_config_input = read_text_or_empty(&codex_config_path).await?;
    let (codex_model_provider, codex_provider_name) =
        resolve_codex_target_provider_and_name(&codex_config_input);
    let has_local_key = config.local_api_keys.iter().any(|key| key.enabled);
    let openai_compat_base_url = build_openai_compat_base_url(&proxy_http_base_url);

    Ok(ClientSetupInfo {
        proxy_http_base_url: proxy_http_base_url.clone(),
        local_auth_required: !config.local_api_keys.is_empty(),
        enabled_api_keys: config
            .local_api_keys
            .iter()
            .filter(|key| key.enabled)
            .map(|key| ClientKeyOption {
                id: key.id.clone(),
                name: key.name.clone(),
            })
            .collect(),
        claude_settings_path: claude_settings_path.to_string_lossy().to_string(),
        claude_base_url: proxy_http_base_url.clone(),
        claude_model: CLAUDE_MODEL.to_string(),
        claude_auth_token_configured: has_local_key,
        codex_config_path: codex_config_path.to_string_lossy().to_string(),
        codex_auth_path: codex_auth_path.to_string_lossy().to_string(),
        codex_disable_response_storage: CODEX_DISABLE_RESPONSE_STORAGE,
        codex_model: CODEX_MODEL.to_string(),
        codex_model_provider,
        codex_model_reasoning_effort: CODEX_MODEL_REASONING_EFFORT.to_string(),
        codex_network_access: CODEX_NETWORK_ACCESS.to_string(),
        codex_preferred_auth_method: CODEX_PREFERRED_AUTH_METHOD.to_string(),
        codex_provider_base_url: openai_compat_base_url.clone(),
        codex_provider_name,
        codex_provider_requires_openai_auth: CODEX_PROVIDER_REQUIRES_OPENAI_AUTH,
        codex_provider_wire_api: CODEX_PROVIDER_WIRE_API.to_string(),
        codex_api_key_configured: has_local_key,
    })
}

pub(crate) async fn write_claude_code_settings(
    app: AppHandle,
    key_id: Option<String>,
) -> Result<ClientConfigWriteResult, String> {
    let config = load_proxy_config(&app).await?;
    let selected_key = selected_local_key(&config, key_id.as_deref())?;
    let proxy_http_base_url = build_proxy_http_base_url(&config)?;
    let settings_path = resolve_claude_settings_path(&app)?;

    // Claude Code 支持在 ~/.claude/settings.json 的 `env` 字段里持久化环境变量，
    // 这样无需改 shell profile 就能全局生效。
    //
    // - ANTHROPIC_BASE_URL: 指向本地代理（不带 /v1）
    // - ANTHROPIC_MODEL: 固定 Claude Code 默认编码模型，避免依赖客户端账号档位推断
    // - ANTHROPIC_AUTH_TOKEN: 用于 Authorization: Bearer <token>，与 Token Proxy 的所选本地访问 Key 匹配
    let mut root = read_json_object_or_default(&settings_path).await?;
    let env = ensure_json_object_field(&mut root, "env")?;
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::Value::String(proxy_http_base_url),
    );
    env.insert(
        "ANTHROPIC_MODEL".to_string(),
        serde_json::Value::String(CLAUDE_MODEL.to_string()),
    );
    match selected_key.as_deref() {
        Some(token) => {
            env.insert(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                serde_json::Value::String(token.to_string()),
            );
        }
        None => {
            // 若本地鉴权被关闭，避免继续给 Claude Code 写入 Authorization，防止误传到上游。
            env.remove("ANTHROPIC_AUTH_TOKEN");
        }
    }
    write_json_with_backup(&settings_path, &serde_json::Value::Object(root)).await?;

    Ok(ClientConfigWriteResult {
        paths: vec![settings_path.to_string_lossy().to_string()],
    })
}

pub(crate) async fn write_codex_config(
    app: AppHandle,
    key_id: Option<String>,
) -> Result<ClientConfigWriteResult, String> {
    let config = load_proxy_config(&app).await?;
    let selected_key = selected_local_key(&config, key_id.as_deref())?;
    let proxy_http_base_url = build_proxy_http_base_url(&config)?;
    let config_path = resolve_codex_config_path(&app)?;
    let auth_path = resolve_codex_auth_path(&app)?;
    let codex_provider_base_url = build_openai_compat_base_url(&proxy_http_base_url);

    // Codex 默认 config 路径为 $CODEX_HOME/config.toml，其中 CODEX_HOME 默认是 ~/.codex。
    // 为了让 Codex 直接走本地代理，我们写入固定的 token_proxy provider 配置，并尽量不动其他字段。
    //
    // - base_url = http://127.0.0.1:<port>/v1
    // - preferred_auth_method = apikey
    // - token 写入 $CODEX_HOME/auth.json 的 OPENAI_API_KEY（而非 experimental_bearer_token）
    let input = read_text_or_empty(&config_path).await?;
    let mut doc = toml_edit::DocumentMut::from_str(&input)
        .map_err(|err| format!("Failed to parse Codex config.toml: {err}"))?;
    let (codex_model_provider, codex_provider_name) =
        resolve_codex_target_provider_and_name_from_doc(&doc);
    apply_codex_proxy_settings(
        &mut doc,
        &codex_model_provider,
        &codex_provider_name,
        &codex_provider_base_url,
    )?;

    write_text_with_backup(&config_path, doc.to_string()).await?;

    let mut auth_root = read_json_object_or_default(&auth_path).await?;
    match selected_key.as_deref() {
        Some(token) => {
            auth_root.insert(
                "OPENAI_API_KEY".to_string(),
                serde_json::Value::String(token.to_string()),
            );
        }
        None => {
            auth_root.remove("OPENAI_API_KEY");
        }
    }
    write_json_with_backup(&auth_path, &serde_json::Value::Object(auth_root)).await?;

    Ok(ClientConfigWriteResult {
        paths: vec![
            config_path.to_string_lossy().to_string(),
            auth_path.to_string_lossy().to_string(),
        ],
    })
}

async fn load_proxy_config(app: &AppHandle) -> Result<ProxyConfigFile, String> {
    let paths = app.state::<Arc<token_proxy_account_store::paths::TokenProxyPaths>>();
    Ok(token_proxy_config::read_config(paths.inner().as_ref())
        .await?
        .config)
}

fn resolve_home_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(dir) = app.path().home_dir() {
        return Ok(dir);
    }
    if let Some(dir) = std::env::var_os("HOME").map(PathBuf::from) {
        return Ok(dir);
    }
    if cfg!(target_os = "windows") {
        if let Some(dir) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            return Ok(dir);
        }
    }
    Err("Failed to resolve user home directory.".to_string())
}

fn resolve_claude_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(resolve_home_dir(app)?.join(".claude").join("settings.json"))
}

fn resolve_codex_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(resolve_codex_home_dir(app)?.join("config.toml"))
}

fn resolve_codex_auth_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(resolve_codex_home_dir(app)?.join("auth.json"))
}

fn resolve_codex_home_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let home = resolve_home_dir(app)?;
    Ok(std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex")))
}

fn build_proxy_http_base_url(config: &ProxyConfigFile) -> Result<String, String> {
    let raw_host = config.host.trim();
    let host = match raw_host {
        "" | "0.0.0.0" | "::" => "127.0.0.1",
        other => other,
    };

    // IPv6 URL host 需要用方括号包裹（http://[::1]:9208）。
    let url_host = if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    Ok(format!("http://{url_host}:{}", config.port))
}

fn build_openai_compat_base_url(proxy_http_base_url: &str) -> String {
    format!("{proxy_http_base_url}/v1")
}

fn resolve_codex_target_provider_and_name(input: &str) -> (String, String) {
    let Ok(doc) = toml_edit::DocumentMut::from_str(input) else {
        return default_codex_provider_identity();
    };
    resolve_codex_target_provider_and_name_from_doc(&doc)
}

fn resolve_codex_target_provider_and_name_from_doc(
    doc: &toml_edit::DocumentMut,
) -> (String, String) {
    let provider = doc
        .as_table()
        .get("model_provider")
        .and_then(toml_edit::Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| CODEX_DEFAULT_MODEL_PROVIDER.to_string());
    let name = doc
        .as_table()
        .get("model_providers")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get(&provider))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get("name"))
        .and_then(toml_edit::Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| default_codex_provider_name(&provider));
    (provider, name)
}

fn default_codex_provider_identity() -> (String, String) {
    (
        CODEX_DEFAULT_MODEL_PROVIDER.to_string(),
        CODEX_PROVIDER_NAME.to_string(),
    )
}

fn default_codex_provider_name(provider: &str) -> String {
    if provider == CODEX_DEFAULT_MODEL_PROVIDER {
        return CODEX_PROVIDER_NAME.to_string();
    }
    provider.to_string()
}

fn apply_codex_proxy_settings(
    doc: &mut toml_edit::DocumentMut,
    codex_model_provider: &str,
    codex_provider_name: &str,
    codex_provider_base_url: &str,
) -> Result<(), String> {
    doc["disable_response_storage"] = toml_edit::value(CODEX_DISABLE_RESPONSE_STORAGE);
    doc["model"] = toml_edit::value(CODEX_MODEL);
    doc["model_provider"] = toml_edit::value(codex_model_provider);
    doc["model_reasoning_effort"] = toml_edit::value(CODEX_MODEL_REASONING_EFFORT);
    doc["network_access"] = toml_edit::value(CODEX_NETWORK_ACCESS);
    doc["preferred_auth_method"] = toml_edit::value(CODEX_PREFERRED_AUTH_METHOD);

    ensure_toml_table_path(doc, &["model_providers"])?;
    ensure_toml_table_path(doc, &["model_providers", codex_model_provider])?;

    doc["model_providers"][codex_model_provider]["base_url"] =
        toml_edit::value(codex_provider_base_url);
    doc["model_providers"][codex_model_provider]["name"] = toml_edit::value(codex_provider_name);
    doc["model_providers"][codex_model_provider]["requires_openai_auth"] =
        toml_edit::value(CODEX_PROVIDER_REQUIRES_OPENAI_AUTH);
    doc["model_providers"][codex_model_provider]["wire_api"] =
        toml_edit::value(CODEX_PROVIDER_WIRE_API);

    if let Some(table) = doc["model_providers"][codex_model_provider].as_table_mut() {
        table.remove("experimental_bearer_token");
    }

    Ok(())
}

async fn read_text_or_empty(path: &Path) -> Result<String, String> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("Failed to read {}: {err}", path.display())),
    }
}

async fn read_json_object_or_default(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let text = read_text_or_empty(path).await?;
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    let sanitized = token_proxy_config::sanitize_jsonc(&text);
    let mut value: serde_json::Value = serde_json::from_str(&sanitized)
        .map_err(|err| format!("Failed to parse {}: {err}", path.display()))?;
    let Some(object) = value.as_object_mut() else {
        return Err(format!("{} must be a JSON object.", path.display()));
    };
    Ok(object.clone())
}

fn ensure_json_object_field<'a>(
    root: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, String> {
    let value = root
        .entry(key.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    value
        .as_object_mut()
        .ok_or_else(|| format!("{} must be a JSON object.", key))
}

async fn write_json_with_backup(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "Invalid path.".to_string())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;

    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        let backup_path = build_backup_path(path);
        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
        tokio::fs::write(&backup_path, contents)
            .await
            .map_err(|err| format!("Failed to write backup {}: {err}", backup_path.display()))?;
    }

    let mut output = serde_json::to_string_pretty(value)
        .map_err(|err| format!("Failed to serialize JSON: {err}"))?;
    output.push('\n');
    tokio::fs::write(path, output)
        .await
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    Ok(())
}

async fn write_text_with_backup(path: &Path, contents: String) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "Invalid path.".to_string())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;

    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        let backup_path = build_backup_path(path);
        let old = tokio::fs::read_to_string(path)
            .await
            .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
        tokio::fs::write(&backup_path, old)
            .await
            .map_err(|err| format!("Failed to write backup {}: {err}", backup_path.display()))?;
    }

    let output = if contents.ends_with('\n') {
        contents
    } else {
        format!("{contents}\n")
    };
    tokio::fs::write(path, output)
        .await
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply_codex_proxy_settings, resolve_codex_target_provider_and_name};
    use std::str::FromStr;
    use toml_edit::DocumentMut;

    #[test]
    fn resolve_codex_target_provider_preserves_existing_model_provider() {
        let existing = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
"#;

        let (provider, name) = resolve_codex_target_provider_and_name(existing);

        assert_eq!(provider, "openai");
        assert_eq!(name, "OpenAI");
    }

    #[test]
    fn resolve_codex_target_provider_falls_back_to_token_proxy_for_empty_config() {
        let (provider, name) = resolve_codex_target_provider_and_name("");

        assert_eq!(provider, "token_proxy");
        assert_eq!(name, "token_proxy");
    }

    #[test]
    fn apply_codex_proxy_settings_keeps_existing_provider_id() {
        let input = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
"#;
        let mut doc = DocumentMut::from_str(input).expect("parse config");

        apply_codex_proxy_settings(&mut doc, "openai", "OpenAI", "http://127.0.0.1:9208/v1")
            .expect("apply codex proxy settings");

        assert_eq!(doc["model_provider"].as_str(), Some("openai"));
        assert_eq!(
            doc["model_providers"]["openai"]["base_url"].as_str(),
            Some("http://127.0.0.1:9208/v1")
        );
        let token_proxy_provider = doc
            .as_table()
            .get("model_providers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|table| table.get("token_proxy"));
        assert!(token_proxy_provider.is_none());
    }

    #[test]
    fn apply_codex_proxy_settings_writes_current_default_model() {
        let mut doc = DocumentMut::new();

        apply_codex_proxy_settings(
            &mut doc,
            "token_proxy",
            "token_proxy",
            "http://127.0.0.1:9208/v1",
        )
        .expect("apply codex proxy settings");

        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
    }
}

fn build_backup_path(path: &Path) -> PathBuf {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
    {
        Some(extension) => path.with_extension(format!("{extension}.token_proxy.bak")),
        None => path.with_extension("token_proxy.bak"),
    }
}

fn ensure_toml_table_path(doc: &mut toml_edit::DocumentMut, path: &[&str]) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }

    // toml_edit 的索引访问在 path 中间节点不是 table 时会产生不易读的错误；
    // 这里显式确保每一段都是 table。
    let mut current: &mut toml_edit::Item = doc.as_item_mut();
    for segment in path {
        if !current.is_table() {
            *current = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let table = current
            .as_table_mut()
            .ok_or_else(|| "Failed to build TOML table path.".to_string())?;
        current = table
            .entry(*segment)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    }

    Ok(())
}

#[cfg(test)]
mod local_key_tests {
    use super::*;
    use token_proxy_config::{LocalApiKey, LocalApiKeyScope};

    #[test]
    fn client_key_selection_requires_explicit_choice_and_never_substitutes() {
        let mut config = ProxyConfigFile::default();
        assert_eq!(selected_local_key(&config, None).unwrap(), None);
        assert!(selected_local_key(&config, Some("missing")).is_err());
        let key = |id: &str| LocalApiKey {
            id: id.into(),
            name: id.into(),
            key: format!("secret-{id}"),
            enabled: true,
            scope: LocalApiKeyScope::Auto,
        };
        config.local_api_keys = vec![key("a")];
        assert_eq!(
            selected_local_key(&config, None).unwrap().as_deref(),
            Some("secret-a")
        );
        config.local_api_keys.push(key("b"));
        assert!(selected_local_key(&config, None).is_err());
        assert_eq!(
            selected_local_key(&config, Some("b")).unwrap().as_deref(),
            Some("secret-b")
        );
        config.local_api_keys[1].enabled = false;
        assert!(selected_local_key(&config, Some("b")).is_err());
        config.local_api_keys[0].enabled = false;
        assert!(selected_local_key(&config, None).is_err());
    }
}
