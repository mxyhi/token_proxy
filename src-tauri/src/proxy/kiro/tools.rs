use serde_json::{Map, Value};

use super::types::{KiroInputSchema, KiroToolSpecification, KiroToolWrapper};

const KIRO_MAX_TOOL_DESC_LEN: usize = 10237;
const TOOL_COMPRESSION_TARGET_SIZE: usize = 20 * 1024;
const MIN_TOOL_DESCRIPTION_LENGTH: usize = 50;

pub(crate) fn convert_openai_tools(tools: Option<&Value>, is_chat_only: bool) -> Vec<KiroToolWrapper> {
    if is_chat_only {
        return Vec::new();
    }
    let Some(Value::Array(items)) = tools else {
        return Vec::new();
    };

    let mut output = Vec::new();
    for item in items {
        let Some(tool) = item.as_object() else {
            continue;
        };
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("function");
        if tool_type != "function" {
            continue;
        }
        let (name, description, parameters) = match extract_tool_fields(tool) {
            Some(fields) => fields,
            None => continue,
        };
        if name.is_empty() {
            continue;
        }
        let name = shorten_tool_name(name);
        let mut description = description.to_string();
        if description.trim().is_empty() {
            description = format!("Tool: {name}");
        }
        if description.len() > KIRO_MAX_TOOL_DESC_LEN {
            description = truncate_utf8(&description, KIRO_MAX_TOOL_DESC_LEN - 30)
                + "... (description truncated)";
        }
        let parameters = parameters.unwrap_or_else(|| Value::Object(Map::new()));

        output.push(KiroToolWrapper {
            tool_specification: KiroToolSpecification {
                name: name.to_string(),
                description,
                input_schema: KiroInputSchema { json: parameters },
            },
        });
    }

    compress_tools_if_needed(output)
}

fn extract_tool_fields(tool: &Map<String, Value>) -> Option<(&str, &str, Option<Value>)> {
    if let Some(function) = tool.get("function").and_then(Value::as_object) {
        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let parameters = function.get("parameters").cloned();
        return Some((name, description, parameters));
    }

    let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let parameters = tool
        .get("parameters")
        .cloned()
        .or_else(|| tool.get("input_schema").cloned());
    Some((name, description, parameters))
}

fn shorten_tool_name(name: &str) -> String {
    const LIMIT: usize = 64;
    if name.len() <= LIMIT {
        return name.to_string();
    }
    if let Some(stripped) = name.strip_prefix("mcp__") {
        if let Some(idx) = stripped.rfind("__") {
            let suffix = &stripped[idx + 2..];
            let candidate = format!("mcp__{suffix}");
            if candidate.len() <= LIMIT {
                return candidate;
            }
            return candidate.chars().take(LIMIT).collect();
        }
    }
    name.chars().take(LIMIT).collect()
}

fn truncate_utf8(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    let mut end = max_len;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn compress_tools_if_needed(tools: Vec<KiroToolWrapper>) -> Vec<KiroToolWrapper> {
    if tools.is_empty() {
        return tools;
    }
    let original_size = calculate_tools_size(&tools);
    if original_size <= TOOL_COMPRESSION_TARGET_SIZE {
        return tools;
    }

    let mut compressed = tools
        .into_iter()
        .map(|tool| KiroToolWrapper {
            tool_specification: KiroToolSpecification {
                name: tool.tool_specification.name,
                description: tool.tool_specification.description,
                input_schema: KiroInputSchema {
                    json: tool.tool_specification.input_schema.json,
                },
            },
        })
        .collect::<Vec<_>>();

    for tool in &mut compressed {
        tool.tool_specification.input_schema.json =
            simplify_schema(&tool.tool_specification.input_schema.json);
    }

    let size_after_schema = calculate_tools_size(&compressed);
    if size_after_schema <= TOOL_COMPRESSION_TARGET_SIZE {
        return compressed;
    }

    let size_to_reduce = (size_after_schema - TOOL_COMPRESSION_TARGET_SIZE) as f64;
    let total_desc_len: f64 = compressed
        .iter()
        .map(|tool| tool.tool_specification.description.len() as f64)
        .sum();

    if total_desc_len > 0.0 {
        let mut keep_ratio = 1.0 - (size_to_reduce / total_desc_len);
        if keep_ratio > 1.0 {
            keep_ratio = 1.0;
        }
        if keep_ratio < 0.0 {
            keep_ratio = 0.0;
        }
        for tool in &mut compressed {
            let desc = tool.tool_specification.description.clone();
            let target_len = (desc.len() as f64 * keep_ratio) as usize;
            tool.tool_specification.description = compress_description(&desc, target_len);
        }
    }

    compressed
}

fn compress_description(description: &str, target_len: usize) -> String {
    let mut target = target_len;
    if target < MIN_TOOL_DESCRIPTION_LENGTH {
        target = MIN_TOOL_DESCRIPTION_LENGTH;
    }
    if description.len() <= target {
        return description.to_string();
    }
    let trimmed = truncate_utf8(description, target.saturating_sub(3));
    format!("{trimmed}...")
}

fn calculate_tools_size(tools: &[KiroToolWrapper]) -> usize {
    serde_json::to_vec(tools).map(|data| data.len()).unwrap_or(0)
}

fn simplify_schema(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let mut simplified = Map::new();

    for key in ["type", "enum", "required"] {
        if let Some(val) = object.get(key) {
            simplified.insert(key.to_string(), val.clone());
        }
    }

    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        let mut simplified_props = Map::new();
        for (key, val) in properties {
            simplified_props.insert(key.clone(), simplify_schema(val));
        }
        simplified.insert("properties".to_string(), Value::Object(simplified_props));
    }

    if let Some(items) = object.get("items") {
        simplified.insert("items".to_string(), simplify_schema(items));
    }

    if let Some(additional) = object.get("additionalProperties") {
        simplified.insert(
            "additionalProperties".to_string(),
            simplify_schema(additional),
        );
    }

    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(values)) = object.get(key) {
            let simplified_values = values.iter().map(simplify_schema).collect::<Vec<_>>();
            simplified.insert(key.to_string(), Value::Array(simplified_values));
        }
    }

    Value::Object(simplified)
}
