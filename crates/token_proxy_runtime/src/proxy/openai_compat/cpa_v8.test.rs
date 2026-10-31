use super::*;

#[test]
fn responses_historical_tools_are_promoted_before_chat_name_mapping() {
    let value = json!({
        "model":"gpt-5", "tools":[{"type":"function","name":"lookup","description":"root","parameters":{"type":"object"}}],
        "input":[
            {"type":"additional_tools","tools":[{"type":"function","name":"lookup","description":"stale"},{"type":"namespace","name":"repo","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]}]},
            {"type":"function_call","call_id":"call_1","namespace":"repo","name":"read","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_1","output":"missing file","is_error":true}
        ]
    });
    let output = json_from_bytes(responses_request_to_chat(&bytes_from_json(value)).unwrap());
    assert_eq!(output["tools"].as_array().unwrap().len(), 2);
    assert_eq!(output["tools"][0]["function"]["description"], "root");
    assert_eq!(output["tools"][1]["function"]["name"], "repo__read");
    assert_eq!(
        output["messages"][0]["tool_calls"][0]["function"]["name"],
        "repo__read"
    );
    assert_eq!(output["messages"][1]["is_error"], true);
}

#[test]
fn claude_failed_tool_results_reach_gemini_error_payload() {
    let clients = ProxyHttpClients::new().unwrap();
    let output = transform_request_value(
        FormatTransform::AnthropicToGemini,
        json!({
            "model":"gemini-3.5-pro", "max_tokens":128,
            "tools":[{"name":"lookup","input_schema":{"type":"object"}}],
            "messages":[
                {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","is_error":true,"content":"missing file"}]}
            ]
        }),
        &clients,
        None,
    );
    let response = &output["contents"][1]["parts"][0]["functionResponse"]["response"];
    assert!(response.get("error").is_some(), "{output}");
    assert!(response.get("result").is_none());
}
