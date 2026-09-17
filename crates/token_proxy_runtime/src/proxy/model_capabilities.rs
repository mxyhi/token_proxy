//! 仅依据所选上游的显式能力调整工具结果，普通用户图片始终保留。
use axum::body::Bytes;
use serde_json::{json, Value};

use super::request_body::ReplayableBody;

const IMAGE_OMITTED: &str = "[image omitted: unsupported by upstream]";

pub(super) fn requires_native_search(body: &ReplayableBody) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body.as_bytes()) else {
        return false;
    };
    value
        .get("web_search_options")
        .is_some_and(|value| !value.is_null())
        || value
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| {
                            kind == "web_search"
                                || kind.starts_with("web_search_")
                                || kind == "google_search"
                                || kind == "x_search"
                        })
                        || tool.get("googleSearch").is_some()
                        || tool.get("google_search").is_some()
                })
            })
}

/// 在协议转换前清理原生工具结果，以免 Claude 图片被转换成普通 user 图片后丢失来源。
pub(super) fn text_only_tool_results(body: &ReplayableBody) -> Result<ReplayableBody, String> {
    let mut value: Value = serde_json::from_slice(body.as_bytes())
        .map_err(|err| format!("Invalid JSON for model capability conversion: {err}"))?;
    let mut removed = 0;
    if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if message.get("role").and_then(Value::as_str) == Some("tool") {
                if let Some(content) = message.get_mut("content") {
                    replace_images(content, &mut removed);
                }
            } else if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        if let Some(content) = block.get_mut("content") {
                            replace_images(content, &mut removed);
                        }
                    }
                }
            }
        }
    }
    if let Some(input) = value.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            if matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output")
            ) {
                if let Some(output) = item.get_mut("output") {
                    replace_images(output, &mut removed);
                }
            }
        }
    }
    if removed == 0 {
        return Ok(body.clone());
    }
    tracing::debug!(
        removed,
        "omitted tool result images for explicitly text-only model"
    );
    serde_json::to_vec(&value)
        .map(|bytes| ReplayableBody::from_bytes(Bytes::from(bytes)))
        .map_err(|err| err.to_string())
}

fn replace_images(content: &mut Value, removed: &mut usize) {
    match content {
        Value::Array(parts) => {
            for part in parts {
                replace_images(part, removed);
            }
        }
        Value::Object(part) => {
            let kind = part.get("type").and_then(Value::as_str);
            if matches!(kind, Some("image" | "image_url" | "input_image")) {
                // Responses output 使用 input_text；Chat/Claude 使用 text。
                let text_type = if kind == Some("input_image") {
                    "input_text"
                } else {
                    "text"
                };
                *content = json!({"type":text_type,"text":IMAGE_OMITTED});
                *removed += 1;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_search_detection_covers_responses_and_xai_tools() {
        for tool in ["web_search_preview", "x_search", "google_search"] {
            let body = ReplayableBody::from_bytes(Bytes::from(
                json!({"tools":[{"type":tool}]}).to_string(),
            ));
            assert!(
                requires_native_search(&body),
                "{tool} should require search"
            );
        }
        let body = ReplayableBody::from_bytes(Bytes::from(
            json!({"tools":[{"type":"function","name":"lookup"}]}).to_string(),
        ));
        assert!(!requires_native_search(&body));
    }

    #[test]
    fn only_tool_images_are_omitted_and_source_is_immutable() {
        let source = json!({"messages":[{"role":"user","content":[
            {"type":"image","source":{"data":"user-image"}},
            {"type":"tool_result","tool_use_id":"a","content":[{"type":"text","text":"result"},{"type":"image","source":{"data":"tool-image"}}]}
        ]}]});
        let body = ReplayableBody::from_bytes(Bytes::from(source.to_string()));
        let output = text_only_tool_results(&body).unwrap();
        let output: Value = serde_json::from_slice(output.as_bytes()).unwrap();
        assert_eq!(
            output["messages"][0]["content"][0],
            source["messages"][0]["content"][0]
        );
        assert_eq!(
            output["messages"][0]["content"][1]["content"][1]["text"],
            IMAGE_OMITTED
        );
        assert_eq!(
            serde_json::from_slice::<Value>(body.as_bytes()).unwrap(),
            source
        );
    }
}
