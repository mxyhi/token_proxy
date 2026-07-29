use serde_json::{json, Map, Value};

// OpenAI Responses tool_choice <-> Anthropic Messages tool_choice mapping
// Mirrors QuantumNous/new-api semantics:
// - "required" <-> "any"
// - parallel_tool_calls <-> disable_parallel_tool_use (negated)

pub fn map_responses_tools_to_anthropic(value: &Value) -> Value {
    let Some(tools) = value.as_array() else {
        return Value::Array(Vec::new());
    };
    let mapped = tools
        .iter()
        .filter_map(map_responses_tool)
        .collect::<Vec<_>>();
    Value::Array(mapped)
}

fn map_responses_tool(value: &Value) -> Option<Value> {
    let tool = value.as_object()?;
    let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
    if is_responses_web_search_tool(tool_type, tool.get("name").and_then(Value::as_str)) {
        let mut out = Map::new();
        out.insert(
            "type".to_string(),
            Value::String("web_search_20250305".to_string()),
        );
        out.insert("name".to_string(), Value::String("web_search".to_string()));
        copy_optional_fields(
            tool,
            &mut out,
            &[
                "max_uses",
                "allowed_domains",
                "blocked_domains",
                "user_location",
            ],
        );
        return Some(Value::Object(out));
    }
    if tool_type != "function" {
        return None;
    }

    // Accept both Responses-style ({name, description, parameters}) and Chat-style ({function:{...}}).
    if let Some(name) = tool.get("name").and_then(Value::as_str) {
        let mut out = Map::new();
        out.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(description) = tool.get("description") {
            out.insert("description".to_string(), description.clone());
        }
        if let Some(parameters) = tool.get("parameters") {
            out.insert(
                "input_schema".to_string(),
                normalize_claude_tool_input_schema(parameters),
            );
        }
        return Some(Value::Object(out));
    }

    let function = tool.get("function").and_then(Value::as_object)?;
    let name = function.get("name").and_then(Value::as_str)?;
    let mut out = Map::new();
    out.insert("name".to_string(), Value::String(name.to_string()));
    if let Some(description) = function.get("description") {
        out.insert("description".to_string(), description.clone());
    }
    if let Some(parameters) = function.get("parameters") {
        out.insert(
            "input_schema".to_string(),
            normalize_claude_tool_input_schema(parameters),
        );
    }
    Some(Value::Object(out))
}

// Claude requires an object at the schema root. Root unions are flattened while
// nested unions remain intact, because nested alternatives are valid properties.
fn normalize_claude_tool_input_schema(schema: &Value) -> Value {
    let Some(root) = schema.as_object() else {
        tracing::debug!("replaced invalid Claude tool input schema with an empty object");
        return empty_claude_tool_input_schema();
    };
    let mut root = root.clone();
    let mut properties = root
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut normalized_unions = 0_u8;

    for union_name in ["anyOf", "oneOf", "allOf"] {
        let Some(union) = root.remove(union_name) else {
            continue;
        };
        normalized_unions = normalized_unions.saturating_add(1);
        let Some(branches) = union.as_array() else {
            continue;
        };
        for branch in branches {
            let Some(branch) = branch
                .as_object()
                .filter(|branch| claude_schema_can_be_object(branch))
            else {
                continue;
            };
            if let Some(branch_properties) = branch.get("properties").and_then(Value::as_object) {
                for (name, property) in branch_properties {
                    properties
                        .entry(name.clone())
                        .or_insert_with(|| property.clone());
                }
            }
            if union_name == "allOf" {
                merge_claude_schema_required(&mut root, branch.get("required"));
            }
        }
    }

    root.insert("type".to_string(), Value::String("object".to_string()));
    root.insert("properties".to_string(), Value::Object(properties));
    if normalized_unions > 0 {
        tracing::debug!(
            root_union_count = normalized_unions,
            "normalized root unions in Claude tool input schema"
        );
    }
    Value::Object(root)
}

fn empty_claude_tool_input_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn claude_schema_can_be_object(schema: &Map<String, Value>) -> bool {
    match schema.get("type") {
        None => true,
        Some(Value::String(schema_type)) => schema_type == "object",
        Some(Value::Array(schema_types)) => schema_types
            .iter()
            .any(|schema_type| schema_type.as_str() == Some("object")),
        Some(_) => false,
    }
}

fn merge_claude_schema_required(root: &mut Map<String, Value>, branch_required: Option<&Value>) {
    let Some(branch_required) = branch_required.and_then(Value::as_array) else {
        return;
    };
    let mut required = root
        .get("required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(|name| Value::String(name.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut seen = required
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>();
    for name in branch_required.iter().filter_map(Value::as_str) {
        if seen.insert(name.to_string()) {
            required.push(Value::String(name.to_string()));
        }
    }
    if !required.is_empty() {
        root.insert("required".to_string(), Value::Array(required));
    }
}

fn is_responses_web_search_tool(tool_type: &str, tool_name: Option<&str>) -> bool {
    matches!(
        tool_type,
        "google_search" | "web_search" | "web_search_preview" | "web_search_20250305"
    ) || matches!(tool_name, Some("google_search" | "web_search"))
}

fn copy_optional_fields(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    fields: &[&str],
) {
    for field in fields {
        if let Some(value) = source.get(*field) {
            target.insert((*field).to_string(), value.clone());
        }
    }
}

pub fn map_anthropic_tools_to_responses(value: &Value) -> Value {
    let Some(tools) = value.as_array() else {
        return Value::Array(Vec::new());
    };
    let mapped = tools
        .iter()
        .filter_map(map_anthropic_tool)
        .collect::<Vec<_>>();
    Value::Array(mapped)
}

fn map_anthropic_tool(value: &Value) -> Option<Value> {
    let tool = value.as_object()?;
    let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
    let tool_name = tool.get("name").and_then(Value::as_str).unwrap_or("");
    if tool_type.starts_with("web_search") || tool_name == "web_search" {
        return Some(json!({ "type": "web_search_preview" }));
    }
    let name = tool.get("name").and_then(Value::as_str)?;
    let mut out = Map::new();
    out.insert("type".to_string(), json!("function"));
    out.insert("name".to_string(), Value::String(name.to_string()));
    if let Some(description) = tool.get("description") {
        out.insert("description".to_string(), description.clone());
    }
    out.insert(
        "parameters".to_string(),
        normalize_anthropic_input_schema(tool.get("input_schema")),
    );
    out.insert("strict".to_string(), Value::Bool(false));
    Some(Value::Object(out))
}

fn normalize_anthropic_input_schema(input_schema: Option<&Value>) -> Value {
    let Some(Value::Object(schema)) = input_schema else {
        return json!({ "type": "object", "properties": {} });
    };
    let mut schema = schema.clone();
    if schema.get("type").and_then(Value::as_str) == Some("object")
        && !schema.contains_key("properties")
    {
        schema.insert("properties".to_string(), json!({}));
    }
    Value::Object(schema)
}

pub fn map_responses_tool_choice_to_anthropic(
    tool_choice: Option<&Value>,
    parallel_tool_calls: Option<bool>,
) -> Option<Value> {
    let mut out = match tool_choice {
        None => None,
        Some(Value::String(choice)) => match choice.as_str() {
            "auto" => Some(json!({ "type": "auto" })),
            "required" => Some(json!({ "type": "any" })),
            "none" => Some(json!({ "type": "none" })),
            _ => None,
        },
        Some(Value::Object(choice)) => {
            if choice.get("type").and_then(Value::as_str) != Some("function") {
                None
            } else {
                let name = choice.get("name").and_then(Value::as_str).unwrap_or("");
                if name.is_empty() {
                    None
                } else {
                    Some(json!({ "type": "tool", "name": name }))
                }
            }
        }
        _ => None,
    };

    if let Some(parallel) = parallel_tool_calls {
        let disable_parallel = !parallel;
        if out.is_none() {
            out = Some(json!({ "type": "auto" }));
        }
        if let Some(Value::Object(object)) = out.as_mut() {
            object.insert(
                "disable_parallel_tool_use".to_string(),
                Value::Bool(disable_parallel),
            );
        }
    }

    out
}

pub fn map_anthropic_tool_choice_to_responses(
    tool_choice: Option<&Value>,
) -> (Option<Value>, Option<bool>) {
    let Some(tool_choice) = tool_choice.and_then(Value::as_object) else {
        return (None, None);
    };

    let choice_type = tool_choice
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mapped_choice = match choice_type {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => {
            let name = tool_choice
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name.is_empty() {
                None
            } else {
                Some(json!({ "type": "function", "name": name }))
            }
        }
        _ => None,
    };

    let parallel_tool_calls = tool_choice
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        .map(|disable| !disable);

    (mapped_choice, parallel_tool_calls)
}

pub fn map_openai_stop_to_anthropic_stop_sequences(stop: Option<&Value>) -> Option<Value> {
    let stop = stop?;
    match stop {
        Value::String(_) => Some(Value::Array(vec![stop.clone()])),
        Value::Array(items) => Some(Value::Array(items.clone())),
        _ => None,
    }
}

pub fn map_anthropic_stop_sequences_to_openai_stop(stop: Option<&Value>) -> Option<Value> {
    let stop = stop?;
    let items = stop.as_array()?;
    match items.len() {
        0 => None,
        1 => Some(items[0].clone()),
        _ => Some(Value::Array(items.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_tools_normalize_root_schema_unions_for_claude() {
        let mapped = map_responses_tools_to_anthropic(&json!([
            {
                "type": "function",
                "name": "any_tool",
                "parameters": {
                    "anyOf": [
                        {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
                        {"type": "object", "properties": {"b": {"type": "integer"}}, "required": ["b"]}
                    ]
                }
            },
            {
                "type": "function",
                "name": "one_tool",
                "parameters": {
                    "type": "object",
                    "properties": {"nested": {"oneOf": [{"type": "string"}, {"type": "number"}]}},
                    "oneOf": [
                        {"properties": {"a": {"type": "string"}}, "required": ["a"]},
                        {"properties": {"b": {"type": "string"}}, "required": ["b"]}
                    ]
                }
            },
            {
                "type": "function",
                "name": "all_tool",
                "parameters": {
                    "type": "object",
                    "properties": {"base": {"type": "boolean"}},
                    "required": ["base"],
                    "allOf": [
                        {"properties": {"a": {"type": "string"}}, "required": ["a"]},
                        {"properties": {"b": {"type": "integer"}}, "required": ["a", "b"]}
                    ]
                }
            }
        ]));

        assert_eq!(
            mapped[0]["input_schema"],
            json!({
                "type": "object",
                "properties": {"a": {"type": "string"}, "b": {"type": "integer"}}
            })
        );
        assert_eq!(
            mapped[1]["input_schema"],
            json!({
                "type": "object",
                "properties": {
                    "nested": {"oneOf": [{"type": "string"}, {"type": "number"}]},
                    "a": {"type": "string"},
                    "b": {"type": "string"}
                }
            })
        );
        assert_eq!(
            mapped[2]["input_schema"],
            json!({
                "type": "object",
                "properties": {
                    "base": {"type": "boolean"},
                    "a": {"type": "string"},
                    "b": {"type": "integer"}
                },
                "required": ["base", "a", "b"]
            })
        );
    }

    #[test]
    fn chat_tools_normalize_invalid_and_preserve_object_schemas() {
        let mapped = map_responses_tools_to_anthropic(&json!([
            {
                "type": "function",
                "function": {"name": "invalid", "parameters": true}
            },
            {
                "type": "function",
                "function": {
                    "name": "ordinary",
                    "parameters": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}},
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }
            }
        ]));

        assert_eq!(
            mapped[0]["input_schema"],
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            mapped[1]["input_schema"],
            json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
                "additionalProperties": false
            })
        );
    }
}
