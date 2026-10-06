use super::*;

fn shell_request() -> Value {
    json!({"model":"unit-model", "input":"inspect", "tools":[
        {"type":"shell","environment":{"type":"local"}},
        {"type":"function","name":"__token_proxy_local_shell","parameters":{"type":"object"}}
    ], "tool_choice":{"type":"shell"}})
}

#[test]
fn local_shell_declaration_choice_history_and_response_roundtrip() {
    let clients = ProxyHttpClients::new().unwrap();
    let mut request = shell_request();
    let action = json!({"commands":["pwd","ls"],"timeout_ms":1000,"max_output_length":4096});
    let result = json!({"type":"shell_call_output","call_id":"shell_a", "max_output_length":4096,
        "output":[{"stdout":"ok","stderr":"warning","outcome":{"type":"exit","exit_code":3}},
        {"stdout":"partial","stderr":"","outcome":{"type":"timeout"}}]});
    request["input"] = json!([
        {"type":"shell_call","id":"sh_a","call_id":"shell_a","action":action,"status":"completed"}, result
    ]);
    let chat = transform_request_value(
        FormatTransform::ResponsesToChat,
        request.clone(),
        &clients,
        None,
    );
    let name = chat["tool_choice"]["function"]["name"]
        .as_str()
        .expect("shell choice mapped");
    assert_ne!(name, "__token_proxy_local_shell");
    assert!(chat["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["type"] == "function"));
    assert_eq!(chat["messages"][0]["tool_calls"][0]["id"], "shell_a");
    assert_eq!(
        chat["messages"][0]["tool_calls"][0]["function"]["name"],
        name
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            chat["messages"][0]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        action
    );
    assert_eq!(
        serde_json::from_str::<Value>(chat["messages"][1]["content"].as_str().unwrap()).unwrap(),
        result
    );

    let response = bytes_from_json(json!({"choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","tool_calls":[{"id":"shell_b","type":"function","function":{"name":name,"arguments":action.to_string()}}]
    }}]}));
    let converted = transform_response_body_with_request_body(
        FormatTransform::ChatToResponses,
        &response,
        None,
        Some(&request.to_string()),
    )
    .unwrap();
    let converted = json_from_bytes(converted);
    assert_eq!(converted["output"][0]["type"], "shell_call");
    assert_eq!(converted["output"][0]["call_id"], "shell_b");
    assert_eq!(converted["output"][0]["action"], action);

    request.as_object_mut().unwrap().remove("tools");
    request.as_object_mut().unwrap().remove("tool_choice");
    let historical =
        transform_request_value(FormatTransform::ResponsesToChat, request, &clients, None);
    assert!(historical.get("tools").is_none());
    assert_eq!(historical["messages"][0]["tool_calls"][0]["id"], "shell_a");
    assert_eq!(
        serde_json::from_str::<Value>(historical["messages"][1]["content"].as_str().unwrap())
            .unwrap(),
        result
    );
}

#[test]
fn local_shell_rejects_hosted_and_invalid_returned_actions() {
    let clients = ProxyHttpClients::new().unwrap();
    let mut hosted = shell_request();
    hosted["tools"][0]["environment"]["type"] = json!("container_auto");
    let error = run_async(transform_request_body(
        FormatTransform::ResponsesToChat,
        &bytes_from_json(hosted.clone()),
        &clients,
        None,
    ))
    .unwrap_err();
    assert!(error.contains("local"));
    assert_eq!(
        run_async(transform_request_body(
            FormatTransform::None,
            &bytes_from_json(hosted.clone()),
            &clients,
            None
        ))
        .unwrap(),
        bytes_from_json(hosted)
    );
    let request = shell_request();
    let chat = transform_request_value(
        FormatTransform::ResponsesToChat,
        request.clone(),
        &clients,
        None,
    );
    let name = &chat["tool_choice"]["function"]["name"];
    for arguments in [
        "{}",
        "{\"commands\":[]}",
        "{\"commands\":[1]}",
        "{\"commands\":[\" \" ]}",
        "{\"commands\":[\"pwd\"],\"timeout_ms\":0}",
        "{\"commands\":[\"pwd\"],\"max_output_length\":1.5}",
        "{\"commands\":[\"pwd\"],\"timeout_ms\":null}",
        "{\"commands\":[",
    ] {
        let response = bytes_from_json(
            json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[
                {"id":"s","function":{"name":name,"arguments":arguments}}
            ]}}]}),
        );
        let err = transform_response_body_with_request_body(
            FormatTransform::ChatToResponses,
            &response,
            None,
            Some(&request.to_string()),
        )
        .unwrap_err();
        assert!(err.contains("shell"), "{err}");
    }
}
