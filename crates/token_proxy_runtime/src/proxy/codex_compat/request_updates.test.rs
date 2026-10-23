use super::*;
#[test]
fn chat_parallel_choice_is_preserved() {
    for (choice, expected) in [
        (Some(json!(false)), false),
        (Some(json!(true)), true),
        (None, true),
        (Some(Value::Null), true),
    ] {
        let mut request = json!({"model":"gpt-6","messages":[{"role":"user","content":"hi"}]});
        if let Some(choice) = choice {
            request["parallel_tool_calls"] = choice;
        }
        let body = Bytes::from(serde_json::to_vec(&request).unwrap());
        let result = chat_request_to_codex(&body, None).unwrap();
        let result: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(result["parallel_tool_calls"], expected);
    }
}
#[test]
fn search_history_declaration_preserves_disabled_tool_semantics() {
    let mut value = json!({"input":[{"type":"web_search_call","id":"ws_1"}],"tools":[]});
    assert!(ensure_web_search_history(
        value.as_object_mut().unwrap(),
        false
    ));
    assert_eq!(
        value["tools"][0],
        json!({"type":"web_search","external_web_access":false})
    );
    assert_eq!(value["tool_choice"], "none");
    assert!(!ensure_web_search_history(
        value.as_object_mut().unwrap(),
        false
    ));
}
#[test]
fn lite_search_declaration_uses_additional_tools_before_compaction_trigger() {
    let mut value =
        json!({"input":[{"type":"web_search_call"},{"type":"compaction_trigger"}],"tools":[]});
    assert!(ensure_web_search_history(
        value.as_object_mut().unwrap(),
        true
    ));
    assert_eq!(value["input"][1]["type"], "additional_tools");
    assert_eq!(value["input"][2]["type"], "compaction_trigger");
    assert!(value["tools"].as_array().unwrap().is_empty());
}
#[test]
fn search_history_does_not_expand_existing_tools_allowlist() {
    let mut value = json!({"input":[{"type":"web_search_call"}],"tools":[{"type":"function","name":"f"}],"tool_choice":{"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"f"}]}});
    let choice = value["tool_choice"].clone();
    assert!(ensure_web_search_history(
        value.as_object_mut().unwrap(),
        false
    ));
    assert_eq!(value["tool_choice"], choice);
    let mut no_history = json!({"input":[{"type":"message"}]});
    assert!(!ensure_web_search_history(
        no_history.as_object_mut().unwrap(),
        false
    ));
}

#[test]
fn search_history_auto_keeps_only_original_callable_tools() {
    let mut value = json!({"input":[{"type":"web_search_call"}],"tools":[{"type":"function","name":"f","parameters":{"type":"object"}}]});
    assert!(ensure_web_search_history(
        value.as_object_mut().unwrap(),
        false
    ));
    assert_eq!(
        value["tool_choice"],
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"f"}]})
    );
    let mut invalid =
        json!({"input":[{"type":"web_search_call"}],"tools":[],"tool_choice":"required"});
    assert!(!ensure_web_search_history(
        invalid.as_object_mut().unwrap(),
        false
    ));
}
