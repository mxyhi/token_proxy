//! 每个请求构建确定性的名称映射；合法短名优先保留，冲突按身份排序消解。
use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Map, Value};

type Identity = (String, String);

struct Tool {
    identity: Identity,
    value: Value,
    custom: bool,
    mapped: String,
    order: usize,
}

fn sources(object: &Map<String, Value>) -> Vec<&Value> {
    let mut tools = Vec::new();
    if let Some(values) = object.get("tools").and_then(Value::as_array) {
        tools.extend(values);
    }
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                if let Some(values) = item.get("tools").and_then(Value::as_array) {
                    tools.extend(values);
                }
            }
        }
    }
    tools
}

fn name(tool: &Value) -> Option<&str> {
    tool.get("name")
        .or_else(|| tool.get("function")?.get("name"))?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn tools(object: &Map<String, Value>, preserved: &[&str]) -> Vec<Tool> {
    let mut collected = BTreeMap::new();
    for tool in sources(object) {
        if tool.get("type").and_then(Value::as_str) == Some("namespace") {
            let Some(namespace) = name(tool).filter(|name| !preserved.contains(name)) else {
                continue;
            };
            if let Some(children) = tool
                .get("tools")
                .or_else(|| tool.get("children"))
                .and_then(Value::as_array)
            {
                for child in children {
                    collect(&mut collected, namespace, child);
                }
            }
        } else {
            collect(&mut collected, "", tool);
        }
    }
    let mut tools: Vec<Tool> = collected.into_values().collect();
    // 先占用所有合法原名，缩名不得抢占其它工具的原名。
    let mut used: HashSet<String> = tools
        .iter()
        .filter_map(|tool| {
            (tool.identity.0.is_empty() && valid(&tool.identity.1)).then(|| tool.identity.1.clone())
        })
        .collect();
    for tool in &mut tools {
        if tool.identity.0.is_empty() && valid(&tool.identity.1) {
            tool.mapped = tool.identity.1.clone();
            continue;
        }
        let raw = if tool.identity.0.is_empty() {
            tool.identity.1.clone()
        } else {
            format!("{}__{}", tool.identity.0, tool.identity.1)
        };
        let base: String = raw
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                    ch
                } else {
                    '_'
                }
            })
            .take(64)
            .collect();
        let mut candidate = base.clone();
        let mut index = 0;
        while !used.insert(candidate.clone()) {
            index += 1;
            let suffix = format!("_{index}");
            candidate = format!("{}{}", &base[..base.len().min(64 - suffix.len())], suffix);
        }
        tool.mapped = candidate;
    }
    tools.sort_by_key(|tool| tool.order);
    tools
}

fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
}

fn collect(collected: &mut BTreeMap<Identity, Tool>, namespace: &str, value: &Value) {
    let kind = value.get("type").and_then(Value::as_str);
    if !matches!(kind, Some("function" | "custom")) {
        return;
    }
    let Some(name) = name(value) else {
        return;
    };
    let identity = (namespace.to_string(), name.to_string());
    let order = collected.len();
    collected.entry(identity.clone()).or_insert_with(|| Tool {
        identity,
        value: value.clone(),
        custom: kind == Some("custom"),
        mapped: String::new(),
        order,
    });
}

pub(super) fn identities(
    object: &Map<String, Value>,
    preserved: &[&str],
) -> HashMap<String, (String, String, bool)> {
    tools(object, preserved)
        .into_iter()
        .map(|tool| (tool.mapped, (tool.identity.1, tool.identity.0, tool.custom)))
        .collect()
}

pub(super) fn normalize(
    object: &mut Map<String, Value>,
    preserved: &[&str],
) -> Result<usize, String> {
    let tools = tools(object, preserved);
    if tools.is_empty() {
        return Ok(0);
    }
    let mapping: HashMap<Identity, String> = tools
        .iter()
        .map(|tool| (tool.identity.clone(), tool.mapped.clone()))
        .collect();
    let mut normalized: Vec<Value> = sources(object)
        .into_iter()
        .filter(|tool| {
            !matches!(
                tool.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) && (tool.get("type").and_then(Value::as_str) != Some("namespace")
                || name(tool).is_some_and(|name| preserved.contains(&name)))
        })
        .cloned()
        .collect();
    let mut changed = 0;
    for tool in tools {
        let mut value = tool.value;
        let body = if value.get("function").is_some_and(Value::is_object) {
            value.get_mut("function").expect("function object")
        } else {
            &mut value
        };
        body["name"] = json!(tool.mapped);
        if !tool.identity.0.is_empty() && tool.custom {
            value["type"] = json!("function");
            value["parameters"] = json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"]});
        }
        changed += usize::from(!tool.identity.0.is_empty() || tool.identity.1 != tool.mapped);
        normalized.push(value);
    }
    object.insert("tools".to_string(), Value::Array(normalized));
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            if matches!(
                item.get("type").and_then(Value::as_str),
                Some(
                    "function_call"
                        | "custom_tool_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                )
            ) {
                rewrite(item, &mapping);
            }
        }
    }
    if let Some(choice) = object.get_mut("tool_choice") {
        if choice.get("type").and_then(Value::as_str) == Some("namespace")
            && name(choice).is_some_and(|name| !preserved.contains(&name))
        {
            // 保持显式 namespace 选择范围，避免退化成任意工具 auto。
            let namespace = name(choice).unwrap_or_default();
            let mut allowed: Vec<String> = mapping
                .iter()
                .filter(|((ns, _), _)| ns == namespace)
                .map(|(_, mapped)| mapped.clone())
                .collect();
            allowed.sort();
            *choice = json!({"type":"allowed_tools","mode":"required","tools":allowed.into_iter().map(|name| json!({"type":"function","name":name})).collect::<Vec<_>>()});
        } else {
            rewrite(choice, &mapping);
            if let Some(allowed) = choice.get_mut("tools").and_then(Value::as_array_mut) {
                for tool in allowed {
                    rewrite(tool, &mapping);
                }
            }
        }
    }
    if changed > 0 {
        tracing::debug!(changed, "normalized bounded tool identities");
    }
    Ok(changed)
}

fn rewrite(value: &mut Value, mapping: &HashMap<Identity, String>) {
    let Some(local) = name(value) else {
        return;
    };
    let namespace = value
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let Some(mapped) = mapping.get(&(namespace.to_string(), local.to_string())) else {
        return;
    };
    let nested = value.get("function").is_some_and(Value::is_object);
    if nested {
        value["function"]["name"] = json!(mapped);
    } else {
        value["name"] = json!(mapped);
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("namespace");
        if object.get("type").and_then(Value::as_str) == Some("custom_tool_call")
            && !namespace.is_empty()
        {
            let input = object.remove("input").unwrap_or(json!(""));
            object.insert("type".into(), json!("function_call"));
            object.insert(
                "arguments".into(),
                json!(json!({"input":input}).to_string()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_invalid_and_colliding_names_deterministically() {
        let mut request = json!({
            "tools": [
                {"type":"function","name":"safe_name"},
                {"type":"namespace","name":"namespace with spaces","tools":[
                    {"type":"function","name":"tool/with/slashes"},
                    {"type":"function","name":"tool_with_slashes"}
                ]}
            ],
            "input":[{"type":"function_call","namespace":"namespace with spaces","name":"tool/with/slashes"}]
        });
        let changed = normalize(request.as_object_mut().expect("object"), &[]).unwrap();
        assert_eq!(changed, 2);
        let tools = request["tools"].as_array().expect("tools");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names[0], "safe_name");
        assert!(names[1].len() <= 64);
        assert!(names[1]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')));
        assert_ne!(names[1], names[2]);
        assert_eq!(request["input"][0]["name"], names[1]);
        assert!(request["input"][0].get("namespace").is_none());
    }
}
