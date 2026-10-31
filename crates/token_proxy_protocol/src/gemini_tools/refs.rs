//! 先沿 Schema 节点展开本地引用，禁止网络引用和无界递归；实例数据保持不透明。
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 16_384;
const MAX_LITERAL_BYTES: usize = 8 * 1024 * 1024;

struct Budget {
    nodes: usize,
    bytes: usize,
}

fn charge_literal(value: &Value, budget: &mut Budget, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH || budget.nodes == 0 {
        return Err("Gemini tool schema expansion exceeds its limit.".into());
    }
    budget.nodes -= 1;
    match value {
        Value::String(value) => {
            budget.bytes = budget
                .bytes
                .checked_sub(value.len())
                .ok_or("Gemini tool schema expansion exceeds its byte limit.")?
        }
        Value::Array(values) => {
            for value in values {
                charge_literal(value, budget, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                budget.bytes = budget
                    .bytes
                    .checked_sub(key.len())
                    .ok_or("Gemini tool schema expansion exceeds its byte limit.")?;
                charge_literal(value, budget, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn clone_literal(value: &Value, budget: &mut Budget, depth: usize) -> Result<Value, String> {
    charge_literal(value, budget, depth)?;
    Ok(value.clone())
}

pub(super) fn resolve(schema: &Value) -> Result<Value, String> {
    let mut budget = Budget {
        nodes: MAX_NODES,
        bytes: MAX_LITERAL_BYTES,
    };
    visit(schema, schema, &mut Vec::new(), &mut budget, 0)
}

fn visit(
    value: &Value,
    root: &Value,
    stack: &mut Vec<String>,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value, String> {
    if depth > MAX_DEPTH || budget.nodes == 0 {
        return Err("Gemini tool schema expansion exceeds its limit.".into());
    }
    budget.nodes -= 1;
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|item| visit(item, root, stack, budget, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    let Some(object) = value.as_object() else {
        return clone_literal(value, budget, depth);
    };
    let mut output = if let Some(reference) = object.get("$ref") {
        let reference = reference
            .as_str()
            .ok_or("Gemini tool schema reference must be a string.")?;
        let pointer = reference
            .strip_prefix('#')
            .ok_or("Gemini tool schema only supports local references.")?;
        if (!pointer.is_empty() && !pointer.starts_with('/'))
            || stack.iter().any(|item| item == reference)
        {
            return Err("Gemini tool schema reference is cyclic or unsupported.".into());
        }
        let target = root
            .pointer(pointer)
            .ok_or("Gemini tool schema reference could not be resolved.")?;
        stack.push(reference.to_string());
        let resolved = visit(target, root, stack, budget, depth + 1)?;
        stack.pop();
        match resolved {
            Value::Object(object) => object,
            _ => return Err("Gemini tool schema reference must resolve to an object.".into()),
        }
    } else {
        Map::new()
    };
    let has_closed_boundary = [&output, object].iter().any(|schema| {
        schema
            .get("additionalProperties")
            .is_some_and(|value| value != &Value::Bool(true))
    });
    let has_sibling_assertion = object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "$ref" | "$defs" | "definitions" | "description" | "title" | "default" | "examples"
        )
    });
    if object.contains_key("$ref") && has_closed_boundary && has_sibling_assertion {
        // additionalProperties is evaluated against its own properties set.
        // Hoisting sibling properties across that boundary would permit keys
        // the reference forbids (or vice versa).
        let mut siblings = object.clone();
        siblings.remove("$ref");
        let siblings = visit(&Value::Object(siblings), root, stack, budget, depth + 1)?;
        return Ok(serde_json::json!({"allOf":[output,siblings]}));
    }
    for (key, child) in object {
        budget.bytes = budget
            .bytes
            .checked_sub(key.len())
            .ok_or("Gemini tool schema expansion exceeds its byte limit.")?;
        if matches!(key.as_str(), "$ref" | "$defs" | "definitions") {
            continue;
        }
        let child = match key.as_str() {
            "properties" | "patternProperties" | "dependentSchemas" => {
                if let Some(properties) = child.as_object() {
                    let mut mapped = Map::new();
                    for (name, schema) in properties {
                        budget.bytes = budget
                            .bytes
                            .checked_sub(name.len())
                            .ok_or("Gemini tool schema expansion exceeds its byte limit.")?;
                        mapped.insert(name.clone(), visit(schema, root, stack, budget, depth + 1)?);
                    }
                    Value::Object(mapped)
                } else {
                    clone_literal(child, budget, depth + 1)?
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(items) = child.as_array() {
                    Value::Array(
                        items
                            .iter()
                            .map(|item| visit(item, root, stack, budget, depth + 1))
                            .collect::<Result<_, _>>()?,
                    )
                } else {
                    clone_literal(child, budget, depth + 1)?
                }
            }
            "items"
            | "additionalProperties"
            | "additionalItems"
            | "contains"
            | "not"
            | "if"
            | "then"
            | "else"
            | "propertyNames"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "contentSchema" => visit(child, root, stack, budget, depth + 1)?,
            _ => clone_literal(child, budget, depth + 1)?,
        };
        if let Some(previous) = output.get_mut(key) {
            if previous == &child {
                continue;
            }
            if key == "properties" {
                if let (Some(left), Some(right)) = (previous.as_object_mut(), child.as_object()) {
                    for (name, schema) in right {
                        if let Some(old) = left.get(name).filter(|old| *old != schema) {
                            left.insert(name.clone(), serde_json::json!({"allOf":[old,schema]}));
                            continue;
                        }
                        left.insert(name.clone(), schema.clone());
                    }
                    continue;
                }
            }
            if key == "required" {
                if let (Some(left), Some(right)) = (previous.as_array_mut(), child.as_array()) {
                    for name in right {
                        if !left.contains(name) {
                            left.push(name.clone());
                        }
                    }
                    continue;
                }
            }
            if !matches!(
                key.as_str(),
                "description" | "title" | "default" | "examples"
            ) {
                // A sibling constrains the referenced schema; it never replaces
                // the reference's assertion. Preserve conjunction for the cleaner.
                let constraint = serde_json::json!({key.clone():child});
                output
                    .entry("allOf".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                    .ok_or("Gemini tool schema allOf must be an array.")?
                    .push(constraint);
                continue;
            }
        }
        output.insert(key.clone(), child);
    }
    Ok(Value::Object(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn refs_preserve_constraints_and_literal_data() {
        let literal = json!({"$ref":"https://invalid.example/ref", "readOnly":true});
        let schema = json!({"type":"object","$defs":{"a/b":{"type":"integer","minimum":2}},"properties":{"x":{"$ref":"#/$defs/a~1b","description":"count"}},"default":literal});
        let resolved = resolve(&schema).unwrap();
        assert_eq!(resolved["properties"]["x"]["minimum"], 2);
        assert_eq!(resolved["default"], literal);
        assert!(resolved.get("$defs").is_none());
    }

    #[test]
    fn refs_keep_sibling_conjunction_without_overwriting_constraints() {
        let schema = json!({"$defs":{"base":{"type":"object","properties":{"x":{"type":"integer","minimum":2}},"required":["x"]}},"$ref":"#/$defs/base","properties":{"x":{"maximum":5}},"required":["y"]});
        let resolved = resolve(&schema).unwrap();
        assert_eq!(
            resolved["properties"]["x"]["allOf"],
            json!([{"type":"integer","minimum":2},{"maximum":5}])
        );
        assert_eq!(resolved["required"], json!(["x", "y"]));
        let ranged = resolve(
            &json!({"$defs":{"n":{"type":"number","minimum":2}},"$ref":"#/$defs/n","minimum":5}),
        )
        .unwrap();
        assert_eq!(ranged["minimum"], 2);
        assert_eq!(ranged["allOf"], json!([{"minimum":5}]));
        let closed = resolve(&json!({"$defs":{"closed":{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false}},"$ref":"#/$defs/closed","properties":{"b":{"type":"string"}},"required":["b"]})).unwrap();
        assert!(closed.get("properties").is_none());
        assert!(closed["allOf"][0]["properties"].get("b").is_none());
        assert_eq!(closed["allOf"][1]["required"], json!(["b"]));
    }
    #[test]
    fn refs_bound_depth_and_literal_expansion_and_merge_siblings() {
        let mut deep = json!({"type":"string"});
        for _ in 0..MAX_DEPTH + 1 {
            deep = json!({"type":"object","properties":{"next":deep}});
        }
        assert!(resolve(&deep).is_err());
        let large = json!({"type":"object","$defs":{"text":{"type":"string","description":"x".repeat(MAX_LITERAL_BYTES / 2 + 1)}},"properties":{"a":{"$ref":"#/$defs/text"},"b":{"$ref":"#/$defs/text"}}});
        assert!(resolve(&large).is_err());
        let merged = resolve(&json!({"$defs":{"base":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}},"$ref":"#/$defs/base","properties":{"b":{"type":"integer"}},"required":["b"]})).unwrap();
        assert_eq!(merged["required"], json!(["a", "b"]));
        assert_eq!(merged["properties"]["b"]["type"], "integer");
    }

    #[test]
    fn refs_reject_cycles_external_missing_and_expansion_bombs() {
        for schema in [
            json!({"$ref":"#"}),
            json!({"$ref":"https://example.com"}),
            json!({"$ref":"#/missing"}),
        ] {
            assert!(resolve(&schema).is_err());
        }
        let mut defs = Map::new();
        defs.insert("n0".into(), json!({"type":"string"}));
        for n in 1..16 {
            defs.insert(format!("n{n}"), json!({"type":"object","properties":{"a":{"$ref":format!("#/$defs/n{}",n-1)},"b":{"$ref":format!("#/$defs/n{}",n-1)}}}));
        }
        assert!(resolve(&json!({"$ref":"#/$defs/n15","$defs":defs})).is_err());
    }
}
