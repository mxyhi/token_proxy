use base64::Engine;
use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Map, Value};

use super::super::http_client::ProxyHttpClients;

const MAX_MEDIA_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

pub(super) async fn input_image_part_to_claude_block(
    part: &Map<String, Value>,
    http_clients: &ProxyHttpClients,
) -> Result<Option<Value>, String> {
    let url = part.get("image_url").and_then(normalize_url_value);
    let Some(url) = url else {
        return Ok(None);
    };

    let (media_type, data) = resolve_media_to_base64(&url, http_clients).await?;
    Ok(Some(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data
        }
    })))
}

pub(super) async fn input_file_part_to_claude_block(
    part: &Map<String, Value>,
    http_clients: &ProxyHttpClients,
) -> Result<Option<Value>, String> {
    // Prefer file_url, but accept "file" wrappers some clients may send.
    let url = part
        .get("file_url")
        .and_then(normalize_url_value)
        .or_else(|| part.get("file").and_then(normalize_url_value));
    let Some(url) = url else {
        // file_id needs an OpenAI Files API call; keep it explicit instead of guessing.
        if part.get("file_id").and_then(Value::as_str).is_some() {
            return Err("input_file with file_id is not supported; use file_url or data: URL.".to_string());
        }
        return Ok(None);
    };

    let (media_type, data) = resolve_media_to_base64(&url, http_clients).await?;
    Ok(Some(json!({
        "type": "document",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data
        }
    })))
}

pub(super) fn claude_image_block_to_input_image_part(block: &Map<String, Value>) -> Option<Value> {
    let source = block.get("source").and_then(Value::as_object)?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let media_type = source.get("media_type").and_then(Value::as_str).unwrap_or("image/png");
    let data = source.get("data").and_then(Value::as_str)?;
    Some(json!({
        "type": "input_image",
        "image_url": format!("data:{media_type};base64,{data}")
    }))
}

pub(super) fn claude_document_block_to_input_file_part(block: &Map<String, Value>) -> Option<Value> {
    let source = block.get("source").and_then(Value::as_object)?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let media_type = source
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let data = source.get("data").and_then(Value::as_str)?;
    Some(json!({
        "type": "input_file",
        "file_url": format!("data:{media_type};base64,{data}")
    }))
}

fn normalize_url_value(value: &Value) -> Option<String> {
    match value {
        Value::String(url) => Some(url.to_string()),
        Value::Object(object) => object.get("url").and_then(Value::as_str).map(|url| url.to_string()),
        _ => None,
    }
}

async fn resolve_media_to_base64(
    url: &str,
    http_clients: &ProxyHttpClients,
) -> Result<(String, String), String> {
    if let Some((media_type, data)) = parse_data_url(url) {
        return Ok((media_type, data));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return download_url_as_base64(url, http_clients).await;
    }
    Err("Unsupported media URL; expected http(s):// or data: URL.".to_string())
}

fn parse_data_url(url: &str) -> Option<(String, String)> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(",")?;
    let meta = meta.trim();
    let data = data.trim();

    // We only need to forward the base64 blob; avoid decoding to keep it fast.
    let (media_type, is_base64) = match meta.split_once(";") {
        Some((media_type, rest)) => (media_type.trim(), rest.trim() == "base64"),
        None => (meta, false),
    };
    if !is_base64 {
        return None;
    }
    if media_type.is_empty() {
        return Some(("application/octet-stream".to_string(), data.to_string()));
    }
    Some((media_type.to_string(), data.to_string()))
}

async fn download_url_as_base64(
    url: &str,
    http_clients: &ProxyHttpClients,
) -> Result<(String, String), String> {
    let client = http_clients.client_for_proxy_url(None)?;
    let res = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("Failed to download media: {err}"))?;

    if !res.status().is_success() {
        return Err(format!("Media download failed with status: {}", res.status()));
    }

    let header_type = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_string());

    let mut bytes = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(next) = stream.next().await {
        let chunk = next.map_err(|err| format!("Failed to download media: {err}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_MEDIA_DOWNLOAD_BYTES {
            return Err(format!(
                "Media download exceeds {MAX_MEDIA_DOWNLOAD_BYTES} bytes limit."
            ));
        }
        bytes.extend_from_slice(&chunk);
    }

    let sniffed = sniff_mime_type(&bytes);
    let media_type = header_type.unwrap_or_else(|| sniffed);

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((media_type, base64_data))
}

fn sniff_mime_type(bytes: &[u8]) -> String {
    if bytes.len() >= 8 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return "image/png".to_string();
    }
    if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        return "image/jpeg".to_string();
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return "image/gif".to_string();
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp".to_string();
    }
    if bytes.len() >= 4 && &bytes[..4] == b"%PDF" {
        return "application/pdf".to_string();
    }
    "application/octet-stream".to_string()
}

