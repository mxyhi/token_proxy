use base64::Engine;
use serde_json::Value;

use super::token_estimator::{self, TokenProvider};

const OPENAI_MESSAGE_OVERHEAD: u64 = 3;
const OPENAI_NAME_OVERHEAD: u64 = 3;
const OPENAI_TOOL_OVERHEAD: u64 = 8;
const OPENAI_FIXED_OVERHEAD: u64 = 3;

const DEFAULT_IMAGE_TOKENS: u64 = 520;
const DEFAULT_AUDIO_TOKENS: u64 = 256;
const DEFAULT_VIDEO_TOKENS: u64 = 4096 * 2;
const DEFAULT_FILE_TOKENS: u64 = 4096;

const IMAGE_DECODE_LIMIT_BYTES: usize = 64 * 1024;

pub(crate) fn estimate_request_input_tokens(value: &Value, model: Option<&str>) -> Option<u64> {
    let message_stats = sum_message_stats(value, model);
    let root_tokens = sum_root_tokens(value, model);
    let tool_count = count_tools(value);
    let total = message_stats
        .tokens
        .saturating_add(root_tokens)
        .saturating_add(openai_overhead_tokens(
            message_stats.message_count,
            message_stats.name_count,
            tool_count,
        ));

    if total == 0 {
        None
    } else {
        Some(total)
    }
}

#[derive(Default)]
struct MessageStats {
    tokens: u64,
    message_count: u64,
    name_count: u64,
}

fn sum_message_stats(value: &Value, model: Option<&str>) -> MessageStats {
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return MessageStats::default();
    };

    let mut stats = MessageStats::default();
    for message in messages {
        stats.message_count = stats.message_count.saturating_add(1);
        if message.get("name").and_then(Value::as_str).is_some() {
            stats.name_count = stats.name_count.saturating_add(1);
        }
        stats.tokens = stats
            .tokens
            .saturating_add(sum_message_text_tokens(message, model))
            .saturating_add(sum_message_media_tokens(message, model));
    }
    stats
}

fn sum_root_tokens(value: &Value, model: Option<&str>) -> u64 {
    let mut total = 0u64;

    if let Some(prompt) = value.get("prompt") {
        total = total.saturating_add(sum_text_value(prompt, model));
    }

    if let Some(input) = value.get("input") {
        total = total.saturating_add(sum_input_text_tokens(input, model));
        total = total.saturating_add(sum_input_media_tokens(input, model));
    }

    if let Some(system) = value.get("system") {
        total = total.saturating_add(sum_text_value(system, model));
    }

    if let Some(system_instruction) = value.get("system_instruction") {
        total = total.saturating_add(sum_text_value(system_instruction, model));
    }

    if let Some(system_instruction) = value.get("systemInstruction") {
        total = total.saturating_add(sum_text_value(system_instruction, model));
    }

    if let Some(instructions) = value.get("instructions") {
        total = total.saturating_add(sum_text_value(instructions, model));
    }

    if let Some(contents) = value.get("contents") {
        total = total.saturating_add(sum_gemini_contents_text_tokens(contents, model));
        total = total.saturating_add(sum_gemini_contents_media_tokens(contents, model));
    }

    total
}

fn count_tools(value: &Value) -> u64 {
    value
        .get("tools")
        .and_then(Value::as_array)
        .map(|items| items.len() as u64)
        .unwrap_or(0)
}

fn openai_overhead_tokens(message_count: u64, name_count: u64, tool_count: u64) -> u64 {
    if message_count == 0 && name_count == 0 && tool_count == 0 {
        return 0;
    }
    message_count
        .saturating_mul(OPENAI_MESSAGE_OVERHEAD)
        .saturating_add(name_count.saturating_mul(OPENAI_NAME_OVERHEAD))
        .saturating_add(tool_count.saturating_mul(OPENAI_TOOL_OVERHEAD))
        .saturating_add(OPENAI_FIXED_OVERHEAD)
}

fn sum_message_text_tokens(message: &Value, model: Option<&str>) -> u64 {
    let Some(content) = message.get("content") else {
        return 0;
    };
    sum_content_text_tokens(content, model)
}

fn sum_message_media_tokens(message: &Value, model: Option<&str>) -> u64 {
    let Some(content) = message.get("content") else {
        return 0;
    };
    sum_content_media_tokens(content, model)
}

fn sum_input_text_tokens(input: &Value, model: Option<&str>) -> u64 {
    match input {
        Value::String(_) => sum_text_value(input, model),
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            let mut total = acc;
            if item.is_string() {
                total = total.saturating_add(sum_text_value(item, model));
            } else if let Some(content) = item.get("content") {
                total = total.saturating_add(sum_content_text_tokens(content, model));
            } else if let Some(text) = item.get("text") {
                total = total.saturating_add(sum_text_value(text, model));
            }
            total
        }),
        Value::Object(object) => object
            .get("content")
            .map(|content| sum_content_text_tokens(content, model))
            .or_else(|| object.get("text").map(|text| sum_text_value(text, model)))
            .unwrap_or(0),
        _ => 0,
    }
}

fn sum_input_media_tokens(input: &Value, model: Option<&str>) -> u64 {
    match input {
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            let mut total = acc;
            if let Some(content) = item.get("content") {
                total = total.saturating_add(sum_content_media_tokens(content, model));
            }
            total
        }),
        Value::Object(object) => object
            .get("content")
            .map(|content| sum_content_media_tokens(content, model))
            .unwrap_or(0),
        _ => 0,
    }
}

fn sum_gemini_contents_text_tokens(contents: &Value, model: Option<&str>) -> u64 {
    let Some(contents) = contents.as_array() else {
        return 0;
    };
    contents.iter().fold(0u64, |acc, content| {
        let mut total = acc;
        if let Some(parts) = content.get("parts").and_then(Value::as_array) {
            for part in parts {
                if let Some(text) = part.get("text") {
                    total = total.saturating_add(sum_text_value(text, model));
                }
            }
        }
        total
    })
}

fn sum_gemini_contents_media_tokens(contents: &Value, model: Option<&str>) -> u64 {
    let Some(contents) = contents.as_array() else {
        return 0;
    };
    contents.iter().fold(0u64, |acc, content| {
        let mut total = acc;
        if let Some(parts) = content.get("parts").and_then(Value::as_array) {
            for part in parts {
                total = total.saturating_add(sum_gemini_part_media_tokens(part, model));
            }
        }
        total
    })
}

fn sum_gemini_part_media_tokens(part: &Value, model: Option<&str>) -> u64 {
    if let Some(inline) = part.get("inlineData") {
        let mime = inline.get("mimeType").and_then(Value::as_str);
        let data = inline.get("data").and_then(Value::as_str);
        return estimate_media_tokens(model, mime, data, None);
    }
    if let Some(file_data) = part.get("fileData") {
        let mime = file_data.get("mimeType").and_then(Value::as_str);
        return estimate_media_tokens(model, mime, None, None);
    }
    0
}

fn sum_content_text_tokens(content: &Value, model: Option<&str>) -> u64 {
    match content {
        Value::String(_) => sum_text_value(content, model),
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            let mut total = acc;
            if let Some(text) = item.get("text") {
                total = total.saturating_add(sum_text_value(text, model));
            } else if item.is_string() {
                total = total.saturating_add(sum_text_value(item, model));
            }
            total
        }),
        _ => 0,
    }
}

fn sum_content_media_tokens(content: &Value, model: Option<&str>) -> u64 {
    let Some(items) = content.as_array() else {
        return 0;
    };
    items.iter().fold(0u64, |acc, item| {
        acc.saturating_add(sum_openai_part_media_tokens(item, model))
            .saturating_add(sum_anthropic_part_media_tokens(item, model))
    })
}

fn sum_openai_part_media_tokens(part: &Value, model: Option<&str>) -> u64 {
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        return 0;
    };
    match part_type {
        "image_url" => {
            let image = part.get("image_url");
            let (url, detail) = match image {
                Some(Value::String(url)) => (Some(url.as_str()), None),
                Some(Value::Object(obj)) => (
                    obj.get("url").and_then(Value::as_str),
                    obj.get("detail").and_then(Value::as_str),
                ),
                _ => (None, None),
            };
            estimate_media_tokens(model, Some("image/*"), url, detail)
        }
        "input_audio" => {
            let audio = part.get("input_audio").and_then(Value::as_object);
            let data = audio.and_then(|obj| obj.get("data").and_then(Value::as_str));
            estimate_media_tokens(model, Some("audio/*"), data, None)
        }
        _ => 0,
    }
}

fn sum_anthropic_part_media_tokens(part: &Value, model: Option<&str>) -> u64 {
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        return 0;
    };
    if part_type != "image" {
        return 0;
    }
    let source = part.get("source");
    let mime = source
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("media_type"))
        .and_then(Value::as_str);
    let data = source
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("data"))
        .and_then(Value::as_str);
    estimate_media_tokens(model, mime, data, None)
}

fn sum_text_value(value: &Value, model: Option<&str>) -> u64 {
    match value {
        Value::String(text) => token_estimator::estimate_text_tokens(model, text),
        Value::Array(items) => items.iter().fold(0u64, |acc, item| {
            acc.saturating_add(sum_text_value(item, model))
        }),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(|text| token_estimator::estimate_text_tokens(model, text))
            .unwrap_or(0),
        _ => 0,
    }
}

fn estimate_media_tokens(
    model: Option<&str>,
    mime: Option<&str>,
    data: Option<&str>,
    detail: Option<&str>,
) -> u64 {
    let kind = media_kind_from_mime(mime);
    match kind {
        MediaKind::Image => estimate_image_tokens(model, data, detail),
        MediaKind::Audio => DEFAULT_AUDIO_TOKENS,
        MediaKind::Video => DEFAULT_VIDEO_TOKENS,
        MediaKind::Other => DEFAULT_FILE_TOKENS,
    }
}

fn estimate_image_tokens(model: Option<&str>, data: Option<&str>, detail: Option<&str>) -> u64 {
    let provider = token_estimator::provider_for_model(model);
    if provider != TokenProvider::OpenAI {
        return DEFAULT_IMAGE_TOKENS;
    }

    let normalized = model.unwrap_or("").trim().to_ascii_lowercase();

    if normalized.contains("glm-4") {
        return 1047;
    }

    if let Some(multiplier) = patch_multiplier(&normalized) {
        if let Some((width, height)) = decode_image_dimensions(data) {
            return estimate_patch_tokens(width, height, multiplier);
        }
        return base_tile_tokens(&normalized).0;
    }

    let (base_tokens, tile_tokens) = base_tile_tokens(&normalized);
    if detail == Some("low") {
        return base_tokens;
    }

    if let Some((width, height)) = decode_image_dimensions(data) {
        return estimate_tile_tokens(width, height, base_tokens, tile_tokens);
    }

    base_tokens
}

fn media_kind_from_mime(mime: Option<&str>) -> MediaKind {
    let Some(mime) = mime else {
        return MediaKind::Other;
    };
    let normalized = mime.to_ascii_lowercase();
    if normalized.starts_with("image/") {
        return MediaKind::Image;
    }
    if normalized.starts_with("audio/") {
        return MediaKind::Audio;
    }
    if normalized.starts_with("video/") {
        return MediaKind::Video;
    }
    MediaKind::Other
}

fn patch_multiplier(model: &str) -> Option<f64> {
    if model.contains("gpt-4.1-mini") || model.contains("gpt-5-mini") {
        return Some(1.62);
    }
    if model.contains("gpt-4.1-nano") || model.contains("gpt-5-nano") {
        return Some(2.46);
    }
    if model.contains("o4-mini") {
        return Some(1.72);
    }
    None
}

fn base_tile_tokens(model: &str) -> (u64, u64) {
    if model.contains("gpt-4o-mini") {
        return (2833, 5667);
    }
    if model.contains("gpt-5-chat-latest")
        || (model.contains("gpt-5") && !model.contains("mini") && !model.contains("nano"))
    {
        return (70, 140);
    }
    if model.starts_with("o1") || model.starts_with("o3") || model.contains("o1-pro") {
        return (75, 150);
    }
    if model.contains("computer-use-preview") {
        return (65, 129);
    }
    if model.contains("4.1") || model.contains("4o") || model.contains("4.5") {
        return (85, 170);
    }
    (85, 170)
}

// tile 规则：最长边 <= 2048；最短边缩放至 768；按 512 分块。
fn estimate_tile_tokens(width: u32, height: u32, base: u64, tile: u64) -> u64 {
    if width == 0 || height == 0 {
        return base;
    }
    let (mut w, mut h) = (width as f64, height as f64);
    let max_side = w.max(h);
    if max_side > 2048.0 {
        let ratio = 2048.0 / max_side;
        w *= ratio;
        h *= ratio;
    }
    let min_side = w.min(h);
    if min_side > 0.0 {
        let ratio = 768.0 / min_side;
        w *= ratio;
        h *= ratio;
    }
    let tiles_w = (w / 512.0).ceil() as u64;
    let tiles_h = (h / 512.0).ceil() as u64;
    base.saturating_add(tiles_w.saturating_mul(tiles_h).saturating_mul(tile))
}

// patch 规则：32x32 patch，上限 1536，按 multiplier 估算。
fn estimate_patch_tokens(width: u32, height: u32, multiplier: f64) -> u64 {
    if width == 0 || height == 0 {
        return 0;
    }
    let (mut w, mut h) = (width as f64, height as f64);
    let mut patches = (w / 32.0).ceil() * (h / 32.0).ceil();
    if patches > 1536.0 {
        let ratio = (1536.0 / patches).sqrt();
        w *= ratio;
        h *= ratio;
        patches = (w / 32.0).ceil() * (h / 32.0).ceil();
    }
    (patches * multiplier).ceil() as u64
}

fn decode_image_dimensions(data: Option<&str>) -> Option<(u32, u32)> {
    let Some(data) = data else {
        return None;
    };

    if let Some((mime, payload)) = parse_data_uri(data) {
        let bytes = decode_base64_prefix(payload, IMAGE_DECODE_LIMIT_BYTES)?;
        return decode_dimensions_from_bytes(mime, &bytes);
    }

    let bytes = decode_base64_prefix(data, IMAGE_DECODE_LIMIT_BYTES)?;
    decode_dimensions_from_bytes(None, &bytes)
}

fn parse_data_uri(data: &str) -> Option<(Option<&str>, &str)> {
    let data = data.strip_prefix("data:")?;
    let (meta, payload) = data.split_once(',')?;
    let (mime, encoding) = meta.split_once(';')?;
    if encoding.trim() != "base64" {
        return None;
    }
    Some((Some(mime.trim()), payload))
}

fn decode_base64_prefix(data: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let max_chars = ((max_bytes + 2) / 3) * 4;
    let mut slice_len = data.len().min(max_chars);
    slice_len -= slice_len % 4;
    if slice_len == 0 {
        return None;
    }
    let prefix = &data[..slice_len];
    base64::engine::general_purpose::STANDARD
        .decode(prefix)
        .ok()
}

fn decode_dimensions_from_bytes(mime: Option<&str>, bytes: &[u8]) -> Option<(u32, u32)> {
    if let Some(mime) = mime {
        let normalized = mime.to_ascii_lowercase();
        if normalized.contains("png") {
            return png_dimensions(bytes);
        }
        if normalized.contains("jpeg") || normalized.contains("jpg") {
            return jpeg_dimensions(bytes);
        }
    }
    png_dimensions(bytes).or_else(|| jpeg_dimensions(bytes))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != PNG_SIGNATURE {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2usize;
    while index + 3 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if marker == 0xD8 || marker == 0xD9 {
            index += 2;
            continue;
        }
        if index + 3 >= bytes.len() {
            break;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if length < 2 || index + 2 + length > bytes.len() {
            break;
        }
        if is_sof_marker(marker) {
            if length >= 7 {
                let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
                let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]);
                return Some((width as u32, height as u32));
            }
            return None;
        }
        index += 2 + length;
    }
    None
}

fn is_sof_marker(marker: u8) -> bool {
    matches!(
        marker,
        0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaKind {
    Image,
    Audio,
    Video,
    Other,
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "request_token_estimate.test.rs"]
mod tests;
