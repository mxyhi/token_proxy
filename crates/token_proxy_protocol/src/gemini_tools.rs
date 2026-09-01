//! OpenAI Chat ↔ Gemini 工具定义转换

use serde_json::{json, Map, Value};

const GEMINI_UNSUPPORTED_SCHEMA_KEYS: &[&str] = &[
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "definitions",
    "additionalProperties",
    "patternProperties",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "contains",
    "exclusiveMinimum",
    "if",
    "then",
    "else",
    "deprecated",
];

const SCHEMA_CONTAINER_KEYS: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "prefixItems",
    "anyOf",
    "oneOf",
    "allOf",
    "$defs",
    "definitions",
    "description",
    "title",
    "enum",
    "format",
    "default",
    "const",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "pattern",
    "additionalProperties",
    "additionalItems",
    "contains",
    "not",
    "if",
    "then",
    "else",
];

/// 将 OpenAI Chat 格式的 tools 转换为 Gemini 格式的 functionDeclarations
pub fn map_chat_tools_to_gemini(tools: &Value) -> Value {
    let Some(tools) = tools.as_array() else {
        return json!([]);
    };

    let declarations: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            let tool = tool.as_object()?;
            if !matches!(
                tool.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) {
                return None;
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .unwrap_or(tool);
            let name = function.get("name").and_then(Value::as_str)?;
            let description = function
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let parameters = function
                .get("parameters")
                .or_else(|| function.get("format"))
                .map(clean_tool_schema)
                .unwrap_or_else(|| json!({}));
            Some(json!({
                "name": name,
                "description": description,
                "parameters": parameters
            }))
        })
        .collect();

    json!([{
        "functionDeclarations": declarations
    }])
}

fn clean_tool_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(object) => {
            clean_tool_schema_object(&normalize_malformed_schema_object(object))
        }
        Value::Array(items) => Value::Array(items.iter().map(clean_tool_schema).collect()),
        other => other.clone(),
    }
}

fn clean_tool_schema_object(object: &Map<String, Value>) -> Value {
    let mut source = object.clone();
    merge_conditional_properties(&mut source, object.get("then"));
    merge_conditional_properties(&mut source, object.get("else"));
    let all_of = source.get("allOf").cloned();
    let any_of = source.get("anyOf").cloned();
    let one_of = source.get("oneOf").cloned();
    let contains_hint = source.get("contains").and_then(schema_constraint_hint);
    let mut cleaned = Map::new();
    for (key, value) in &source {
        if GEMINI_UNSUPPORTED_SCHEMA_KEYS.contains(&key.as_str()) || key == "allOf" {
            continue;
        }
        if key == "properties" {
            let properties = value
                .as_object()
                .map(|properties| {
                    properties
                        .iter()
                        .map(|(name, schema)| (name.clone(), clean_tool_schema(schema)))
                        .collect::<Map<String, Value>>()
                })
                .unwrap_or_default();
            cleaned.insert(key.clone(), Value::Object(properties));
        } else if matches!(key.as_str(), "$defs" | "definitions") {
            let definitions = value
                .as_object()
                .map(|definitions| {
                    definitions
                        .iter()
                        .map(|(name, schema)| (name.clone(), clean_tool_schema(schema)))
                        .collect::<Map<String, Value>>()
                })
                .unwrap_or_default();
            cleaned.insert(key.clone(), Value::Object(definitions));
        } else {
            cleaned.insert(key.clone(), clean_tool_schema(value));
        }
    }
    normalize_gemini_enum(&mut cleaned);
    normalize_gemini_schema_type(&mut cleaned);
    normalize_integer_exclusive_minimum(&mut cleaned, object.get("exclusiveMinimum"));
    merge_all_of_properties(&mut cleaned, all_of.as_ref());
    merge_union_properties(&mut cleaned, "anyOf", any_of.as_ref());
    merge_union_properties(&mut cleaned, "oneOf", one_of.as_ref());
    if let Some(hint) = contains_hint {
        append_schema_description(&mut cleaned, &format!("contains: {hint}"));
    }
    if cleaned.get("type").and_then(Value::as_str) == Some("ARRAY")
        && !cleaned.contains_key("items")
    {
        cleaned.insert("items".to_string(), json!({ "type": "STRING" }));
    }
    if !cleaned.contains_key("properties") {
        cleaned.remove("required");
    }
    Value::Object(cleaned)
}

fn schema_constraint_hint(value: &Value) -> Option<String> {
    match value {
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok(),
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

fn append_schema_description(cleaned: &mut Map<String, Value>, hint: &str) {
    let description = cleaned
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let combined = if description.is_empty() {
        hint.to_string()
    } else {
        format!("{description}; {hint}")
    };
    cleaned.insert("description".to_string(), Value::String(combined));
}

fn merge_union_properties(cleaned: &mut Map<String, Value>, key: &str, union: Option<&Value>) {
    let Some(union) = union.and_then(Value::as_array) else {
        return;
    };
    let branches = union.iter().map(clean_tool_schema).collect::<Vec<_>>();
    let mut branch_properties = Map::new();
    let mut accepted_types = Vec::new();
    for branch in &branches {
        if let Some(schema_type) = branch.get("type").and_then(Value::as_str) {
            if !accepted_types.iter().any(|value| value == schema_type) {
                accepted_types.push(schema_type.to_string());
            }
        }
        if let Some(properties) = branch.get("properties").and_then(Value::as_object) {
            for (name, schema) in properties {
                branch_properties
                    .entry(name.clone())
                    .or_insert_with(|| schema.clone());
            }
        }
    }

    if !branch_properties.is_empty() {
        let properties = cleaned
            .entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            for (name, schema) in branch_properties {
                properties.entry(name).or_insert(schema);
            }
            cleaned
                .entry("type".to_string())
                .or_insert_with(|| Value::String("OBJECT".to_string()));
        }
    } else if let Some(selected) = branches.first().and_then(Value::as_object) {
        for (name, value) in selected {
            if name != "description" {
                cleaned.entry(name.clone()).or_insert_with(|| value.clone());
            }
        }
    }

    cleaned.remove(key);
    if accepted_types.len() > 1 {
        append_schema_description(cleaned, &format!("Accepts: {}", accepted_types.join(" | ")));
    }
}

fn normalize_malformed_schema_object(object: &Map<String, Value>) -> Map<String, Value> {
    let mut normalized = object.clone();
    let is_bare_property_map = !object.is_empty()
        && !object.contains_key("type")
        && !object
            .keys()
            .any(|key| SCHEMA_CONTAINER_KEYS.contains(&key.as_str()))
        && object.values().all(Value::is_object);
    if is_bare_property_map {
        let (properties, required) = normalize_properties(object);
        normalized.clear();
        normalized.insert("type".to_string(), Value::String("object".to_string()));
        normalized.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            normalized.insert("required".to_string(), json!(required));
        }
        return normalized;
    }

    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        let (properties, promoted) = normalize_properties(properties);
        normalized.insert("properties".to_string(), Value::Object(properties));
        if !promoted.is_empty() {
            let mut required = object
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            for name in promoted {
                if !required.contains(&name) {
                    required.push(name);
                }
            }
            normalized.insert("required".to_string(), json!(required));
        }
    }
    normalized
}

fn normalize_properties(properties: &Map<String, Value>) -> (Map<String, Value>, Vec<String>) {
    let mut normalized = Map::new();
    let mut required = Vec::new();
    for (name, value) in properties {
        let Some(object) = value.as_object() else {
            normalized.insert(name.clone(), value.clone());
            continue;
        };
        let mut child = object.clone();
        if let Some(Value::Bool(is_required)) = child.remove("required") {
            if is_required {
                required.push(name.clone());
            }
        }
        normalized.insert(name.clone(), Value::Object(child));
    }
    (normalized, required)
}

fn normalize_gemini_enum(cleaned: &mut Map<String, Value>) {
    let Some(Value::Array(values)) = cleaned.get("enum") else {
        return;
    };
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        match value {
            Value::String(value) => normalized.push(Value::String(value.clone())),
            Value::Null | Value::Bool(_) | Value::Number(_) => {
                normalized.push(Value::String(value.to_string()))
            }
            Value::Array(_) | Value::Object(_) => {
                cleaned.remove("enum");
                return;
            }
        }
    }
    cleaned.insert("enum".to_string(), Value::Array(normalized));
}

fn merge_conditional_properties(target: &mut Map<String, Value>, branch: Option<&Value>) {
    let Some(branch_properties) = branch
        .and_then(|value| value.get("properties"))
        .and_then(Value::as_object)
    else {
        return;
    };
    let properties = target
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(properties) = properties.as_object_mut() else {
        return;
    };
    for (name, schema) in branch_properties {
        properties
            .entry(name.clone())
            .or_insert_with(|| schema.clone());
    }
}

fn merge_all_of_properties(cleaned: &mut Map<String, Value>, all_of: Option<&Value>) {
    let Some(branches) = all_of.and_then(Value::as_array) else {
        return;
    };
    for branch in branches {
        let branch = clean_tool_schema(branch);
        let Some(branch_properties) = branch.get("properties").and_then(Value::as_object) else {
            continue;
        };
        let properties = cleaned
            .entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(properties) = properties.as_object_mut() else {
            return;
        };
        for (name, schema) in branch_properties {
            properties
                .entry(name.clone())
                .or_insert_with(|| schema.clone());
        }
    }
}

fn normalize_integer_exclusive_minimum(
    cleaned: &mut Map<String, Value>,
    exclusive_minimum: Option<&Value>,
) {
    if cleaned.get("type").and_then(Value::as_str) != Some("INTEGER") {
        return;
    }
    let Some(candidate) = exclusive_minimum.and_then(increment_integral_bound) else {
        return;
    };
    let should_replace = match cleaned.get("minimum") {
        None => true,
        Some(existing) => schema_number(existing)
            .zip(schema_number(&candidate))
            .is_some_and(|(existing, candidate)| existing < candidate),
    };
    if should_replace {
        cleaned.insert("minimum".to_string(), candidate);
    }
}

fn increment_integral_bound(value: &Value) -> Option<Value> {
    let number = value.as_number()?;
    if let Some(value) = number.as_i64() {
        return value.checked_add(1).map(Value::from);
    }
    if let Some(value) = number.as_u64() {
        return value.checked_add(1).map(Value::from);
    }
    let value = number.as_f64()?;
    if !value.is_finite() || value.fract() != 0.0 || value + 1.0 <= value {
        return None;
    }
    serde_json::Number::from_f64(value + 1.0).map(Value::Number)
}

fn schema_number(value: &Value) -> Option<f64> {
    value
        .as_number()?
        .as_f64()
        .filter(|value| value.is_finite())
}

fn normalize_gemini_schema_type(object: &mut Map<String, Value>) {
    match object.get("type") {
        Some(Value::String(schema_type)) => {
            object.insert(
                "type".to_string(),
                Value::String(schema_type.to_ascii_uppercase()),
            );
        }
        Some(Value::Array(schema_types)) => {
            let normalized = schema_types
                .iter()
                .filter_map(Value::as_str)
                .find(|schema_type| !schema_type.eq_ignore_ascii_case("null"))
                .map(str::to_ascii_uppercase);
            match normalized {
                Some(schema_type) => {
                    object.insert("type".to_string(), Value::String(schema_type));
                }
                None => {
                    object.remove("type");
                }
            }
        }
        _ => {}
    }
}

/// 将 OpenAI Chat 格式的 tool_choice 转换为 Gemini 格式的 toolConfig
pub fn map_chat_tool_choice_to_gemini(tool_choice: &Value) -> Option<Value> {
    match tool_choice {
        Value::String(s) => match s.as_str() {
            "none" => Some(json!({ "functionCallingConfig": { "mode": "NONE" } })),
            "auto" => Some(json!({ "functionCallingConfig": { "mode": "AUTO" } })),
            "required" => Some(json!({ "functionCallingConfig": { "mode": "ANY" } })),
            _ => None,
        },
        Value::Object(obj) => {
            // { "type": "function", "function": { "name": "..." } }
            if obj.get("type").and_then(Value::as_str) == Some("function") {
                if let Some(function) = obj.get("function").and_then(Value::as_object) {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        return Some(json!({
                            "functionCallingConfig": {
                                "mode": "ANY",
                                "allowedFunctionNames": [name]
                            }
                        }));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// 将 Gemini 格式的 tools 转换为 OpenAI Chat 格式的 tools
pub fn map_gemini_tools_to_chat(value: &Value) -> Value {
    let Some(groups) = value.as_array() else {
        return json!([]);
    };

    let mut tools = Vec::new();
    for group in groups {
        let Some(group) = group.as_object() else {
            continue;
        };
        let Some(declarations) = group.get("functionDeclarations").and_then(Value::as_array) else {
            continue;
        };
        for declaration in declarations {
            let Some(declaration) = declaration.as_object() else {
                continue;
            };
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let description = declaration
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let parameters = declaration
                .get("parameters")
                .or_else(|| declaration.get("parametersJsonSchema"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": parameters
                }
            }));
        }
    }

    Value::Array(tools)
}

/// 将 Gemini 格式的 toolConfig 转换为 OpenAI Chat 格式的 tool_choice
pub fn map_gemini_tool_config_to_chat(value: &Value) -> Option<Value> {
    let tool_config = value.as_object()?;
    let config = tool_config
        .get("functionCallingConfig")
        .and_then(Value::as_object)?;

    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("");
    match mode {
        "NONE" => Some(Value::String("none".to_string())),
        "AUTO" => Some(Value::String("auto".to_string())),
        "ANY" => {
            let allowed = config
                .get("allowedFunctionNames")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if allowed.len() == 1 {
                let name = allowed
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() {
                    return Some(json!({
                        "type": "function",
                        "function": { "name": name }
                    }));
                }
            }
            Some(Value::String("required".to_string()))
        }
        _ => None,
    }
}

/// 将 Gemini 格式的 functionCall 转换为 OpenAI Chat 格式的 tool_call
pub fn gemini_function_call_to_chat_tool_call(
    function_call: &serde_json::Map<String, Value>,
    index: usize,
) -> Value {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let args = function_call
        .get("args")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arguments = match args {
        Value::String(s) => s,
        other => serde_json::to_string(&other).unwrap_or_else(|_| "{}".to_string()),
    };

    json!({
        "id": format!("call_gemini_{index}"),
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_schema_normalizes_integer_exclusive_minimum() {
        let schema = json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer", "exclusiveMinimum": 2, "minimum": 1 },
                "strict": { "type": "integer", "exclusiveMinimum": 2, "minimum": 5 },
                "fractional": { "type": "integer", "exclusiveMinimum": 0.5 },
                "number": { "type": "number", "exclusiveMinimum": 0 }
            }
        });

        let cleaned = clean_tool_schema(&schema);

        assert_eq!(cleaned["properties"]["count"]["minimum"], 3);
        assert_eq!(cleaned["properties"]["strict"]["minimum"], 5);
        assert!(cleaned["properties"]["fractional"].get("minimum").is_none());
        assert!(cleaned["properties"]["number"].get("minimum").is_none());
        for name in ["count", "strict", "fractional", "number"] {
            assert!(cleaned["properties"][name]
                .get("exclusiveMinimum")
                .is_none());
        }
    }

    #[test]
    fn clean_schema_merges_conditional_properties_at_root_and_nested_paths() {
        let schema = json!({
            "type": "object",
            "properties": {
                "existing": { "type": "string", "description": "keep" },
                "nested": {
                    "type": "object",
                    "then": { "properties": { "nested_then": { "type": "string" } } }
                }
            },
            "if": { "properties": { "kind": { "const": "a" } } },
            "then": { "properties": {
                "existing": { "type": "number" },
                "from_then": { "type": "string" }
            } },
            "else": { "properties": { "from_else": { "type": "integer" } } }
        });

        let cleaned = clean_tool_schema(&schema);

        assert_eq!(cleaned["properties"]["existing"]["type"], "STRING");
        assert_eq!(cleaned["properties"]["from_then"]["type"], "STRING");
        assert_eq!(cleaned["properties"]["from_else"]["type"], "INTEGER");
        assert_eq!(
            cleaned["properties"]["nested"]["properties"]["nested_then"]["type"],
            "STRING"
        );
        assert!(!cleaned.to_string().contains("\"then\""));
        assert!(!cleaned.to_string().contains("\"else\""));
        assert!(!cleaned.to_string().contains("\"if\""));
    }

    #[test]
    fn clean_schema_hoists_conditional_properties_from_all_of() {
        let schema = json!({
            "type": "object",
            "allOf": [{
                "if": { "properties": { "kind": { "const": "sell" } } },
                "then": { "properties": {
                    "reason": { "type": "string", "description": "why" }
                } }
            }]
        });

        let cleaned = clean_tool_schema(&schema);

        assert_eq!(cleaned["properties"]["reason"]["type"], "STRING");
        assert_eq!(cleaned["properties"]["reason"]["description"], "why");
        assert!(cleaned.get("allOf").is_none());
    }

    #[test]
    fn clean_schema_repairs_bare_properties_and_boolean_required_recursively() {
        let schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "parent": { "type": "string", "required": true },
                    "tasks": {
                        "type": "array",
                        "items": { "name": { "type": "string", "required": true } }
                    }
                }
            }
        });

        let cleaned = clean_tool_schema(&schema);
        assert_eq!(cleaned["properties"]["data"]["type"], "OBJECT");
        assert_eq!(cleaned["properties"]["data"]["required"], json!(["parent"]));
        assert!(cleaned["properties"]["data"]["properties"]["parent"]
            .get("required")
            .is_none());
        assert_eq!(
            cleaned["properties"]["data"]["properties"]["tasks"]["items"]["type"],
            "OBJECT"
        );
        assert_eq!(
            cleaned["properties"]["data"]["properties"]["tasks"]["items"]["required"],
            json!(["name"])
        );
    }

    #[test]
    fn clean_schema_flattens_scalar_unions_and_drops_invalid_enum() {
        let schema = json!({
            "anyOf": [
                { "deprecated": true, "enum": ["on", false, 1, null] },
                { "enum": ["ok", { "invalid": true }] }
            ],
            "$defs": { "nested": { "deprecated": true, "enum": [2] } }
        });

        let cleaned = clean_tool_schema(&schema);
        assert_eq!(cleaned["enum"], json!(["on", "false", "1", "null"]));
        assert!(cleaned.get("anyOf").is_none());
        assert!(cleaned.get("$defs").is_none());
    }

    #[test]
    fn clean_schema_merges_union_properties_and_preserves_contains_hint() {
        let schema = json!({
            "type": "object",
            "description": "query",
            "properties": { "base": { "type": "string" } },
            "required": ["base"],
            "anyOf": [
                { "properties": { "from_any": { "type": "integer" } }, "required": ["from_any"] },
                { "properties": { "other": { "type": "boolean" } } }
            ],
            "contains": { "type": "string", "minLength": 2 }
        });

        let cleaned = clean_tool_schema(&schema);

        assert!(cleaned.get("anyOf").is_none());
        assert_eq!(cleaned["properties"]["from_any"]["type"], "INTEGER");
        assert_eq!(cleaned["properties"]["other"]["type"], "BOOLEAN");
        assert_eq!(cleaned["required"], json!(["base"]));
        assert!(cleaned["description"]
            .as_str()
            .unwrap()
            .contains("contains:"));
        assert!(cleaned.get("contains").is_none());
    }

    #[test]
    fn clean_schema_flattens_root_union_properties_without_leaking_union_keywords() {
        let schema = json!({
            "oneOf": [
                { "type": "object", "properties": { "city": { "type": "string" } } },
                { "type": "object", "properties": { "coordinates": { "type": "array" } } }
            ]
        });

        let cleaned = clean_tool_schema(&schema);

        assert_eq!(cleaned["type"], "OBJECT");
        assert_eq!(cleaned["properties"]["city"]["type"], "STRING");
        assert_eq!(cleaned["properties"]["coordinates"]["type"], "ARRAY");
        assert_eq!(
            cleaned["properties"]["coordinates"]["items"],
            json!({ "type": "STRING" })
        );
        assert!(cleaned.get("oneOf").is_none());
        assert!(cleaned.get("anyOf").is_none());
    }

    #[test]
    fn clean_schema_removes_required_without_properties_and_repairs_array_items() {
        let schema = json!({
            "type": "array",
            "required": ["missing"]
        });

        let cleaned = clean_tool_schema(&schema);

        assert!(cleaned.get("required").is_none());
        assert_eq!(cleaned["items"], json!({ "type": "STRING" }));
    }

    #[test]
    fn clean_schema_drops_contains_in_a_schema_without_other_keywords() {
        let schema = json!({
            "contains": { "type": "string" }
        });

        let cleaned = clean_tool_schema(&schema);

        assert!(cleaned.get("contains").is_none());
        assert!(cleaned.get("properties").is_none());
        assert!(cleaned["description"]
            .as_str()
            .is_some_and(|description| description.contains("contains:")));
    }
}
