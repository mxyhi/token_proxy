use serde::Serialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tauri::AppHandle;
use tauri::Manager;

use crate::proxy::config::ProxyConfigFile;

const CODEX_DISABLE_RESPONSE_STORAGE: bool = true;
const CODEX_MODEL: &str = "gpt-5.2-codex";
const CODEX_MODEL_PROVIDER: &str = "token_proxy";
const CODEX_MODEL_REASONING_EFFORT: &str = "xhigh";
const CODEX_NETWORK_ACCESS: &str = "enabled";
const CODEX_PREFERRED_AUTH_METHOD: &str = "apikey";
const CODEX_PROVIDER_NAME: &str = "token_proxy";
const CODEX_PROVIDER_REQUIRES_OPENAI_AUTH: bool = true;
const CODEX_PROVIDER_WIRE_API: &str = "responses";

const OPENCODE_PROVIDER_ID: &str = "token_proxy";
const OPENCODE_PROVIDER_NPM: &str = "@ai-sdk/openai-compatible";
const OPENCODE_PROVIDER_NAME: &str = "Token Proxy";
const OPENCODE_SCHEMA_URL: &str = "https://opencode.ai/config.json";

#[derive(Clone, Serialize)]
pub(crate) struct ClientSetupInfo {
    pub(crate) proxy_http_base_url: String,

    pub(crate) claude_settings_path: String,
    pub(crate) claude_base_url: String,
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

    pub(crate) opencode_config_path: String,
    pub(crate) opencode_auth_path: String,
    pub(crate) opencode_provider_id: String,
    pub(crate) opencode_provider_base_url: String,
    pub(crate) opencode_models: Vec<String>,
    pub(crate) opencode_api_key_configured: bool,
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
    let opencode_config_path = resolve_opencode_config_path(&app)?;
    let opencode_auth_path = resolve_opencode_auth_path(&app)?;
    let has_local_key = config
        .local_api_key
        .as_ref()
        .is_some_and(|key| !key.trim().is_empty());
    let openai_compat_base_url = build_openai_compat_base_url(&proxy_http_base_url);
    let opencode_models = collect_opencode_models(&config);

    Ok(ClientSetupInfo {
        proxy_http_base_url: proxy_http_base_url.clone(),
        claude_settings_path: claude_settings_path.to_string_lossy().to_string(),
        claude_base_url: proxy_http_base_url.clone(),
        claude_auth_token_configured: has_local_key,
        codex_config_path: codex_config_path.to_string_lossy().to_string(),
        codex_auth_path: codex_auth_path.to_string_lossy().to_string(),
        codex_disable_response_storage: CODEX_DISABLE_RESPONSE_STORAGE,
        codex_model: CODEX_MODEL.to_string(),
        codex_model_provider: CODEX_MODEL_PROVIDER.to_string(),
        codex_model_reasoning_effort: CODEX_MODEL_REASONING_EFFORT.to_string(),
        codex_network_access: CODEX_NETWORK_ACCESS.to_string(),
        codex_preferred_auth_method: CODEX_PREFERRED_AUTH_METHOD.to_string(),
        codex_provider_base_url: openai_compat_base_url.clone(),
        codex_provider_name: CODEX_PROVIDER_NAME.to_string(),
        codex_provider_requires_openai_auth: CODEX_PROVIDER_REQUIRES_OPENAI_AUTH,
        codex_provider_wire_api: CODEX_PROVIDER_WIRE_API.to_string(),
        codex_api_key_configured: has_local_key,
        opencode_config_path: opencode_config_path.to_string_lossy().to_string(),
        opencode_auth_path: opencode_auth_path.to_string_lossy().to_string(),
        opencode_provider_id: OPENCODE_PROVIDER_ID.to_string(),
        opencode_provider_base_url: openai_compat_base_url,
        opencode_models,
        opencode_api_key_configured: has_local_key,
    })
}

pub(crate) async fn write_claude_code_settings(
    app: AppHandle,
) -> Result<ClientConfigWriteResult, String> {
    let config = load_proxy_config(&app).await?;
    let proxy_http_base_url = build_proxy_http_base_url(&config)?;
    let settings_path = resolve_claude_settings_path(&app)?;

    // Claude Code 支持在 ~/.claude/settings.json 的 `env` 字段里持久化环境变量，
    // 这样无需改 shell profile 就能全局生效。
    //
    // - ANTHROPIC_BASE_URL: 指向本地代理（不带 /v1）
    // - ANTHROPIC_AUTH_TOKEN: 用于 Authorization: Bearer <token>，与 Token Proxy 的 local_api_key 匹配
    let mut root = read_json_object_or_default(&settings_path).await?;
    let env = ensure_json_object_field(&mut root, "env")?;
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::Value::String(proxy_http_base_url),
    );
    match config
        .local_api_key
        .as_ref()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
    {
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

pub(crate) async fn write_codex_config(app: AppHandle) -> Result<ClientConfigWriteResult, String> {
    let config = load_proxy_config(&app).await?;
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

    doc["disable_response_storage"] = toml_edit::value(CODEX_DISABLE_RESPONSE_STORAGE);
    doc["model"] = toml_edit::value(CODEX_MODEL);
    doc["model_provider"] = toml_edit::value(CODEX_MODEL_PROVIDER);
    doc["model_reasoning_effort"] = toml_edit::value(CODEX_MODEL_REASONING_EFFORT);
    doc["network_access"] = toml_edit::value(CODEX_NETWORK_ACCESS);
    doc["preferred_auth_method"] = toml_edit::value(CODEX_PREFERRED_AUTH_METHOD);

    ensure_toml_table_path(&mut doc, &["model_providers"])?;
    ensure_toml_table_path(&mut doc, &["model_providers", CODEX_MODEL_PROVIDER])?;

    doc["model_providers"][CODEX_MODEL_PROVIDER]["base_url"] =
        toml_edit::value(codex_provider_base_url);
    doc["model_providers"][CODEX_MODEL_PROVIDER]["name"] = toml_edit::value(CODEX_PROVIDER_NAME);
    doc["model_providers"][CODEX_MODEL_PROVIDER]["requires_openai_auth"] =
        toml_edit::value(CODEX_PROVIDER_REQUIRES_OPENAI_AUTH);
    doc["model_providers"][CODEX_MODEL_PROVIDER]["wire_api"] =
        toml_edit::value(CODEX_PROVIDER_WIRE_API);

    if let Some(table) = doc["model_providers"][CODEX_MODEL_PROVIDER].as_table_mut() {
        table.remove("experimental_bearer_token");
    }

    write_text_with_backup(&config_path, doc.to_string()).await?;

    let mut auth_root = read_json_object_or_default(&auth_path).await?;
    match config
        .local_api_key
        .as_ref()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
    {
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

pub(crate) async fn write_opencode_config(
    app: AppHandle,
) -> Result<ClientConfigWriteResult, String> {
    let config = load_proxy_config(&app).await?;
    let proxy_http_base_url = build_proxy_http_base_url(&config)?;
    let opencode_config_path = resolve_opencode_config_path(&app)?;
    let opencode_auth_path = resolve_opencode_auth_path(&app)?;
    let opencode_provider_base_url = build_openai_compat_base_url(&proxy_http_base_url);
    let opencode_models = collect_opencode_models(&config);

    if opencode_models.is_empty() {
        return Err(
            "OpenCode requires at least one model for custom providers. Add an exact model mapping (no '*') in Upstreams, then try again."
                .to_string(),
        );
    }

    let mut root = read_json_object_or_default(&opencode_config_path).await?;
    root.insert(
        "$schema".to_string(),
        serde_json::Value::String(OPENCODE_SCHEMA_URL.to_string()),
    );
    let providers = ensure_json_object_field(&mut root, "provider")?;
    providers.insert(
        OPENCODE_PROVIDER_ID.to_string(),
        build_opencode_provider_config(&opencode_provider_base_url, &opencode_models),
    );
    write_json_with_backup(&opencode_config_path, &serde_json::Value::Object(root)).await?;

    let mut auth_root = read_json_object_or_default(&opencode_auth_path).await?;
    match config
        .local_api_key
        .as_ref()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
    {
        Some(token) => {
            auth_root.insert(
                OPENCODE_PROVIDER_ID.to_string(),
                serde_json::Value::Object(serde_json::Map::from_iter([
                    (
                        "type".to_string(),
                        serde_json::Value::String("api".to_string()),
                    ),
                    (
                        "key".to_string(),
                        serde_json::Value::String(token.to_string()),
                    ),
                ])),
            );
        }
        None => {
            auth_root.remove(OPENCODE_PROVIDER_ID);
        }
    }
    write_json_with_backup(&opencode_auth_path, &serde_json::Value::Object(auth_root)).await?;

    Ok(ClientConfigWriteResult {
        paths: vec![
            opencode_config_path.to_string_lossy().to_string(),
            opencode_auth_path.to_string_lossy().to_string(),
        ],
    })
}

async fn load_proxy_config(app: &AppHandle) -> Result<ProxyConfigFile, String> {
    let paths = app.state::<Arc<token_proxy_core::paths::TokenProxyPaths>>();
    Ok(crate::proxy::config::read_config(paths.inner().as_ref())
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

fn resolve_opencode_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("OPENCODE_CONFIG").map(PathBuf::from) {
        return Ok(path);
    }

    let dir = resolve_opencode_config_dir(app)?;
    let jsonc = dir.join("opencode.jsonc");
    let json = dir.join("opencode.json");
    if jsonc.exists() {
        return Ok(jsonc);
    }
    if json.exists() {
        return Ok(json);
    }
    Ok(jsonc)
}

fn resolve_opencode_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("OPENCODE_CONFIG_DIR").map(PathBuf::from) {
        return Ok(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        return Ok(dir.join("opencode"));
    }
    Ok(resolve_home_dir(app)?.join(".config").join("opencode"))
}

fn resolve_opencode_auth_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(resolve_opencode_data_dir(app)?.join("auth.json"))
}

fn resolve_opencode_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        return Ok(dir.join("opencode"));
    }
    Ok(resolve_home_dir(app)?
        .join(".local")
        .join("share")
        .join("opencode"))
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

fn collect_opencode_models(config: &ProxyConfigFile) -> Vec<String> {
    let mut models = HashSet::new();
    for upstream in &config.upstreams {
        for pattern in upstream.model_mappings.keys() {
            let value = pattern.trim();
            if value.is_empty() {
                continue;
            }
            if value.contains('*') {
                continue;
            }
            models.insert(value.to_string());
        }
    }
    let mut list = models.into_iter().collect::<Vec<_>>();
    list.sort();
    list
}

fn build_opencode_provider_config(base_url: &str, models: &[String]) -> serde_json::Value {
    let mut models_object = serde_json::Map::new();
    for model in models {
        models_object.insert(
            model.to_string(),
            serde_json::Value::Object(serde_json::Map::from_iter([(
                "name".to_string(),
                serde_json::Value::String(model.to_string()),
            )])),
        );
    }

    serde_json::Value::Object(serde_json::Map::from_iter([
        (
            "npm".to_string(),
            serde_json::Value::String(OPENCODE_PROVIDER_NPM.to_string()),
        ),
        (
            "name".to_string(),
            serde_json::Value::String(OPENCODE_PROVIDER_NAME.to_string()),
        ),
        (
            "options".to_string(),
            serde_json::Value::Object(serde_json::Map::from_iter([(
                "baseURL".to_string(),
                serde_json::Value::String(base_url.to_string()),
            )])),
        ),
        (
            "models".to_string(),
            serde_json::Value::Object(models_object),
        ),
    ]))
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
    let sanitized = crate::jsonc::sanitize_jsonc(&text);
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
