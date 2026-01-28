use std::path::Path;
use std::time::Instant;

use crate::paths::TokenProxyPaths;

use super::ProxyConfigFile;
use super::migrate::migrate_config_json;

const DEFAULT_CONFIG_HEADER: &str = concat!(
    "// Token Proxy config (JSONC). Comments and trailing commas are supported.\n",
    "// log_level (optional): silent|error|warn|info|debug|trace. Default: silent.\n",
    "// app_proxy_url (optional): http(s)://... | socks5(h)://... (used for app updates and upstream proxy reuse).\n",
    "// upstreams[].proxy_url (optional): empty => direct; \"$app_proxy_url\" => use app_proxy_url; or an explicit proxy URL.\n",
    "// upstreams[].providers (required): one upstream can serve multiple providers. Example: [\"openai\", \"openai-response\"].\n",
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
            let parsed = parse_config_file(&contents, &path)?;
            if parsed.migrated {
                tracing::info!(path = %path.display(), "config migrated, writing back");
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
    ensure_parent_dir(&path).await?;
    tracing::debug!(
        path = %path.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "save_config_file ensured dir"
    );
    let data = serde_json::to_string_pretty(config)
        .map_err(|err| format!("Failed to serialize config: {err}"))?;
    let header = read_existing_header(&path)
        .await
        .unwrap_or_else(default_config_header);
    tracing::debug!(
        path = %path.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "save_config_file header ready"
    );
    let output = merge_header_and_body(header, data);
    tokio::fs::write(&path, output)
        .await
        .map_err(|err| format!("Failed to write config file: {err}"))?;
    tracing::debug!(
        path = %path.display(),
        elapsed_ms = start.elapsed().as_millis(),
        "save_config_file wrote"
    );
    Ok(())
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
    let migrated = migrate_config_json(&mut value);
    let config: ProxyConfigFile = serde_json::from_value(value)
        .map_err(|err| format!("Failed to parse config file {}: {err}", path.display()))?;
    Ok(ParsedConfigFile { config, migrated })
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
