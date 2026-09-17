use serde_json::{json, Map, Value};

pub(super) fn split_responses_tools_for_chat(value: &Value) -> (Vec<Value>, Option<Value>) {
    let Some(tools) = value.as_array() else {
        return (Vec::new(), None);
    };

    let mut mapped = Vec::new();
    let mut web_search_options = None;
    for tool in tools {
        if let Some(options) = responses_web_search_tool_to_chat_options(tool) {
            web_search_options = Some(options);
            continue;
        }
        mapped.push(map_responses_tool_to_chat(tool));
    }
    (mapped, web_search_options)
}

fn map_responses_tool_to_chat(value: &Value) -> Value {
    let Some(tool) = value.as_object() else {
        return value.clone();
    };

    if tool.get("function").and_then(Value::as_object).is_some() {
        return value.clone();
    }
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return value.clone();
    }

    let mut function = Map::new();
    if let Some(name) = tool.get("name") {
        function.insert("name".to_string(), name.clone());
    }
    if let Some(description) = tool.get("description") {
        function.insert("description".to_string(), description.clone());
    }
    if let Some(parameters) = normalize_tool_parameters(tool.get("parameters")) {
        function.insert("parameters".to_string(), parameters.clone());
    }
    copy_optional_tool_key(tool, &mut function, "strict");

    let mut output = Map::new();
    output.insert("type".to_string(), json!("function"));
    output.insert("function".to_string(), Value::Object(function));
    copy_optional_tool_key(tool, &mut output, "cache_control");
    copy_optional_tool_key(tool, &mut output, "defer_loading");
    copy_optional_tool_key(tool, &mut output, "allowed_callers");
    copy_optional_tool_key(tool, &mut output, "input_examples");

    Value::Object(output)
}

pub(super) fn map_chat_tools_to_responses(value: &Value) -> Value {
    let Some(tools) = value.as_array() else {
        return value.clone();
    };
    let mapped = tools
        .iter()
        .map(map_chat_tool_to_responses)
        .collect::<Vec<_>>();
    Value::Array(mapped)
}

fn map_chat_tool_to_responses(value: &Value) -> Value {
    let Some(tool) = value.as_object() else {
        return value.clone();
    };

    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return value.clone();
    }
    if tool.get("name").and_then(Value::as_str).is_some() {
        return value.clone();
    }
    let Some(function) = tool.get("function").and_then(Value::as_object) else {
        return value.clone();
    };

    let mut output = Map::new();
    output.insert("type".to_string(), json!("function"));
    if let Some(name) = function.get("name") {
        output.insert("name".to_string(), name.clone());
    }
    if let Some(description) = function.get("description") {
        output.insert("description".to_string(), description.clone());
    }
    if let Some(parameters) = normalize_tool_parameters(function.get("parameters")) {
        output.insert("parameters".to_string(), parameters);
    }
    // OpenAI-compatible Responses tools expect an explicit `strict` boolean.
    output.insert(
        "strict".to_string(),
        function
            .get("strict")
            .cloned()
            .unwrap_or_else(|| json!(false)),
    );
    copy_optional_tool_key(tool, &mut output, "cache_control");
    copy_optional_tool_key(tool, &mut output, "defer_loading");
    copy_optional_tool_key(tool, &mut output, "allowed_callers");
    copy_optional_tool_key(tool, &mut output, "input_examples");
    Value::Object(output)
}

pub(super) fn map_responses_tool_choice_to_chat(value: &Value) -> Value {
    if let Some(choice) = value.as_str() {
        return Value::String(choice.to_string());
    }
    let Some(choice) = value.as_object() else {
        return value.clone();
    };
    if let Some(function) = choice.get("function").and_then(Value::as_object) {
        if function.get("name").and_then(Value::as_str).is_some() {
            return value.clone();
        }
    }

    match choice.get("type").and_then(Value::as_str) {
        Some("auto") => json!("auto"),
        Some("none") => json!("none"),
        Some("required") | Some("tool") | Some("any") => json!("required"),
        Some("function") => {
            let name = choice
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| {
                    choice
                        .get("function")
                        .and_then(Value::as_object)
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                })
                .unwrap_or("");
            if name.is_empty() {
                return json!("required");
            }
            json!({
                "type": "function",
                "function": { "name": name }
            })
        }
        _ => value.clone(),
    }
}

pub(super) fn map_chat_tool_choice_to_responses(value: &Value) -> Value {
    let Some(choice) = value.as_object() else {
        return value.clone();
    };
    if choice.get("name").and_then(Value::as_str).is_some() {
        return value.clone();
    }
    if choice.get("type").and_then(Value::as_str) != Some("function") {
        return value.clone();
    }
    let name = choice
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    json!({
        "type": "function",
        "name": name
    })
}

fn responses_web_search_tool_to_chat_options(value: &Value) -> Option<Value> {
    let tool = value.as_object()?;
    let tool_type = tool.get("type").and_then(Value::as_str)?;
    if tool_type != "web_search" && tool_type != "web_search_preview" {
        return None;
    }

    let mut options = Map::new();
    copy_optional_tool_key(tool, &mut options, "search_context_size");
    copy_optional_tool_key(tool, &mut options, "user_location");
    Some(Value::Object(options))
}

fn normalize_tool_parameters(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(object)) => {
            let mut normalized = object.clone();
            if !normalized.contains_key("type") {
                normalized.insert("type".to_string(), json!("object"));
            }
            Some(Value::Object(normalized))
        }
        Some(other) => Some(other.clone()),
        None => Some(json!({ "type": "object" })),
    }
}

fn copy_optional_tool_key(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key) {
        target.insert(key.to_string(), value.clone());
    }
}

/// Chat 没有 allowed_tools 结构：收窄声明后保留 auto/required 选择意图。
pub(super) fn restrict_allowed_tools_for_chat(
    object: &mut Map<String, Value>,
) -> Result<(), String> {
    let Some(choice) = object
        .get("tool_choice")
        .filter(|choice| choice.get("type").and_then(Value::as_str) == Some("allowed_tools"))
    else {
        return Ok(());
    };
    let names: std::collections::HashSet<String> = choice
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            tool.get("name")
                .or_else(|| tool.get("function")?.get("name"))?
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    let mode = choice
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_owned();
    if !matches!(mode.as_str(), "auto" | "required") {
        return Err("Unsupported allowed_tools mode.".to_string());
    }
    let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return Err("allowed_tools requires tool declarations.".to_string());
    };
    tools.retain(|tool| {
        tool.get("name")
            .or_else(|| tool.get("function")?.get("name"))
            .and_then(Value::as_str)
            .is_some_and(|name| names.contains(name))
    });
    if tools.is_empty() {
        return Err("allowed_tools does not match any declared tool.".to_string());
    }
    let count = tools.len();
    object.insert("tool_choice".into(), json!(mode));
    tracing::debug!(count, "restricted Chat tool declarations to allowed_tools");
    Ok(())
}
