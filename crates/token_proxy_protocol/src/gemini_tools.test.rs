use super::*;

#[test]
fn schema_root_rejects_non_object_parameters() {
    for parameters in [
        json!({"type":"string","properties":{"x":{"type":"string"}}}),
        json!({"type":"array","items":{"type":"string"}}),
        json!({"type":["string","null"]}),
        json!({"type":"null"}),
        json!({"allOf":[{"type":"string"}]}),
        json!(false),
    ] {
        let tools =
            json!([{"type":"function","function":{"name":"invalid","parameters":parameters}}]);
        let error = map_chat_tools_to_gemini(&tools).expect_err("invalid tool root");
        assert!(error.contains("invalid"));
        assert!(error.contains("object"));
    }
}

#[test]
fn schema_types_are_inferred_only_for_unambiguous_object_nodes() {
    let instance = json!({"properties":{"data":{"type":null}},"type":null});
    let tools = json!([{"type":"function","function":{"name":"typed","parameters":{
        "allOf":[{"properties":{
            "nested":{"type":null,"properties":{"x":{"type":"string"}}},
            "dict":{"additionalProperties":{"properties":{"x":{"type":"integer"}}}},
            "scalar":{"type":"string","properties":{"x":{"type":"string"}}},
            "union":{"type":["object","null"],"properties":{"x":{"type":"string"}}},
            "nullable":{"anyOf":[{"properties":{"x":{"type":"string"}}},{"type":"null"}]},
            "nullable_object":{"anyOf":[{"type":"object","nullable":true,"properties":{"x":{"type":"string"}}}]},
            "open_union":{"anyOf":[{"properties":{"x":{"type":"string"}}},{}]},
            "intersection":{"allOf":[{"type":"string"},{"properties":{"x":{"type":"string"}}}]},
            "ambiguous":{"properties":{"x":{"type":"string"}},"items":{"type":"number"}},
            "data":{"default":instance,"enum":[instance],"const":instance}
        }}]
    }}}]);
    let mapped = map_chat_tools_to_gemini(&tools).unwrap();
    let schema = &mapped[0]["functionDeclarations"][0]["parametersJsonSchema"];
    assert_eq!(schema["type"], "object");
    let properties = &schema["properties"];
    assert_eq!(properties["nested"]["type"], "object");
    assert_eq!(properties["dict"]["type"], "object");
    assert_eq!(properties["dict"]["additionalProperties"]["type"], "object");
    assert_eq!(properties["scalar"]["type"], "string");
    assert_eq!(properties["union"]["type"], json!(["object", "null"]));
    assert!(properties["nullable"].get("type").is_none());
    assert_eq!(properties["nullable"]["anyOf"][0]["type"], "object");
    assert_eq!(properties["nullable"]["anyOf"][1]["type"], "null");
    assert_eq!(properties["nullable_object"]["anyOf"][0]["nullable"], true);
    assert!(properties["open_union"].get("type").is_none());
    assert_eq!(properties["open_union"]["anyOf"][1], json!({}));
    assert!(properties["intersection"].get("type").is_none());
    assert_eq!(properties["intersection"]["allOf"][0]["type"], "string");
    assert!(properties["ambiguous"].get("type").is_none());
    assert_eq!(properties["ambiguous"]["items"]["type"], "number");
    assert_eq!(properties["data"]["default"], instance);
    assert_eq!(properties["data"]["enum"], json!([instance]));
    assert_eq!(properties["data"]["const"], instance);
}

// 严格模式仅影响 auto/缺省，不得覆盖明确的调用选择。
#[test]
fn strict_tools_preserve_explicit_tool_choice() {
    let tools = json!([{"type":"function", "function":{"name":"lookup", "strict":true}}]);
    for (choice, mode, name) in [
        (None, "VALIDATED", None),
        (Some(Value::Null), "VALIDATED", None),
        (Some(json!("auto")), "VALIDATED", None),
        (Some(json!("none")), "NONE", None),
        (Some(json!("required")), "ANY", None),
        (
            Some(json!({"type":"function","function":{"name":"lookup"}})),
            "ANY",
            Some("lookup"),
        ),
        (
            Some(json!({"type":"function","name":"lookup"})),
            "ANY",
            Some("lookup"),
        ),
    ] {
        let mapped = map_chat_tool_choice_to_gemini(choice.as_ref(), Some(&tools)).unwrap();
        assert_eq!(mapped["functionCallingConfig"]["mode"], mode);
        if let Some(name) = name {
            assert_eq!(
                mapped["functionCallingConfig"]["allowedFunctionNames"],
                json!([name])
            );
        }
    }
    let tools =
        json!([{"type":"function", "strict":true, "function":{"name":"lookup", "strict":false}}]);
    assert!(map_chat_tool_choice_to_gemini(None, Some(&tools)).is_none());
    assert_eq!(
        map_chat_tool_choice_to_gemini(Some(&json!("auto")), Some(&tools)).unwrap()
            ["functionCallingConfig"]["mode"],
        "AUTO"
    );
    assert!(map_chat_tool_choice_to_gemini(
        None,
        Some(&json!([{"type":"web_search", "strict":true}]))
    )
    .is_none());
}

#[test]
fn json_schema_keeps_constraints_and_instance_values() {
    let example = json!({"required":null,"minLength":4,"id":"data","$ref":"literal"});
    let tools = json!([{"type":"function", "name":"lookup", "strict":true, "parameters":{
        "type":"object", "additionalProperties":false,
        "properties":{
            "value":{"type":"string", "pattern":"^ok$", "minLength":2, "maxLength":5},
            "list":{"type":"array", "minItems":1, "maxItems":4, "uniqueItems":true, "items":{"type":"integer", "enum":[1,2]}},
            "dict":{"type":"object", "additionalProperties":{"type":"string", "minLength":1}},
            "nested":{"type":"object", "properties":{"x":{"type":"string"}}, "required":["x"]},
            "free":{"type":"object", "additionalProperties":true, "default":example, "examples":[example]}
        }, "required":["value","nested"]
    }}]);
    let mapped = map_chat_tools_to_gemini(&tools).unwrap();
    let declaration = &mapped[0]["functionDeclarations"][0];
    assert!(declaration.get("parameters").is_none());
    assert!(declaration.get("strict").is_none());
    assert_eq!(declaration["parametersJsonSchema"], tools[0]["parameters"]);
}

#[test]
fn validated_mode_round_trips_strictness_and_allowed_names() {
    let tools = json!([{"functionDeclarations":[
        {"name":"yes", "parametersJsonSchema":{"type":"object","additionalProperties":false}},
        {"name":"no", "parametersJsonSchema":{"type":"object"}}
    ]}]);
    let config =
        json!({"functionCallingConfig":{"mode":"VALIDATED","allowedFunctionNames":["yes"]}});
    let mapped = map_gemini_tools_to_chat(&tools, Some(&config));
    assert_eq!(mapped.as_array().unwrap().len(), 1);
    assert_eq!(mapped[0]["function"]["name"], "yes");
    assert_eq!(mapped[0]["function"]["strict"], true);
    assert_eq!(
        mapped[0]["function"]["parameters"]["additionalProperties"],
        false
    );
    let choice = map_gemini_tool_config_to_chat(&config).unwrap();
    assert_eq!(choice, "auto");
    assert_eq!(
        map_chat_tool_choice_to_gemini(Some(&choice), Some(&mapped)).unwrap()
            ["functionCallingConfig"]["mode"],
        "VALIDATED"
    );
}
