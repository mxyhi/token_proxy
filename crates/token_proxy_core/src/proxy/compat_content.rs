use serde_json::{json, Value};

pub(crate) fn chat_message_content_from_responses_parts(parts: &[Value]) -> Value {
    let mut output_parts = Vec::new();
    let mut combined_text = String::new();
    let mut text_only = true;

    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        let part_type = part.get("type").and_then(Value::as_str);
        match part_type {
            Some("output_text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    combined_text.push_str(text);
                    output_parts.push(json!({ "type": "text", "text": text }));
                }
            }
            Some("output_audio") => {}
            Some("reasoning_text") => {}
            Some("refusal") => {
                let text = part
                    .get("refusal")
                    .or_else(|| part.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !text.is_empty() {
                    combined_text.push_str(text);
                    output_parts.push(json!({ "type": "text", "text": text }));
                }
            }
            Some("output_image") => {
                if let Some(image_url) = part.get("image_url") {
                    text_only = false;
                    output_parts
                        .push(json!({ "type": "image_url", "image_url": image_url.clone() }));
                }
            }
            Some("input_image") => {
                if let Some(image_url) = part.get("image_url") {
                    text_only = false;
                    output_parts
                        .push(json!({ "type": "image_url", "image_url": image_url.clone() }));
                }
            }
            Some("input_text") | Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    combined_text.push_str(text);
                    output_parts.push(json!({ "type": "text", "text": text }));
                }
            }
            _ => {
                text_only = false;
            }
        }
    }

    if text_only {
        Value::String(combined_text)
    } else {
        Value::Array(output_parts)
    }
}

pub(crate) fn chat_message_audio_from_responses_parts(parts: &[Value]) -> Option<Value> {
    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("output_audio") {
            continue;
        }
        if let Some(audio) = part.get("audio") {
            return Some(audio.clone());
        }
    }
    None
}

pub(crate) fn chat_message_non_text_parts_from_responses(parts: &[Value]) -> Vec<Value> {
    let mut output_parts = Vec::new();
    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        let part_type = part.get("type").and_then(Value::as_str);
        match part_type {
            Some("output_image") | Some("input_image") => {
                if let Some(image_url) = part.get("image_url") {
                    output_parts
                        .push(json!({ "type": "image_url", "image_url": image_url.clone() }));
                }
            }
            _ => {}
        }
    }
    output_parts
}
