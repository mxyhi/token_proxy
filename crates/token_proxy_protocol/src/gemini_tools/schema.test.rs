use super::*;

#[test]
fn clean_schema_preserves_numeric_constraints() {
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

    assert_eq!(cleaned["properties"]["count"]["minimum"], 1);
    assert_eq!(cleaned["properties"]["strict"]["minimum"], 5);
    assert_eq!(cleaned["properties"]["count"]["exclusiveMinimum"], 2);
    assert_eq!(cleaned["properties"]["fractional"]["exclusiveMinimum"], 0.5);
    assert_eq!(cleaned["properties"]["number"]["exclusiveMinimum"], 0);
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

    assert_eq!(cleaned["properties"]["existing"]["type"], "string");
    assert_eq!(cleaned["properties"]["from_then"]["type"], "string");
    assert_eq!(cleaned["properties"]["from_else"]["type"], "integer");
    assert_eq!(
        cleaned["properties"]["nested"]["properties"]["nested_then"]["type"],
        "string"
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

    assert_eq!(cleaned["properties"]["reason"]["type"], "string");
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
    assert_eq!(cleaned["properties"]["data"]["type"], "object");
    assert_eq!(cleaned["properties"]["data"]["required"], json!(["parent"]));
    assert!(cleaned["properties"]["data"]["properties"]["parent"]
        .get("required")
        .is_none());
    assert_eq!(
        cleaned["properties"]["data"]["properties"]["tasks"]["items"]["type"],
        "object"
    );
    assert_eq!(
        cleaned["properties"]["data"]["properties"]["tasks"]["items"]["required"],
        json!(["name"])
    );
}

#[test]
fn clean_schema_preserves_enum_value_types_in_scalar_unions() {
    let schema = json!({
        "anyOf": [
            { "deprecated": true, "enum": ["on", false, 1, null] },
            { "enum": ["ok", { "invalid": true }] }
        ],
        "$defs": { "nested": { "deprecated": true, "enum": [2] } }
    });

    let cleaned = clean_tool_schema(&schema);
    assert_eq!(cleaned["enum"], json!(["on", false, 1, null]));
    assert!(cleaned.get("anyOf").is_none());
    assert!(cleaned.get("$defs").is_none());
}

#[test]
fn clean_schema_merges_union_properties_and_preserves_contains() {
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
    assert_eq!(cleaned["properties"]["from_any"]["type"], "integer");
    assert_eq!(cleaned["properties"]["other"]["type"], "boolean");
    assert_eq!(cleaned["required"], json!(["base"]));
    assert_eq!(cleaned["description"], "query");
    assert_eq!(cleaned["contains"], json!({"type":"string", "minLength":2}));
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

    assert_eq!(cleaned["type"], "object");
    assert_eq!(cleaned["properties"]["city"]["type"], "string");
    assert_eq!(cleaned["properties"]["coordinates"]["type"], "array");
    assert_eq!(
        cleaned["properties"]["coordinates"]["items"],
        json!({ "type": "string" })
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
    assert_eq!(cleaned["items"], json!({ "type": "string" }));
}

#[test]
fn clean_schema_preserves_standalone_contains() {
    let schema = json!({"contains":{"type":"string"}});
    assert_eq!(clean_tool_schema(&schema), schema);
}

#[test]
fn clean_schema_repairs_boolean_required_and_array_schema_edges() {
    let schema = json!({
        "type": "object",
        "required": null,
        "additionalItems": true,
        "unevaluatedItems": {},
        "unevaluatedProperties": {},
        "contentSchema": {},
        "properties": {
            "free": true,
            "disabled": false,
            "tuple": {
                "type": "array",
                "prefixItems": [{"type": "string"}, {"type": "number"}]
            },
            "inferred": {"items": {"type": "string"}},
            "union": {"type": ["string", "array"], "items": {"type": "string"}},
            "wrong": {"type": "string", "items": {"type": "string"}},
            "nested": {"type": "array", "items": true}
        }
    });

    let cleaned = clean_tool_schema(&schema);

    assert!(cleaned.get("required").is_none());
    assert!(cleaned.get("additionalItems").is_none());
    assert!(cleaned.get("unevaluatedItems").is_none());
    assert!(cleaned.get("unevaluatedProperties").is_none());
    assert!(cleaned.get("contentSchema").is_none());
    assert_eq!(cleaned["properties"]["free"], json!({}));
    assert_eq!(cleaned["properties"]["disabled"], json!(false));
    assert_eq!(cleaned["properties"]["tuple"]["type"], "array");
    assert_eq!(cleaned["properties"]["tuple"]["items"]["type"], "string");
    assert_eq!(cleaned["properties"]["inferred"]["type"], "array");
    assert_eq!(
        cleaned["properties"]["union"]["type"],
        json!(["string", "array"])
    );
    assert!(cleaned["properties"]["wrong"].get("items").is_none());
    assert_eq!(cleaned["properties"]["nested"]["items"], json!({}));
}

#[cfg(test)]
mod identifier_tests {
    use super::*;

    #[test]
    fn identifiers_are_removed_only_from_schema_nodes() {
        let data: Value = serde_json::from_str(
            r#"{"id":"user-id","$anchor":"keep","nested":{"id":"keep"},"n":9007199254740993}"#,
        )
        .unwrap();
        let schema = json!({"type":"object","id":"root","$anchor":"root","$vocabulary":{"v":true},"properties":{
            "id":{"type":"string","id":"schema","default":"user"},
            "$anchor":{"type":"object","$dynamicRef":"#x","$dynamicAnchor":"x","default":data,"const":data},
            "items":{"type":"array","items":{"type":"string","id":"item"}}
        }});
        let clean = clean_tool_schema(&schema);
        assert!(clean.get("id").is_none());
        assert!(clean.get("$anchor").is_none());
        assert!(clean.get("$vocabulary").is_none());
        assert!(clean["properties"]["id"].get("id").is_none());
        assert_eq!(clean["properties"]["id"]["type"], "string");
        assert!(clean["properties"]["$anchor"].get("$dynamicRef").is_none());
        assert_eq!(clean["properties"]["$anchor"]["default"], data);
        assert_eq!(clean["properties"]["$anchor"]["const"], data);
        assert!(clean["properties"]["items"]["items"].get("id").is_none());
    }
}
