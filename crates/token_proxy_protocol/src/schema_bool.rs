//! JSON Schema 布尔子模式归一：严格的 OpenAPI 校验器拒绝 `items: true` 这类合法写法。
//! 只遍历 Schema 节点；`false` 保留拒绝语义，`additionalProperties` 的布尔值是结构化输出约束，原样保留。
use serde_json::{Map, Value};

const SCHEMA_MAPS: &[&str] = &[
    "properties",
    "$defs",
    "definitions",
    "patternProperties",
    "dependentSchemas",
];
const SCHEMA_VALUES: &[&str] = &[
    "items",
    "prefixItems",
    "contains",
    "propertyNames",
    "unevaluatedItems",
    "additionalItems",
    "contentSchema",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
];

/// 返回被替换的 `true` 子模式数量。根节点本身不在此处改写，由调用方决定根形状。
pub fn normalize_true_subschemas(schema: &mut Value) -> usize {
    let mut changed = 0;
    if let Some(object) = schema.as_object_mut() {
        normalize_object(object, &mut changed);
    }
    changed
}

fn normalize_object(object: &mut Map<String, Value>, changed: &mut usize) {
    for key in SCHEMA_MAPS {
        if let Some(children) = object.get_mut(*key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_subschema(child, changed);
            }
        }
    }
    for key in SCHEMA_VALUES {
        match object.get_mut(*key) {
            Some(Value::Array(children)) => {
                for child in children {
                    normalize_subschema(child, changed);
                }
            }
            Some(child) => normalize_subschema(child, changed),
            None => {}
        }
    }
    // additionalProperties 可以是 Schema 对象；布尔值保留，对象继续向下遍历。
    if let Some(Value::Object(child)) = object.get_mut("additionalProperties") {
        normalize_object(child, changed);
    }
}

fn normalize_subschema(schema: &mut Value, changed: &mut usize) {
    match schema {
        Value::Bool(true) => {
            *schema = Value::Object(Map::new());
            *changed += 1;
        }
        Value::Object(object) => normalize_object(object, changed),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replaces_true_subschemas_but_keeps_false_and_additional_properties() {
        let mut schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "patch": { "type": "array", "items": true },
                "anything": true,
                "never": false,
                "choice": { "anyOf": [true, { "type": "string" }] },
                "flag": { "type": "boolean", "enum": [true, false], "default": true },
                "map": { "type": "object", "additionalProperties": { "type": "object", "properties": { "x": true } } }
            },
            "$defs": { "wildcard": true }
        });

        assert_eq!(normalize_true_subschemas(&mut schema), 5);
        assert_eq!(schema["properties"]["patch"]["items"], json!({}));
        assert_eq!(schema["properties"]["anything"], json!({}));
        assert_eq!(schema["properties"]["never"], json!(false));
        assert_eq!(schema["properties"]["choice"]["anyOf"][0], json!({}));
        assert_eq!(schema["properties"]["flag"]["enum"], json!([true, false]));
        assert_eq!(schema["properties"]["flag"]["default"], json!(true));
        assert_eq!(
            schema["properties"]["map"]["additionalProperties"]["properties"]["x"],
            json!({})
        );
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["$defs"]["wildcard"], json!({}));
    }
}
