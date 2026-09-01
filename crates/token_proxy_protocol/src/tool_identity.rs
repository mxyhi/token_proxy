use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

const RESPONSES_TOOL_CALL_ID_PREFIXES: &[&str] = &["fc_", "ctc_", "tsc_"];

/// Flattens Responses namespaces for protocols without native namespace support.
/// Declarations and matching history/choice references are changed together.
pub fn flatten_responses_namespaces(
    object: &mut Map<String, Value>,
    preserved_namespaces: &[&str],
) -> Result<usize, String> {
    let Some(tools) = object.get("tools").and_then(Value::as_array) else {
        return Ok(0);
    };
    let top_level_names = collect_top_level_names(tools);
    let namespace_names = collect_namespace_names(tools, &top_level_names, preserved_namespaces)?;
    if namespace_names.is_empty() {
        return Ok(0);
    }

    object.insert(
        "tools".to_string(),
        Value::Array(flatten_tools(tools, &namespace_names, preserved_namespaces)),
    );
    if let Some(input) = object.get_mut("input") {
        rewrite_function_calls(input, &namespace_names);
    }
    if let Some(tool_choice) = object.get_mut("tool_choice") {
        if tool_choice.get("type").and_then(Value::as_str) == Some("namespace")
            && !tool_choice
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| preserved_namespaces.contains(&name))
        {
            *tool_choice = Value::String("auto".to_string());
        } else {
            rewrite_function_call(tool_choice, &namespace_names);
        }
    }
    tracing::debug!(
        namespace_tool_count = namespace_names.len(),
        "flattened Responses namespace tools"
    );
    Ok(namespace_names.len())
}

fn collect_top_level_names(tools: &[Value]) -> HashSet<String> {
    tools
        .iter()
        .filter(|tool| {
            matches!(
                tool.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            )
        })
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn collect_namespace_names(
    tools: &[Value],
    top_level_names: &HashSet<String>,
    preserved_namespaces: &[&str],
) -> Result<HashMap<String, (String, String)>, String> {
    let mut names = HashMap::new();
    for tool in tools {
        let Some(namespace) = namespace_name(tool, preserved_namespaces) else {
            continue;
        };
        for child in namespace_children(tool) {
            if !matches!(
                child.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) {
                continue;
            }
            let Some(name) = child_name(child) else {
                continue;
            };
            let flat_name = flatten_tool_name(namespace, name);
            if top_level_names.contains(&flat_name) {
                return Err(format!(
                    "Namespace tool {namespace}/{name} conflicts with top-level tool {flat_name}."
                ));
            }
            let identity = (namespace.to_string(), name.to_string());
            if names
                .get(&flat_name)
                .is_some_and(|existing| existing != &identity)
            {
                return Err(format!(
                    "Namespace tool {namespace}/{name} conflicts with another tool flattened as {flat_name}."
                ));
            }
            names.insert(flat_name, identity);
        }
    }
    Ok(names)
}

fn flatten_tools(
    tools: &[Value],
    names: &HashMap<String, (String, String)>,
    preserved_namespaces: &[&str],
) -> Vec<Value> {
    let mut flattened = Vec::with_capacity(tools.len() + names.len());
    let mut seen = HashSet::new();
    for tool in tools {
        if tool.get("type").and_then(Value::as_str) != Some("namespace")
            || tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| preserved_namespaces.contains(&name))
        {
            flattened.push(tool.clone());
            continue;
        }
        let Some(namespace) = tool.get("name").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        for child in namespace_children(tool) {
            if !matches!(
                child.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) {
                continue;
            }
            let Some(name) = child_name(child) else {
                continue;
            };
            let flat_name = flatten_tool_name(namespace, name);
            if !names.contains_key(&flat_name) || !seen.insert(flat_name.clone()) {
                continue;
            }
            let mut child = child.as_object().cloned().unwrap_or_default();
            child.insert("name".to_string(), Value::String(flat_name));
            if child.get("type").and_then(Value::as_str) == Some("custom") {
                child.insert("type".to_string(), Value::String("function".to_string()));
                child.insert(
                    "parameters".to_string(),
                    serde_json::json!({
                        "type": "object",
                        "properties": { "input": { "type": "string" } },
                        "required": ["input"]
                    }),
                );
            }
            flattened.push(Value::Object(child));
        }
    }
    flattened
}

fn namespace_name<'a>(tool: &'a Value, preserved_namespaces: &[&str]) -> Option<&'a str> {
    if tool.get("type").and_then(Value::as_str) != Some("namespace") {
        return None;
    }
    tool.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && !preserved_namespaces.contains(name))
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

/// Restores namespace and custom-tool identity on Responses output produced by
/// a provider that only understands flattened function calls.
pub fn restore_responses_tool_identities(output: &mut Value, request: &Value) -> usize {
    let mut custom_item_ids = HashMap::new();
    restore_responses_tool_identities_with_state(output, request, &mut custom_item_ids)
}

/// Restores tool identities while retaining custom item ids across SSE events.
pub fn restore_responses_tool_identities_with_state(
    output: &mut Value,
    request: &Value,
    custom_item_ids: &mut HashMap<String, String>,
) -> usize {
    let identities = collect_response_tool_identities(request);
    if identities.is_empty() {
        return 0;
    }
    let mut restored = 0;
    collect_custom_item_ids(output, &identities, custom_item_ids);
    restore_tool_identities(output, &identities, custom_item_ids, &mut restored);
    if restored > 0 {
        tracing::debug!(
            restored,
            "restored Responses namespace and custom tool identities"
        );
    }
    restored
}

fn collect_response_tool_identities(request: &Value) -> HashMap<String, (String, String, bool)> {
    let mut identities = HashMap::new();
    let Some(request) = request.as_object() else {
        return identities;
    };
    let mut sources = Vec::new();
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        sources.extend(tools.iter());
    }
    if let Some(input) = request.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                if let Some(tools) = item.get("tools").and_then(Value::as_array) {
                    sources.extend(tools.iter());
                }
            }
        }
    }
    for tool in sources {
        let tool_type = tool.get("type").and_then(Value::as_str);
        match tool_type {
            Some("function") | Some("custom") | None => {
                let Some(name) = tool.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let name = name.trim();
                if !name.is_empty() {
                    identities.insert(
                        name.to_string(),
                        (name.to_string(), String::new(), tool_type == Some("custom")),
                    );
                }
            }
            Some("namespace") => {
                let Some(namespace) = tool.get("name").and_then(Value::as_str).map(str::trim)
                else {
                    continue;
                };
                if namespace.is_empty() {
                    continue;
                }
                for child in namespace_children(tool) {
                    let child_type = child.get("type").and_then(Value::as_str);
                    if !matches!(child_type, Some("function" | "custom")) {
                        continue;
                    }
                    let Some(name) = child_name(child) else {
                        continue;
                    };
                    identities.insert(
                        flatten_tool_name(namespace, name),
                        (
                            name.to_string(),
                            namespace.to_string(),
                            child_type == Some("custom"),
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    identities
}

fn restore_tool_identities(
    value: &mut Value,
    identities: &HashMap<String, (String, String, bool)>,
    custom_item_ids: &mut HashMap<String, String>,
    restored: &mut usize,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                restore_tool_identities(item, identities, custom_item_ids, restored);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.delta")
                && object
                    .get("item_id")
                    .and_then(Value::as_str)
                    .is_some_and(|item_id| custom_item_ids.contains_key(item_id))
            {
                if let Some(item_id) = object.get("item_id").and_then(Value::as_str) {
                    if let Some(client_item_id) = custom_item_ids.get(item_id).cloned() {
                        object.insert("item_id".to_string(), Value::String(client_item_id));
                    }
                }
                object.insert(
                    "type".to_string(),
                    Value::String("response.custom_tool_call_input.delta".to_string()),
                );
                *restored += 1;
            }
            restore_function_call_event(object, identities, custom_item_ids, restored);
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            ) {
                let name = object.get("name").and_then(Value::as_str).unwrap_or("");
                if let Some((local_name, namespace, custom)) = identities.get(name) {
                    object.insert("name".to_string(), Value::String(local_name.clone()));
                    if namespace.is_empty() {
                        object.remove("namespace");
                    } else {
                        object.insert("namespace".to_string(), Value::String(namespace.clone()));
                    }
                    if *custom {
                        if let Some(upstream_item_id) = object.get("id").and_then(Value::as_str) {
                            let client_item_id = retyped_responses_tool_call_item_id(
                                upstream_item_id,
                                "custom_tool_call",
                            );
                            custom_item_ids
                                .insert(upstream_item_id.to_string(), client_item_id.clone());
                            object.insert("id".to_string(), Value::String(client_item_id));
                        }
                        if object.get("type").and_then(Value::as_str) == Some("function_call") {
                            object.insert("type".to_string(), json!("custom_tool_call"));
                            let input = object
                                .get("arguments")
                                .and_then(Value::as_str)
                                .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                                .and_then(|arguments| arguments.get("input").cloned())
                                .unwrap_or_else(|| {
                                    object
                                        .get("arguments")
                                        .cloned()
                                        .unwrap_or(Value::String(String::new()))
                                });
                            object.remove("arguments");
                            object.insert("input".to_string(), input);
                        }
                    }
                    *restored += 1;
                }
            }
            for child in object.values_mut() {
                restore_tool_identities(child, identities, custom_item_ids, restored);
            }
        }
        _ => {}
    }
}

fn restore_function_call_event(
    object: &mut Map<String, Value>,
    identities: &HashMap<String, (String, String, bool)>,
    custom_item_ids: &mut HashMap<String, String>,
    restored: &mut usize,
) {
    if object.get("type").and_then(Value::as_str) != Some("response.function_call_arguments.done") {
        return;
    }
    let name = object.get("name").and_then(Value::as_str).unwrap_or("");
    let Some((local_name, namespace, custom)) = identities.get(name) else {
        return;
    };
    object.insert("name".to_string(), Value::String(local_name.clone()));
    if namespace.is_empty() {
        object.remove("namespace");
    } else {
        object.insert("namespace".to_string(), Value::String(namespace.clone()));
    }
    if *custom {
        if let Some(upstream_item_id) = object.get("item_id").and_then(Value::as_str) {
            let client_item_id = custom_item_ids
                .entry(upstream_item_id.to_string())
                .or_insert_with(|| {
                    retyped_responses_tool_call_item_id(upstream_item_id, "custom_tool_call")
                })
                .clone();
            object.insert("item_id".to_string(), Value::String(client_item_id));
        }
        object.insert(
            "type".to_string(),
            Value::String("response.custom_tool_call_input.done".to_string()),
        );
        let input = object
            .get("arguments")
            .and_then(Value::as_str)
            .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
            .and_then(|arguments| arguments.get("input").cloned())
            .unwrap_or_else(|| {
                object
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::String(String::new()))
            });
        object.remove("arguments");
        object.insert("input".to_string(), input);
    }
    *restored += 1;
}

fn collect_custom_item_ids(
    value: &Value,
    identities: &HashMap<String, (String, String, bool)>,
    custom_item_ids: &mut HashMap<String, String>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_custom_item_ids(item, identities, custom_item_ids);
            }
        }
        Value::Object(object) => {
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            ) {
                let name = object.get("name").and_then(Value::as_str).unwrap_or("");
                if identities.get(name).is_some_and(|(_, _, custom)| *custom) {
                    if let Some(upstream_item_id) = object.get("id").and_then(Value::as_str) {
                        custom_item_ids.insert(
                            upstream_item_id.to_string(),
                            retyped_responses_tool_call_item_id(
                                upstream_item_id,
                                "custom_tool_call",
                            ),
                        );
                    }
                }
            }
            for child in object.values() {
                collect_custom_item_ids(child, identities, custom_item_ids);
            }
        }
        _ => {}
    }
}

/// Re-prefix tool-call item IDs when a function-only upstream is raised back
/// to a typed Responses item. The suffix remains stable for replay matching.
pub fn retyped_responses_tool_call_item_id(id: &str, item_type: &str) -> String {
    let desired = match item_type {
        "custom_tool_call" => "ctc_",
        "tool_search_call" => "tsc_",
        "function_call" => "fc_",
        _ => return id.to_string(),
    };
    if id.is_empty() || id.starts_with(desired) {
        return id.to_string();
    }
    RESPONSES_TOOL_CALL_ID_PREFIXES
        .iter()
        .find_map(|prefix| id.strip_prefix(prefix))
        .map(|suffix| format!("{desired}{suffix}"))
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_namespace_custom_call_and_unwraps_input() {
        let request = json!({
            "tools": [{
                "type": "namespace",
                "name": "shell",
                "tools": [{ "type": "custom", "name": "exec" }]
            }]
        });
        let mut output = json!({
            "output": [{
                "type": "function_call",
                "name": "shell__exec",
                "arguments": "{\"input\":\"pwd\"}"
            }]
        });

        assert_eq!(restore_responses_tool_identities(&mut output, &request), 1);
        assert_eq!(output["output"][0]["type"], "custom_tool_call");
        assert_eq!(output["output"][0]["name"], "exec");
        assert_eq!(output["output"][0]["namespace"], "shell");
        assert_eq!(output["output"][0]["input"], "pwd");
        assert!(output["output"][0].get("arguments").is_none());
    }

    #[test]
    fn restores_function_call_without_changing_unknown_names() {
        let request = json!({
            "tools": [{
                "type": "namespace",
                "name": "mcp",
                "tools": [{ "type": "function", "name": "lookup" }]
            }]
        });
        let mut output = json!({
            "output": [
                { "type": "function_call", "name": "mcp__lookup", "arguments": "{}" },
                { "type": "function_call", "name": "unknown", "arguments": "{}" }
            ]
        });

        assert_eq!(restore_responses_tool_identities(&mut output, &request), 1);
        assert_eq!(output["output"][0]["name"], "lookup");
        assert_eq!(output["output"][0]["namespace"], "mcp");
        assert_eq!(output["output"][1]["name"], "unknown");
    }

    #[test]
    fn preserves_existing_custom_tool_input() {
        let request = json!({
            "tools": [{
                "type": "custom",
                "name": "exec"
            }]
        });
        let mut output = json!({
            "output": [{
                "type": "custom_tool_call",
                "name": "exec",
                "input": "pwd"
            }]
        });

        assert_eq!(restore_responses_tool_identities(&mut output, &request), 1);
        assert_eq!(output["output"][0]["input"], "pwd");
        assert_eq!(output["output"][0]["type"], "custom_tool_call");
    }

    #[test]
    fn restores_custom_stream_delta_using_item_state() {
        let request = json!({
            "tools": [{
                "type": "custom",
                "name": "exec"
            }]
        });
        let mut item_ids = HashMap::new();
        let mut added = json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "id": "ctc_1",
                "name": "exec",
                "arguments": ""
            }
        });
        assert_eq!(
            restore_responses_tool_identities_with_state(&mut added, &request, &mut item_ids),
            1
        );
        assert_eq!(added["item"]["type"], "custom_tool_call");
        assert_eq!(item_ids.get("ctc_1"), Some(&"ctc_1".to_string()));

        let mut delta = json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "ctc_1",
            "delta": "pwd"
        });
        assert_eq!(
            restore_responses_tool_identities_with_state(&mut delta, &request, &mut item_ids),
            1
        );
        assert_eq!(delta["type"], "response.custom_tool_call_input.delta");
    }

    #[test]
    fn retypes_only_known_responses_tool_call_prefixes() {
        assert_eq!(
            retyped_responses_tool_call_item_id("fc_123", "custom_tool_call"),
            "ctc_123"
        );
        assert_eq!(
            retyped_responses_tool_call_item_id("ctc_123", "tool_search_call"),
            "tsc_123"
        );
        assert_eq!(
            retyped_responses_tool_call_item_id("tsc_123", "function_call"),
            "fc_123"
        );
        assert_eq!(
            retyped_responses_tool_call_item_id("ctc_123", "custom_tool_call"),
            "ctc_123"
        );
        assert_eq!(
            retyped_responses_tool_call_item_id("", "custom_tool_call"),
            ""
        );
        assert_eq!(
            retyped_responses_tool_call_item_id("item_unknown", "custom_tool_call"),
            "item_unknown"
        );
    }

    #[test]
    fn restores_typed_custom_ids_across_responses_event_lifecycle() {
        let request = json!({
            "tools": [{ "type": "custom", "name": "exec" }]
        });
        let mut item_ids = HashMap::new();

        let mut added = json!({
            "type": "response.output_item.added",
            "item": {
                "id": "fc_123",
                "type": "function_call",
                "call_id": "call_upstream",
                "name": "exec",
                "arguments": "{\"input\":\"pwd\"}"
            }
        });
        restore_responses_tool_identities_with_state(&mut added, &request, &mut item_ids);
        assert_eq!(added["item"]["id"], "ctc_123");
        assert_eq!(added["item"]["call_id"], "call_upstream");

        let mut delta = json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "fc_123",
            "delta": "pwd"
        });
        restore_responses_tool_identities_with_state(&mut delta, &request, &mut item_ids);
        assert_eq!(delta["type"], "response.custom_tool_call_input.delta");
        assert_eq!(delta["item_id"], "ctc_123");

        let mut input_done = json!({
            "type": "response.function_call_arguments.done",
            "item_id": "fc_123",
            "call_id": "call_upstream",
            "name": "exec",
            "arguments": "{\"input\":\"pwd\"}"
        });
        restore_responses_tool_identities_with_state(&mut input_done, &request, &mut item_ids);
        assert_eq!(input_done["type"], "response.custom_tool_call_input.done");
        assert_eq!(input_done["item_id"], "ctc_123");
        assert_eq!(input_done["call_id"], "call_upstream");

        let mut item_done = json!({
            "type": "response.output_item.done",
            "item": {
                "id": "fc_123",
                "type": "function_call",
                "call_id": "call_upstream",
                "name": "exec",
                "arguments": "{\"input\":\"pwd\"}"
            }
        });
        restore_responses_tool_identities_with_state(&mut item_done, &request, &mut item_ids);
        assert_eq!(item_done["item"]["id"], "ctc_123");
        assert_eq!(item_done["item"]["call_id"], "call_upstream");

        let mut completed = json!({
            "type": "response.completed",
            "response": { "output": [{
                "id": "fc_123",
                "type": "function_call",
                "call_id": "call_upstream",
                "name": "exec",
                "arguments": "{\"input\":\"pwd\"}"
            }] }
        });
        restore_responses_tool_identities_with_state(&mut completed, &request, &mut item_ids);
        assert_eq!(completed["response"]["output"][0]["id"], "ctc_123");
        assert_eq!(
            completed["response"]["output"][0]["call_id"],
            "call_upstream"
        );
    }
}

fn rewrite_function_calls(value: &mut Value, names: &HashMap<String, (String, String)>) {
    match value {
        Value::Array(items) => {
            for item in items {
                rewrite_function_calls(item, names);
            }
        }
        Value::Object(object) => {
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            ) {
                rewrite_function_call_object(object, names);
            }
            for child in object.values_mut() {
                rewrite_function_calls(child, names);
            }
        }
        _ => {}
    }
}

fn rewrite_function_call(value: &mut Value, names: &HashMap<String, (String, String)>) {
    if let Some(object) = value.as_object_mut() {
        rewrite_function_call_object(object, names);
    }
}

fn rewrite_function_call_object(
    object: &mut Map<String, Value>,
    names: &HashMap<String, (String, String)>,
) {
    let Some(namespace) = object
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::trim)
    else {
        return;
    };
    let Some(name) = object.get("name").and_then(Value::as_str).map(str::trim) else {
        return;
    };
    let flat_name = flatten_tool_name(namespace, name);
    if names.get(&flat_name) != Some(&(namespace.to_string(), name.to_string())) {
        return;
    }
    object.insert("name".to_string(), Value::String(flat_name));
    object.remove("namespace");
}
