use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use serde_json::{Map, Value};

/// Marker used when Responses `encrypted_content` carries redacted Claude data,
/// not a replayable thinking signature.
pub(crate) const REDACTED_THINKING_PREFIX: &str = "claude-redacted-thinking:";

/// Marker for an opaque Claude thinking block carried through Responses.
pub(crate) const SIGNED_THINKING_PREFIX: &str = "anthropic-thinking-v1:";

/// Local history marker, never a provider signature or trusted encrypted payload.
pub(crate) const COMMENTARY_MARKER: &str = "cpa:commentary:v1";

pub(crate) fn is_commentary_marker(signature: Option<&str>) -> bool {
    signature.is_some_and(|value| value.trim() == COMMENTARY_MARKER)
}

/// Native Claude requests must restore this internal marker before provider validation.
pub(crate) fn restore_commentary_markers(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for message in messages {
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) == Some("thinking")
                && is_commentary_marker(block.get("signature").and_then(Value::as_str))
            {
                *block = serde_json::json!({"type":"text","text":block.get("thinking").and_then(Value::as_str).unwrap_or("")});
                changed = true;
            }
        }
    }
    changed
}

/// Wrap redacted Claude data in the Responses carrier without changing payload bytes.
pub(crate) fn redacted_thinking_carrier(data: &str) -> Option<String> {
    (!data.is_empty()).then(|| format!("{REDACTED_THINKING_PREFIX}{data}"))
}

/// Return redacted Claude data only when the carrier has the explicit marker.
pub(crate) fn redacted_thinking_data(encrypted_content: &str) -> Option<&str> {
    encrypted_content
        .strip_prefix(REDACTED_THINKING_PREFIX)
        .filter(|data| !data.is_empty())
}

/// Encode only Claude-owned thinking fields; arbitrary OpenAI ciphertext never
/// becomes a Claude signature by accident.
pub(crate) fn signed_thinking_carrier(
    block_type: &str,
    thinking: Option<&str>,
    signature: Option<&str>,
    data: Option<&str>,
) -> Option<String> {
    let mut block = Map::new();
    block.insert("type".to_string(), Value::String(block_type.to_string()));
    if let Some(thinking) = thinking.filter(|value| !value.is_empty()) {
        block.insert("thinking".to_string(), Value::String(thinking.to_string()));
    }
    if let Some(signature) = signature.filter(|value| !value.is_empty()) {
        block.insert(
            "signature".to_string(),
            Value::String(signature.to_string()),
        );
    }
    if let Some(data) = data.filter(|value| !value.is_empty()) {
        block.insert("data".to_string(), Value::String(data.to_string()));
    }

    let valid = match block_type {
        "thinking" => block.get("signature").is_some(),
        "redacted_thinking" => block.get("data").is_some(),
        _ => false,
    };
    if !valid {
        return None;
    }
    let encoded = serde_json::to_vec(&Value::Object(block)).ok()?;
    Some(format!(
        "{SIGNED_THINKING_PREFIX}{}",
        STANDARD_NO_PAD.encode(encoded)
    ))
}

/// Decode a carrier only when its explicit prefix and block shape are valid.
pub(crate) fn signed_thinking_block(carrier: &str) -> Option<Value> {
    let encoded = carrier.strip_prefix(SIGNED_THINKING_PREFIX)?;
    let bytes = STANDARD_NO_PAD.decode(encoded).ok()?;
    let block = serde_json::from_slice::<Value>(&bytes).ok()?;
    let object = block.as_object()?;
    let block_type = object.get("type").and_then(Value::as_str)?;
    let valid = match block_type {
        "thinking" => object
            .get("signature")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "redacted_thinking" => object
            .get("data")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        _ => false,
    };
    valid.then_some(block)
}

pub(crate) fn signed_thinking_signature(carrier: &str) -> Option<String> {
    signed_thinking_block(carrier)?
        .get("signature")
        .and_then(Value::as_str)
        .map(str::to_string)
}
