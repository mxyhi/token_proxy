use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use token_proxy_account_store::paths::TokenProxyPaths;
use tokio::io::AsyncWriteExt;

use super::migrate::migrate_config_json;
use super::ProxyConfigFile;

/// 测试专用：临时文件已 flush/sync 后、rename 前注入失败，验证旧主文件保持。
static FAIL_RENAME_AFTER_TEMP_WRITE: AtomicBool = AtomicBool::new(false);

const DEFAULT_CONFIG_HEADER: &str = concat!(
    "// Token Proxy config (JSONC). Comments and trailing commas are supported.\n",
    "// log_level (optional): silent|error|warn|info|debug|trace. Default: silent.\n",
    "// stream_first_output_timeout_secs (optional): stream first client-visible output timeout in seconds. Minimum: 1. Default: 60.\n",
    "// sync_response_timeout_secs (optional): non-stream full response timeout in seconds. Minimum: 1. Default: 300.\n",
    "// codex_session_scoped_cooldown_enabled (optional): isolate Codex OpenAI Responses cooldown by session_id. Default: false.\n",
    "// xai_inject_x_search (optional): inject xAI native x_search into /v1/responses. Default: false.\n",
    "// upstream_strategy (optional): { order: \"fill_first\"|\"round_robin\", dispatch: { type: \"serial\"|\"hedged\"|\"race\", ... } }.\n",
    "//   Example hedged: { \"order\": \"round_robin\", \"dispatch\": { \"type\": \"hedged\", \"delay_ms\": 2000, \"max_parallel\": 2 } }\n",
    "// upstreams[].credential (required): discriminated union — api_keys | account | passthrough.\n",
    "//   api_keys: { \"type\": \"api_keys\", \"api_keys\": [\"key-a\", \"key-b\"] }\n",
    "//   account: { \"type\": \"account\", \"provider\": \"kiro\"|\"codex\"|\"xai\", \"account_id\": \"...\" }\n",
    "//   passthrough: { \"type\": \"passthrough\" }\n",
    "// app_proxy_url (optional): http(s)://... | socks5(h)://... (used for app updates and upstream proxy reuse).\n",
    "// upstreams[].proxy_url (optional): empty => direct; \"$app_proxy_url\" => use app_proxy_url; or an explicit proxy URL.\n",
    "// upstreams[].providers (required): one upstream can serve multiple providers. Example: [\"openai\", \"openai-response\"].\n",
    "// hot_model_mappings (optional): global alias -> target model map. Delete this field to reset defaults on next load.\n",
    "// upstreams[].convert_from_map (optional): explicitly allow inbound format conversion per provider.\n",
    "//   Example: { \"openai-response\": [\"openai_chat\", \"anthropic_messages\"] }\n"
);

struct ParsedConfigFile {
    config: ProxyConfigFile,
    migrated: bool,
}

pub(super) async fn load_config_file(paths: &TokenProxyPaths) -> Result<ProxyConfigFile, String> {
    let path = paths.config_file();
    tracing::debug!(path = %path.display(), "load_config_file start");
    let start = Instant::now();
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => {
            tracing::debug!(
                path = %path.display(),
                bytes = contents.len(),
                elapsed_ms = start.elapsed().as_millis(),
                "load_config_file read"
            );
            let parsed = parse_config_file(&contents, path)?;
            if parsed.migrated {
                // 迁移写回前先备份原始字节，与仓库其它配置写路径一致（*.token_proxy.bak）。
                tracing::info!(path = %path.display(), "config migrated, backing up then writing back");
                write_config_backup(path, &contents).await?;
                save_config_file(paths, &parsed.config).await?;
            }
            Ok(parsed.config)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                path = %path.display(),
                elapsed_ms = start.elapsed().as_millis(),
                "load_config_file missing, creating default"
            );
            let config = ProxyConfigFile::default();
            save_config_file(paths, &config).await?;
            Ok(config)
        }
        Err(err) => {
            tracing::error!(
                path = %path.display(),
                elapsed_ms = start.elapsed().as_millis(),
                error = %err,
                "load_config_file read failed"
            );
            Err(format!("Failed to read config file: {err}"))
        }
    }
}

pub(super) async fn save_config_file(
    paths: &TokenProxyPaths,
    config: &ProxyConfigFile,
) -> Result<(), String> {
    let path = paths.config_file();
    tracing::debug!(path = %path.display(), "save_config_file start");
    let start = Instant::now();
    ensure_parent_dir(path).await?;
    tracing::debug!(
        path = %path.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "save_config_file ensured dir"
    );
    let data = serde_json::to_string_pretty(config)
        .map_err(|err| format!("Failed to serialize config: {err}"))?;
    let header = read_existing_header(path)
        .await
        .unwrap_or_else(default_config_header);
    tracing::debug!(
        path = %path.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "save_config_file header ready"
    );
    let output = merge_header_and_body(header, data);
    // 同目录临时文件 + flush/sync + 原子 rename；失败时旧主文件保持不变。
    atomic_write_bytes(path, output.as_bytes()).await?;
    tracing::debug!(
        path = %path.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "save_config_file wrote"
    );
    Ok(())
}

/// 测试 seam：临时文件已成功写入后、rename 前失败。
#[cfg(test)]
pub(crate) fn set_fail_rename_after_temp_write(fail: bool) {
    FAIL_RENAME_AFTER_TEMP_WRITE.store(fail, Ordering::SeqCst);
}

/// 可靠写盘：tmp → flush/sync → rename 覆盖目标。rename 前失败不触碰旧主文件。
async fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp_path = temp_config_path(path);
    tracing::debug!(
        target = %path.display(),
        temp = %temp_path.display(),
        bytes = bytes.len(),
        "atomic config write start"
    );

    if let Err(err) = write_temp_file_synced(&temp_path, bytes).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(err);
    }

    // 窄 seam：证明 temp 已落盘后 rename 失败不会改写主文件。
    if FAIL_RENAME_AFTER_TEMP_WRITE.load(Ordering::SeqCst) {
        let _ = tokio::fs::remove_file(&temp_path).await;
        tracing::error!(
            target = %path.display(),
            "injected atomic rename failure after temp write; previous config left intact"
        );
        return Err("injected atomic config rename failure after temp write".to_string());
    }

    if let Err(err) = tokio::fs::rename(&temp_path, path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        tracing::error!(
            target = %path.display(),
            error = %err,
            "atomic config rename failed; previous config file left intact"
        );
        return Err(format!(
            "Failed to atomically replace config {}: {err}",
            path.display()
        ));
    }
    Ok(())
}

async fn write_temp_file_synced(temp_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = tokio::fs::File::create(temp_path).await.map_err(|err| {
        format!(
            "Failed to create temp config {}: {err}",
            temp_path.display()
        )
    })?;
    file.write_all(bytes)
        .await
        .map_err(|err| format!("Failed to write temp config {}: {err}", temp_path.display()))?;
    file.flush()
        .await
        .map_err(|err| format!("Failed to flush temp config {}: {err}", temp_path.display()))?;
    file.sync_all()
        .await
        .map_err(|err| format!("Failed to sync temp config {}: {err}", temp_path.display()))?;
    Ok(())
}

fn temp_config_path(path: &Path) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.jsonc");
    path.with_file_name(format!(".{file_name}.{unique}.tmp"))
}

pub(super) async fn init_default_config_file(paths: &TokenProxyPaths) -> Result<(), String> {
    let path = paths.config_file();
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Err("Config file already exists.".to_string());
    }
    save_config_file(paths, &ProxyConfigFile::default()).await
}

fn parse_config_file(contents: &str, path: &Path) -> Result<ParsedConfigFile, String> {
    let sanitized = crate::jsonc::sanitize_jsonc(contents);
    let mut value: serde_json::Value = serde_json::from_str(&sanitized)
        .map_err(|err| format!("Failed to parse config file {}: {err}", path.display()))?;
    // 迁移冲突/错误必须传播给调用方，禁止吞掉后继续反序列化。
    let migrated = migrate_config_json(&mut value).map_err(|err| {
        tracing::error!(
            path = %path.display(),
            error = %err,
            "config migration failed; original file left unchanged"
        );
        format!("Failed to migrate config file {}: {err}", path.display())
    })?;
    let config: ProxyConfigFile = serde_json::from_value(value)
        .map_err(|err| format!("Failed to parse config file {}: {err}", path.display()))?;
    Ok(ParsedConfigFile { config, migrated })
}

/// 迁移写回前备份原始文件内容；扩展名规则对齐 client_config。
async fn write_config_backup(path: &Path, original_contents: &str) -> Result<(), String> {
    let backup_path = build_backup_path(path);
    tokio::fs::write(&backup_path, original_contents)
        .await
        .map_err(|err| format!("Failed to write backup {}: {err}", backup_path.display()))?;
    tracing::info!(
        path = %path.display(),
        backup_path = %backup_path.display(),
        "wrote config backup before migration writeback"
    );
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

async fn read_existing_header(path: &Path) -> Option<String> {
    tracing::debug!(path = %path.display(), "read_existing_header start");
    let start = Instant::now();
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    tracing::debug!(
        path = %path.display(),
        bytes = contents.len(),
        elapsed_ms = start.elapsed().as_millis(),
        "read_existing_header read"
    );
    let header = extract_leading_jsonc_comments(&contents);
    if header.trim().is_empty() {
        None
    } else {
        Some(header)
    }
}

fn extract_leading_jsonc_comments(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b' ' || byte == b'\t' || byte == b'\r' || byte == b'\n' {
            output.push(byte);
            index += 1;
            continue;
        }

        if byte == b'/' && index + 1 < bytes.len() {
            let next = bytes[index + 1];
            if next == b'/' {
                output.push(byte);
                output.push(next);
                index += 2;
                while index < bytes.len() {
                    let current = bytes[index];
                    output.push(current);
                    index += 1;
                    if current == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if next == b'*' {
                output.push(byte);
                output.push(next);
                index += 2;
                while index < bytes.len() {
                    let current = bytes[index];
                    output.push(current);
                    index += 1;
                    if current == b'*' && index < bytes.len() && bytes[index] == b'/' {
                        output.push(b'/');
                        index += 1;
                        break;
                    }
                }
                continue;
            }
        }

        break;
    }

    String::from_utf8(output).unwrap_or_default()
}

fn default_config_header() -> String {
    DEFAULT_CONFIG_HEADER.to_string()
}

fn merge_header_and_body(header: String, body: String) -> String {
    if header.is_empty() {
        format!("{body}\n")
    } else if header.ends_with('\n') {
        format!("{header}{body}\n")
    } else {
        format!("{header}\n{body}\n")
    }
}

async fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    tracing::debug!(path = %parent.display(), "ensure_parent_dir start");
    let start = Instant::now();
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|err| format!("Failed to create config directory: {err}"))?;
    tracing::debug!(
        path = %parent.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "ensure_parent_dir done"
    );
    Ok(())
}

#[cfg(test)]
#[path = "io.test.rs"]
mod tests;
