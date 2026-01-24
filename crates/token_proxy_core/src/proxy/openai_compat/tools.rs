use serde_json::{json, Map, Value};

pub(super) fn map_responses_tools_to_chat(value: &Value) -> Value {
    let Some(tools) = value.as_array() else {
        return value.clone();
    };
    let mapped = tools.iter().map(map_responses_tool_to_chat).collect::<Vec<_>>();
    Value::Array(mapped)
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
    if let Some(parameters) = tool.get("parameters") {
        function.insert("parameters".to_string(), parameters.clone());
    }

    json!({
        "type": "function",
        "function": Value::Object(function)
    })
}

pub(super) fn map_chat_tools_to_responses(value: &Value) -> Value {
    let Some(tools) = value.as_array() else {
        return value.clone();
    };
    let mapped = tools.iter().map(map_chat_tool_to_responses).collect::<Vec<_>>();
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
    if let Some(parameters) = function.get("parameters") {
        output.insert("parameters".to_string(), parameters.clone());
    }
    Value::Object(output)
}

pub(super) fn map_responses_tool_choice_to_chat(value: &Value) -> Value {
    let Some(choice) = value.as_object() else {
        return value.clone();
    };
    if choice.get("function").and_then(Value::as_object).is_some() {
        return value.clone();
    }
    if choice.get("type").and_then(Value::as_str) != Some("function") {
        return value.clone();
    }
    let name = choice.get("name").and_then(Value::as_str).unwrap_or("");
    json!({
        "type": "function",
        "function": { "name": name }
    })
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

