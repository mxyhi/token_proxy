use std::path::{Path, PathBuf};
use std::time::Instant;

use tauri::{AppHandle, Manager};

use super::ProxyConfigFile;

const CONFIG_FILE_NAME: &str = "config.jsonc";
const DEFAULT_CONFIG_HEADER: &str = concat!(
    "// Token Proxy config (JSONC). Comments and trailing commas are supported.\n",
    "// log_level (optional): silent|error|warn|info|debug|trace. Default: silent.\n",
    "// app_proxy_url (optional): http(s)://... | socks5(h)://... (used for app updates and upstream proxy reuse).\n",
    "// upstreams[].proxy_url (optional): empty => direct; \"$app_proxy_url\" => use app_proxy_url; or an explicit proxy URL.\n"
);

pub(super) async fn load_config_file(app: &AppHandle) -> Result<ProxyConfigFile, String> {
    let path = config_file_path(app)?;
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
            parse_config_file(&contents, &path)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                path = %path.display(),
                elapsed_ms = start.elapsed().as_millis(),
                "load_config_file missing, creating default"
            );
            let config = ProxyConfigFile::default();
            save_config_file(app, &config).await?;
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
    app: &AppHandle,
    config: &ProxyConfigFile,
) -> Result<(), String> {
    let path = config_file_path(app)?;
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

/// Config directory: BaseDirectory::AppConfig
pub(crate) fn config_dir_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|err| format!("Failed to resolve config dir: {err}"))
}

/// Config file path: based on the config directory
pub(super) fn config_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir_path(app)?.join(CONFIG_FILE_NAME))
}

/// Log path: relative paths are based on the config directory
pub(super) fn resolve_log_path(app: &AppHandle, log_path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(log_path);
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(config_dir_path(app)?.join(log_path))
}

fn parse_config_file(contents: &str, path: &Path) -> Result<ProxyConfigFile, String> {
    let sanitized = sanitize_jsonc(contents);
    serde_json::from_str(&sanitized)
        .map_err(|err| format!("Failed to parse config file {}: {err}", path.display()))
}

fn sanitize_jsonc(contents: &str) -> String {
    let without_comments = strip_jsonc_comments(contents);
    strip_trailing_commas(&without_comments)
}

fn strip_jsonc_comments(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escape = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            consume_string_byte(byte, &mut in_string, &mut escape, &mut output);
            index += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            output.push(byte);
            index += 1;
            continue;
        }

        if let Some(next_index) = try_skip_comment(bytes, index, &mut output) {
            index = next_index;
            continue;
        }

        output.push(byte);
        index += 1;
    }

    String::from_utf8(output).unwrap_or_default()
}

fn consume_string_byte(byte: u8, in_string: &mut bool, escape: &mut bool, output: &mut Vec<u8>) {
    output.push(byte);
    if *escape {
        *escape = false;
    } else if byte == b'\\' {
        *escape = true;
    } else if byte == b'"' {
        *in_string = false;
    }
}

fn try_skip_comment(bytes: &[u8], index: usize, output: &mut Vec<u8>) -> Option<usize> {
    if bytes[index] != b'/' || index + 1 >= bytes.len() {
        return None;
    }
    match bytes[index + 1] {
        b'/' => Some(skip_line_comment(bytes, index + 2, output)),
        b'*' => Some(skip_block_comment(bytes, index + 2, output)),
        _ => None,
    }
}

fn skip_line_comment(bytes: &[u8], mut index: usize, output: &mut Vec<u8>) -> usize {
    // Line comment: skip until newline, keep the newline for line numbers.
    while index < bytes.len() {
        let current = bytes[index];
        if current == b'\n' {
            output.push(b'\n');
            return index + 1;
        }
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize, output: &mut Vec<u8>) -> usize {
    // Block comment: preserve line breaks for better error positions.
    while index + 1 < bytes.len() {
        let current = bytes[index];
        if current == b'\n' {
            output.push(b'\n');
        }
        if current == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    index
}

fn strip_trailing_commas(contents: &str) -> String {
    let bytes = contents.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escape = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            output.push(byte);
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            output.push(byte);
            index += 1;
            continue;
        }

        if byte == b',' {
            let mut lookahead = index + 1;
            let mut should_skip = false;
            while lookahead < bytes.len() {
                let next = bytes[lookahead];
                if next == b' ' || next == b'\t' || next == b'\r' || next == b'\n' {
                    lookahead += 1;
                    continue;
                }
                if next == b'}' || next == b']' {
                    should_skip = true;
                }
                break;
            }
            if should_skip {
                index += 1;
                continue;
            }
        }

        output.push(byte);
        index += 1;
    }

    String::from_utf8(output).unwrap_or_default()
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
