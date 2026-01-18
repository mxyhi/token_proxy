use base64::Engine;
use serde_json::json;

use super::estimate_request_input_tokens;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn encode_png_base64(width: u32, height: u32) -> String {
    let mut bytes = vec![0u8; 24];
    bytes[..8].copy_from_slice(&PNG_SIGNATURE);
    bytes[8..12].copy_from_slice(&13u32.to_be_bytes());
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn estimates_openai_overhead_tokens() {
    let value = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": ""},
            {"role": "assistant", "name": "bot", "content": ""}
        ],
        "tools": [{}, {}, {}]
    });
    let tokens = estimate_request_input_tokens(&value, Some("gpt-4o")).unwrap();
    // 2 messages *3 + 1 name *3 + 3 tools *8 + 3 fixed
    assert_eq!(tokens, 36);
}

#[test]
fn estimates_low_detail_image_tokens_for_openai() {
    let value = json!({
        "model": "gpt-4o",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": "https://example.com/a.png", "detail": "low"}}
                ]
            }
        ]
    });
    let tokens = estimate_request_input_tokens(&value, Some("gpt-4o")).unwrap();
    // base 85 + overhead (1 message + fixed)
    assert_eq!(tokens, 91);
}

#[test]
fn estimates_image_tokens_for_non_openai_model() {
    let value = json!({
        "model": "claude-3-opus",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}}
                ]
            }
        ]
    });
    let tokens = estimate_request_input_tokens(&value, Some("claude-3-opus")).unwrap();
    // 520 default image + overhead 6
    assert_eq!(tokens, 526);
}

#[test]
fn estimates_patch_image_tokens_for_gpt_4_1_mini() {
    let data = encode_png_base64(32, 32);
    let value = json!({
        "model": "gpt-4.1-mini",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", data)}}
                ]
            }
        ]
    });
    let tokens = estimate_request_input_tokens(&value, Some("gpt-4.1-mini")).unwrap();
    // patch 1 * 1.62 => ceil 2, overhead 6
    assert_eq!(tokens, 8);
}

#[test]
fn estimates_tile_image_tokens_for_gpt_4o() {
    let data = encode_png_base64(1024, 1024);
    let value = json!({
        "model": "gpt-4o",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", data)}}
                ]
            }
        ]
    });
    let tokens = estimate_request_input_tokens(&value, Some("gpt-4o")).unwrap();
    // base 85 + tiles 4 * 170 + overhead 6
    assert_eq!(tokens, 771);
}

#[test]
fn estimates_gemini_inline_image_tokens() {
    let data = encode_png_base64(64, 64);
    let value = json!({
        "model": "gemini-1.5-flash",
        "contents": [
            {
                "parts": [
                    {"inlineData": {"mimeType": "image/png", "data": data}}
                ]
            }
        ]
    });
    let tokens = estimate_request_input_tokens(&value, Some("gemini-1.5-flash")).unwrap();
    assert_eq!(tokens, 520);
}

#[test]
fn estimates_openai_input_audio_tokens() {
    let value = json!({
        "model": "gpt-4o",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "input_audio", "input_audio": {"data": "dGVzdA=="}}
                ]
            }
        ]
    });
    let tokens = estimate_request_input_tokens(&value, Some("gpt-4o")).unwrap();
    // audio 256 + overhead 6
    assert_eq!(tokens, 262);
}
