use serde_json::{json, Map, Value};
use std::collections::HashMap;

const RESPONSES_TOOL_CALL_ID_PREFIXES: &[&str] = &["fc_", "ctc_", "tsc_"];

mod names;
mod namespaces;
pub use namespaces::flatten_responses_namespaces;

/// 为无原生命名空间协议统一声明、历史及工具选择的名称。
pub fn normalize_responses_tool_names(
    object: &mut Map<String, Value>,
    preserved_namespaces: &[&str],
) -> Result<usize, String> {
    names::normalize(object, preserved_namespaces)
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
    request
        .as_object()
        .map(|request| names::identities(request, &[]))
        .unwrap_or_default()
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
