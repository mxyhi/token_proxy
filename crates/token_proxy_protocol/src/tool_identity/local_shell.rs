//! 将客户端执行的 shell 桥接为函数；只在参数完整且验证成功后还原可执行 action。
use std::collections::HashSet;

use serde_json::{json, Map, Value};

const BASE_NAME: &str = "__token_proxy_local_shell";

/// 名称基于同一原始请求确定；声明与历史都参与避碰，但历史不重新授权工具。
pub fn bridge_name(request: &Value) -> Option<String> {
    let object = request.as_object()?;
    let has_shell = super::names::sources(object)
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some("shell"))
        || object
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("shell_call" | "shell_call_output")
                    )
                })
            });
    if !has_shell {
        return None;
    }
    let mut used: HashSet<String> = super::names::identities(object, &[]).into_keys().collect();
    if let Some(items) = object.get("input").and_then(Value::as_array) {
        used.extend(
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_owned)),
        );
    }
    let mut name = BASE_NAME.to_string();
    let mut suffix = 0;
    while used.contains(&name) {
        suffix += 1;
        name = format!("{BASE_NAME}_{suffix}");
    }
    Some(name)
}

pub fn prepare_request(object: &mut Map<String, Value>) -> Result<(), String> {
    let Some(name) = bridge_name(&Value::Object(object.clone())) else {
        if object.get("tool_choice").is_some_and(is_shell_choice) {
            return Err("shell tool_choice requires a declared local shell tool.".into());
        }
        return Ok(());
    };
    let declared = super::names::sources(object)
        .iter()
        .any(|tool| tool["type"] == "shell");
    if let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) {
        lower_tools(tools, &name)?;
    }
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("additional_tools") => {
                    if let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut) {
                        lower_tools(tools, &name)?;
                    }
                }
                Some("shell_call") => {
                    if let Some(environment) = item.get("environment") {
                        validate_local_environment(environment)?;
                    }
                    let action = item
                        .get("action")
                        .ok_or("shell_call must include action.")?;
                    validate_action(action)?;
                    let arguments = action.to_string();
                    let call_id = call_id(item)?.to_string();
                    let mut call = json!({"type":"function_call", "call_id":call_id, "name":name, "arguments":arguments});
                    for key in ["id", "status"] {
                        if let Some(value) = item.get(key) {
                            call[key] = value.clone();
                        }
                    }
                    *item = call;
                }
                Some("shell_call_output") => {
                    let call_id = call_id(item)?.to_string();
                    // 保留完整结果信封：stdout/stderr、exit/timeout 与截断上限均属于客户端执行结果。
                    let output = item.to_string();
                    *item =
                        json!({"type":"function_call_output", "call_id":call_id, "output":output});
                }
                _ => {}
            }
        }
    }
    if let Some(choice) = object.get_mut("tool_choice") {
        lower_choice(choice, &name, declared)?;
    }
    tracing::debug!(tool_name = %name, declared, "bridged local shell to Chat function");
    Ok(())
}

fn call_id(item: &Value) -> Result<&str, String> {
    item.get("call_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "shell item must include a non-empty call_id.".into())
}

fn validate_local_environment(environment: &Value) -> Result<(), String> {
    if environment.get("type").and_then(Value::as_str) == Some("local") {
        Ok(())
    } else {
        Err("Chat shell bridge only supports environment.type=local.".into())
    }
}

fn lower_tools(tools: &mut [Value], name: &str) -> Result<(), String> {
    for tool in tools {
        if tool.get("type").and_then(Value::as_str) != Some("shell") {
            continue;
        }
        validate_local_environment(&tool["environment"])?;
        *tool = json!({"type":"function", "name":name,
            "description":"Execute commands in the client's local shell environment. The client executes the commands and returns stdout, stderr and outcomes.",
            "parameters":{"type":"object", "properties":{
                "commands":{"type":"array", "items":{"type":"string", "minLength":1},"minItems":1},
                "timeout_ms":{"type":"integer","minimum":1},
                "max_output_length":{"type":"integer","minimum":1}
            },"required":["commands"],"additionalProperties":false}, "strict":false});
    }
    Ok(())
}

fn is_shell_choice(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("shell")
        || value
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().any(is_shell_choice))
}

fn lower_choice(choice: &mut Value, name: &str, declared: bool) -> Result<(), String> {
    if choice.get("type").and_then(Value::as_str) == Some("shell") {
        if !declared {
            return Err("shell tool_choice requires a declared local shell tool.".into());
        }
        *choice = json!({"type":"function","name":name});
    } else if let Some(tools) = choice.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            lower_choice(tool, name, declared)?;
        }
    }
    Ok(())
}

pub fn parse_action(arguments: &str) -> Result<Value, String> {
    let action: Value = serde_json::from_str(arguments).map_err(|_| {
        "Invalid shell action: arguments must be a complete JSON object.".to_string()
    })?;
    validate_action(&action)?;
    Ok(action)
}

pub fn validate_action(action: &Value) -> Result<(), String> {
    let object = action
        .as_object()
        .ok_or("Invalid shell action: expected an object.")?;
    let commands = object
        .get("commands")
        .and_then(Value::as_array)
        .filter(|commands| !commands.is_empty())
        .ok_or("Invalid shell action: commands must be a non-empty array.")?;
    if commands
        .iter()
        .any(|command| !command.as_str().is_some_and(|text| !text.trim().is_empty()))
    {
        return Err("Invalid shell action: each command must be a non-empty string.".into());
    }
    for key in ["timeout_ms", "max_output_length"] {
        if object
            .get(key)
            .is_some_and(|value| !value.as_u64().is_some_and(|number| number > 0))
        {
            return Err(format!(
                "Invalid shell action: {key} must be a positive integer."
            ));
        }
    }
    if let Some(key) = object.keys().find(|key| {
        !matches!(
            key.as_str(),
            "commands" | "timeout_ms" | "max_output_length"
        )
    }) {
        return Err(format!("Invalid shell action: unsupported field {key}."));
    }
    Ok(())
}

pub fn restore_output(output: &mut Value, request: &Value) -> Result<(), String> {
    let Some(name) = bridge_name(request) else {
        return Ok(());
    };
    if let Some(items) = output.get_mut("output").and_then(Value::as_array_mut) {
        for item in items {
            restore_item(item, &name)?;
        }
    }
    Ok(())
}

pub fn restore_item(item: &mut Value, name: &str) -> Result<bool, String> {
    if item.get("type").and_then(Value::as_str) != Some("function_call")
        || item.get("name").and_then(Value::as_str) != Some(name)
    {
        return Ok(false);
    }
    let action = parse_action(item.get("arguments").and_then(Value::as_str).unwrap_or(""))?;
    let object = item.as_object_mut().expect("function item");
    object.insert("type".into(), json!("shell_call"));
    object.insert("action".into(), action);
    object.remove("arguments");
    object.remove("name");
    object.remove("namespace");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_shell_names_and_history_are_stable_and_do_not_redeclare() {
        let mut request = json!({"tools":[
            {"type":"shell","environment":{"type":"local"}},
            {"type":"function","name":BASE_NAME},
            {"type":"namespace","name":"__token_proxy","tools":[{"type":"function","name":"local_shell"}]}
        ], "input":[{"type":"function_call","name":"__token_proxy_local_shell_1"}],
        "tool_choice":{"type":"allowed_tools","mode":"required","tools":[{"type":"shell"}]}});
        let name = bridge_name(&request).unwrap();
        assert_eq!(name, "__token_proxy_local_shell_2");
        prepare_request(request.as_object_mut().unwrap()).unwrap();
        super::super::normalize_responses_tool_names(request.as_object_mut().unwrap(), &[])
            .unwrap();
        assert_eq!(request["tool_choice"]["tools"][0]["name"], name);
        let names: Vec<_> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| &tool["name"])
            .collect();
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), names.len());
        let mut history = json!({"input":[{"type":"shell_call","call_id":"s","action":{"commands":["pwd"]}},
            {"type":"shell_call_output","call_id":"s","output":[],"max_output_length":10}]});
        prepare_request(history.as_object_mut().unwrap()).unwrap();
        assert!(history.get("tools").is_none());
        assert_eq!(history["input"][0]["type"], "function_call");
        let result: Value =
            serde_json::from_str(history["input"][1]["output"].as_str().unwrap()).unwrap();
        assert_eq!(result["max_output_length"], 10);
    }

    #[test]
    fn local_shell_action_validation_is_atomic() {
        for invalid in [
            json!({}),
            json!({"commands":[]}),
            json!({"commands":[null]}),
            json!({"commands":[" "]}),
            json!({"commands":["ls"],"timeout_ms":0}),
            json!({"commands":["ls"],"max_output_length":null}),
            json!({"commands":["ls"],"timeout_ms":-1}),
            json!({"commands":["ls"],"timeout_ms":0.5}),
            json!({"commands":["ls"],"extra":"ignored?"}),
        ] {
            let mut item =
                json!({"type":"function_call","name":BASE_NAME,"arguments":invalid.to_string()});
            let original = item.clone();
            assert!(restore_item(&mut item, BASE_NAME).is_err());
            assert_eq!(item, original);
        }
        let mut item = json!({"type":"function_call","id":"f","call_id":"s","name":BASE_NAME,
            "arguments":"{\"commands\":[\"pwd\"],\"timeout_ms\":1}"});
        assert!(restore_item(&mut item, BASE_NAME).unwrap());
        assert_eq!(item["call_id"], "s");
        assert_eq!(item["action"]["commands"], json!(["pwd"]));
    }
}
