use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::types::AntigravityTokenRecord;

const FIELD_OAUTH: u64 = 6;
const FIELD_ACCESS_TOKEN: u64 = 1;
const FIELD_TOKEN_TYPE: u64 = 2;
const FIELD_REFRESH_TOKEN: u64 = 3;
const FIELD_EXPIRES_AT: u64 = 4;
const FIELD_TIMESTAMP_SECONDS: u64 = 1;

pub(crate) fn extract_token_record(
    base64_state: &str,
) -> Result<Option<AntigravityTokenRecord>, String> {
    let bytes = decode_base64(base64_state)?;
    let mut pos = 0usize;
    while pos < bytes.len() {
        let tag_start = pos;
        let tag = read_varint(&bytes, &mut pos).ok_or_else(|| "Invalid protobuf tag.".to_string())?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        let field_end = skip_field(&bytes, pos, wire_type)?;
        if field_number == FIELD_OAUTH && wire_type == 2 {
            let length = read_varint(&bytes, &mut pos)
                .ok_or_else(|| "Invalid protobuf length.".to_string())? as usize;
            let end = pos + length;
            if end > bytes.len() {
                return Err("Invalid protobuf length.".to_string());
            }
            let record = parse_oauth_message(&bytes[pos..end])?;
            if record.is_some() {
                return Ok(record);
            }
            pos = end;
        } else {
            pos = field_end;
        }
        if pos == tag_start {
            break;
        }
    }
    Ok(None)
}

pub(crate) fn inject_token_record(
    base64_state: &str,
    record: &AntigravityTokenRecord,
) -> Result<String, String> {
    let bytes = decode_base64(base64_state)?;
    let mut output = Vec::with_capacity(bytes.len() + 128);
    let mut pos = 0usize;
    while pos < bytes.len() {
        let field_start = pos;
        let tag = read_varint(&bytes, &mut pos).ok_or_else(|| "Invalid protobuf tag.".to_string())?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        let field_end = skip_field(&bytes, pos, wire_type)?;
        if field_number != FIELD_OAUTH {
            output.extend_from_slice(&bytes[field_start..field_end]);
        }
        pos = field_end;
    }

    let oauth_payload = build_oauth_message(record)?;
    let mut field_bytes = Vec::with_capacity(1 + oauth_payload.len() + 10);
    field_bytes.extend(encode_varint((FIELD_OAUTH << 3) | 2));
    field_bytes.extend(encode_varint(oauth_payload.len() as u64));
    field_bytes.extend(oauth_payload);
    output.extend(field_bytes);

    Ok(STANDARD.encode(output))
}

fn parse_oauth_message(data: &[u8]) -> Result<Option<AntigravityTokenRecord>, String> {
    let mut pos = 0usize;
    let mut access_token: Option<String> = None;
    let mut refresh_token: Option<String> = None;
    let mut token_type: Option<String> = None;
    let mut expires_seconds: Option<i64> = None;

    while pos < data.len() {
        let tag = read_varint(data, &mut pos).ok_or_else(|| "Invalid oauth tag.".to_string())?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        match field_number {
            FIELD_ACCESS_TOKEN if wire_type == 2 => {
                let text = read_length_delimited_string(data, &mut pos)?;
                if !text.trim().is_empty() {
                    access_token = Some(text);
                }
            }
            FIELD_TOKEN_TYPE if wire_type == 2 => {
                let text = read_length_delimited_string(data, &mut pos)?;
                if !text.trim().is_empty() {
                    token_type = Some(text);
                }
            }
            FIELD_REFRESH_TOKEN if wire_type == 2 => {
                let text = read_length_delimited_string(data, &mut pos)?;
                if !text.trim().is_empty() {
                    refresh_token = Some(text);
                }
            }
            FIELD_EXPIRES_AT if wire_type == 2 => {
                let length = read_varint(data, &mut pos)
                    .ok_or_else(|| "Invalid expiry length.".to_string())? as usize;
                let end = pos + length;
                if end > data.len() {
                    return Err("Invalid expiry length.".to_string());
                }
                expires_seconds = parse_timestamp_seconds(&data[pos..end])?;
                pos = end;
            }
            _ => {
                pos = skip_field(data, pos, wire_type)?;
            }
        }
    }

    let access_token = match access_token {
        Some(value) => value,
        None => return Ok(None),
    };
    let now = OffsetDateTime::now_utc();
    let expires_at = expires_seconds
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok());
    let expired = expires_at
        .and_then(|value| value.format(&Rfc3339).ok())
        .filter(|value| !value.is_empty());
    let expires_in = expires_at.map(|value| (value - now).whole_seconds());

    Ok(Some(AntigravityTokenRecord {
        access_token,
        refresh_token,
        expired,
        expires_in,
        timestamp: Some(now.unix_timestamp() * 1000),
        email: None,
        token_type,
        project_id: None,
        source: Some("ide".to_string()),
    }))
}

fn build_oauth_message(record: &AntigravityTokenRecord) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    push_length_delimited(&mut output, FIELD_ACCESS_TOKEN, &record.access_token)?;
    if let Some(token_type) = record.token_type.as_deref().filter(|value| !value.trim().is_empty()) {
        push_length_delimited(&mut output, FIELD_TOKEN_TYPE, token_type)?;
    }
    if let Some(refresh) = record
        .refresh_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        push_length_delimited(&mut output, FIELD_REFRESH_TOKEN, refresh)?;
    }
    if let Some(expires_at) = record.expires_at() {
        let seconds = expires_at.unix_timestamp();
        let mut ts = Vec::new();
        ts.extend(encode_varint((FIELD_TIMESTAMP_SECONDS << 3) | 0));
        ts.extend(encode_varint(seconds as u64));
        output.extend(encode_varint((FIELD_EXPIRES_AT << 3) | 2));
        output.extend(encode_varint(ts.len() as u64));
        output.extend(ts);
    }
    Ok(output)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(value.trim())
        .map_err(|_| "Invalid base64 state payload.".to_string())
}

fn read_varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut shift = 0u32;
    let mut output = 0u64;
    while *pos < bytes.len() {
        let byte = bytes[*pos];
        *pos += 1;
        output |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(output);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
            bytes.push(byte);
        } else {
            bytes.push(byte);
            break;
        }
    }
    bytes
}

fn skip_field(bytes: &[u8], pos: usize, wire_type: u8) -> Result<usize, String> {
    let mut cursor = pos;
    match wire_type {
        0 => {
            let _ = read_varint(bytes, &mut cursor)
                .ok_or_else(|| "Invalid protobuf varint.".to_string())?;
            Ok(cursor)
        }
        1 => Ok(cursor + 8),
        2 => {
            let length = read_varint(bytes, &mut cursor)
                .ok_or_else(|| "Invalid protobuf length.".to_string())? as usize;
            let end = cursor + length;
            if end > bytes.len() {
                return Err("Invalid protobuf length.".to_string());
            }
            Ok(end)
        }
        5 => Ok(cursor + 4),
        _ => Err("Unsupported protobuf wire type.".to_string()),
    }
}

fn read_length_delimited_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    let length = read_varint(bytes, pos).ok_or_else(|| "Invalid string length.".to_string())? as usize;
    let end = *pos + length;
    if end > bytes.len() {
        return Err("Invalid string length.".to_string());
    }
    let value = String::from_utf8_lossy(&bytes[*pos..end]).to_string();
    *pos = end;
    Ok(value)
}

fn parse_timestamp_seconds(data: &[u8]) -> Result<Option<i64>, String> {
    let mut pos = 0usize;
    while pos < data.len() {
        let tag = read_varint(data, &mut pos).ok_or_else(|| "Invalid timestamp tag.".to_string())?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        if field_number == FIELD_TIMESTAMP_SECONDS && wire_type == 0 {
            let seconds = read_varint(data, &mut pos)
                .ok_or_else(|| "Invalid timestamp varint.".to_string())?;
            return Ok(Some(seconds as i64));
        }
        pos = skip_field(data, pos, wire_type)?;
    }
    Ok(None)
}

fn push_length_delimited(
    out: &mut Vec<u8>,
    field_number: u64,
    value: &str,
) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    out.extend(encode_varint((field_number << 3) | 2));
    out.extend(encode_varint(trimmed.len() as u64));
    out.extend(trimmed.as_bytes());
    Ok(())
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "protobuf.test.rs"]
mod tests;
