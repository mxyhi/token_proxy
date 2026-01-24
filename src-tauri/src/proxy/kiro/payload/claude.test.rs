use super::*;

#[test]
fn build_payload_from_claude_includes_tools_and_results() {
    let request = json!({
        "model": "claude-sonnet-4.5",
        "tool_choice": { "type": "tool", "name": "mcp__demo__ping" },
        "tools": [
            {
                "name": "mcp__demo__ping",
                "description": "",
                "input_schema": { "type": "object", "properties": {} }
            }
        ],
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": "hi" },
                    { "type": "tool_result", "tool_use_id": "toolu_1", "is_error": true, "content": [{ "type": "text", "text": "fail" }] }
                ]
            }
        ]
    });
    let headers = HeaderMap::new();
    let result = build_payload_from_claude(
        &request,
        "claude-sonnet-4.5",
        None,
        "CLI",
        false,
        false,
        &headers,
    )
    .expect("payload");
    let payload: Value = serde_json::from_slice(&result.payload).expect("json");
    let context = payload
        .get("conversationState")
        .and_then(Value::as_object)
        .and_then(|state| state.get("currentMessage"))
        .and_then(Value::as_object)
        .and_then(|msg| msg.get("userInputMessage"))
        .and_then(Value::as_object)
        .and_then(|msg| msg.get("userInputMessageContext"))
        .and_then(Value::as_object)
        .expect("context");
    assert!(context.get("tools").and_then(Value::as_array).is_some());
    assert_eq!(
        context
            .get("toolResults")
            .and_then(Value::as_array)
            .map(|items| items.len()),
        Some(1)
    );
}
