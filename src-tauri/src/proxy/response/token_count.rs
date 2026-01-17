use axum::body::Bytes;
use serde_json::Value;

use super::super::token_rate::RequestTokenTracker;

pub(super) async fn apply_output_tokens_from_response(
    tracker: &RequestTokenTracker,
    provider: &str,
    bytes: &Bytes,
) {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let mut texts = Vec::new();

    match provider {
        "openai" | "openai-response" => {
            if let Some(choices) = value.get("choices").and_then(Value::as_array) {
                for choice in choices {
                    if let Some(content) = choice
                        .get("message")
                        .and_then(|message| message.get("content"))
                    {
                        if let Some(text) = content.as_str() {
                            texts.push(text.to_string());
                        } else if let Some(parts) = content.as_array() {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    texts.push(text.to_string());
                                }
                            }
                        }
                    }
                    if let Some(text) = choice.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
            if texts.is_empty() {
                if let Some(output) = value.get("output").and_then(Value::as_array) {
                    collect_responses_output(output, &mut texts);
                }
            }
        }
        "anthropic" => {
            if let Some(content) = value.get("content").and_then(Value::as_array) {
                for item in content {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
        }
        "gemini" => {
            if let Some(candidates) = value.get("candidates").and_then(Value::as_array) {
                collect_gemini_output(candidates, &mut texts);
            }
        }
        _ => {}
    }

    if texts.is_empty() {
        return;
    }

    for text in texts {
        tracker.add_output_text(&text).await;
    }
}

fn collect_responses_output(output: &[Value], texts: &mut Vec<String>) {
    for item in output {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    texts.push(text.to_string());
                }
            }
        }
    }
}

fn collect_gemini_output(candidates: &[Value], texts: &mut Vec<String>) {
    for candidate in candidates {
        if let Some(content) = candidate.get("content") {
            if let Some(parts) = content.get("parts").and_then(Value::as_array) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }
}

