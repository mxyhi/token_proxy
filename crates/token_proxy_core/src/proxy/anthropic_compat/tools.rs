use serde_json::{json, Map, Value};

// OpenAI Responses tool_choice <-> Anthropic Messages tool_choice mapping
// Mirrors QuantumNous/new-api semantics:
// - "required" <-> "any"
// - parallel_tool_calls <-> disable_parallel_tool_use (negated)

pub(super) fn map_responses_tools_to_anthropic(value: &Value) -> Value {
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
            out.insert("input_schema".to_string(), parameters.clone());
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
        out.insert("input_schema".to_string(), parameters.clone());
    }
    Some(Value::Object(out))
}

pub(super) fn map_anthropic_tools_to_responses(value: &Value) -> Value {
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
    if let Some(input_schema) = tool.get("input_schema") {
        out.insert("parameters".to_string(), input_schema.clone());
    }
    Some(Value::Object(out))
}

pub(super) fn map_responses_tool_choice_to_anthropic(
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

pub(super) fn map_anthropic_tool_choice_to_responses(
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

pub(super) fn map_openai_stop_to_anthropic_stop_sequences(stop: Option<&Value>) -> Option<Value> {
    let Some(stop) = stop else {
        return None;
    };
    match stop {
        Value::String(_) => Some(Value::Array(vec![stop.clone()])),
        Value::Array(items) => Some(Value::Array(items.clone())),
        _ => None,
    }
}

pub(super) fn map_anthropic_stop_sequences_to_openai_stop(stop: Option<&Value>) -> Option<Value> {
    let Some(stop) = stop else {
        return None;
    };
    let Some(items) = stop.as_array() else {
        return None;
    };
    match items.len() {
        0 => None,
        1 => Some(items[0].clone()),
        _ => Some(Value::Array(items.clone())),
    }
}
