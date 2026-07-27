use serde_json::{Map, Value};

use super::RestoredToolName;
use token_proxy_protocol::tool_identity;

const PRESERVED_IMAGE_NAMESPACE: &str = "image_gen";

pub(super) fn collect_tool_identities(value: &Value) -> Vec<(String, RestoredToolName)> {
    let mut identities = Vec::new();
    let Some(tools) = value.as_array() else {
        return identities;
    };
    for tool in tools {
        let tool_type = tool.get("type").and_then(Value::as_str);
        if matches!(tool_type, Some("function" | "custom")) {
            // Custom tools keep the name at the top level; function tools may nest it.
            let name = tool
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .or_else(|| tool.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|name| !name.is_empty());
            if let Some(name) = name {
                identities.push((
                    name.to_string(),
                    RestoredToolName {
                        name: name.to_string(),
                        namespace: None,
                    },
                ));
            }
            continue;
        }
        if tool_type != Some("namespace") {
            continue;
        }
        let Some(namespace) = namespace_name(tool) else {
            continue;
        };
        for child in namespace_children(tool) {
            if child.get("type").and_then(Value::as_str) != Some("function") {
                continue;
            }
            let Some(name) = child_name(child) else {
                continue;
            };
            identities.push((
                flatten_tool_name(namespace, name),
                RestoredToolName {
                    name: name.to_string(),
                    namespace: Some(namespace.to_string()),
                },
            ));
        }
    }
    identities
}

/// Codex does not accept arbitrary Responses namespaces, so flatten them before
/// tool-name shortening while preserving its built-in image namespace verbatim.
pub(super) fn flatten_responses_namespaces(object: &mut Map<String, Value>) -> Result<(), String> {
    tool_identity::flatten_responses_namespaces(object, &[PRESERVED_IMAGE_NAMESPACE])?;
    Ok(())
}

fn namespace_name(tool: &Value) -> Option<&str> {
    if tool.get("type").and_then(Value::as_str) != Some("namespace") {
        return None;
    }
    tool.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != PRESERVED_IMAGE_NAMESPACE)
}

fn child_name(child: &Value) -> Option<&str> {
    child
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn namespace_children(tool: &Value) -> &[Value] {
    tool.get("tools")
        .and_then(Value::as_array)
        .or_else(|| tool.get("children").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn flatten_tool_name(namespace: &str, name: &str) -> String {
    format!("{}__{}", namespace.trim(), name.trim())
}
