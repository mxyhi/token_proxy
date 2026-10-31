use super::*;

async fn claude_events(events: Vec<Value>) -> Vec<Value> {
    let (log, context, _) = setup_responses_stream().await;
    let input = events
        .iter()
        .map(|value| format!("data: {value}\n\n"))
        .collect::<String>();
    let chunks = input
        .as_bytes()
        .chunks(7)
        .map(|bytes| Ok::<_, std::io::Error>(Bytes::copy_from_slice(bytes)))
        .collect::<Vec<_>>();
    super::super::responses_to_anthropic::stream_responses_to_anthropic(
        futures_util::stream::iter(chunks),
        context,
        log,
        crate::proxy::token_rate::RequestTokenTracker::disabled(),
    )
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .filter_map(|chunk| parse_anthropic_sse(&chunk.unwrap()).map(|(_, event)| event))
    .collect()
}

#[tokio::test]
async fn commentary_stream_and_done_only_final_answer_have_separate_blocks() {
    let commentary = json!({"id":"comment","type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"检查中"}]});
    let answer = json!({"id":"answer","type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"完成"}]});
    let events = claude_events(vec![
        json!({"type":"response.output_item.added","output_index":0,"item":commentary}),
        json!({"type":"response.output_text.delta","item_id":"comment","delta":"检查"}),
        json!({"type":"response.output_item.done","output_index":0,"item":commentary}),
        json!({"type":"response.output_item.done","output_index":1,"item":answer}),
        json!({"type":"response.completed","response":{"id":"resp","status":"completed","output":[commentary,answer]}}),
    ]).await;
    let thinking = events
        .iter()
        .filter_map(|e| e.pointer("/delta/thinking").and_then(Value::as_str))
        .collect::<String>();
    let text = events
        .iter()
        .filter_map(|e| e.pointer("/delta/text").and_then(Value::as_str))
        .collect::<String>();
    assert_eq!(thinking, "检查中");
    assert_eq!(text, "完成");
    let markers = events
        .iter()
        .filter(|e| {
            e.pointer("/delta/signature").and_then(Value::as_str)
                == Some(crate::proxy::claude_reasoning::COMMENTARY_MARKER)
        })
        .count();
    assert_eq!(markers, 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "content_block_start")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "content_block_stop")
            .count(),
        2
    );
}

#[tokio::test]
async fn commentary_terminal_snapshot_does_not_take_reasoning_signature() {
    let carrier = crate::proxy::claude_reasoning::signed_thinking_carrier(
        "thinking",
        Some("reason"),
        Some("provider-signature"),
        None,
    )
    .unwrap();
    let events = claude_events(vec![
        json!({"type":"response.output_item.added","item":{"id":"reason","type":"reasoning"}}),
        json!({"type":"response.reasoning_summary_text.delta","item_id":"reason","delta":"reason"}),
        json!({"type":"response.output_item.done","item":{"id":"reason","type":"reasoning","encrypted_content":carrier}}),
        json!({"type":"response.completed","response":{"id":"resp","status":"completed","output":[
        {"id":"comment","type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"working"}]},
        {"id":"answer","type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}
    ]}})]).await;
    assert_eq!(
        events
            .iter()
            .filter_map(|e| e.pointer("/delta/thinking").and_then(Value::as_str))
            .collect::<String>(),
        "reasonworking"
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|e| e.pointer("/delta/text").and_then(Value::as_str))
            .collect::<String>(),
        "done"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "content_block_start")
            .count(),
        3
    );
    let signatures: Vec<&Value> = events
        .iter()
        .filter(|e| e.pointer("/delta/signature").is_some())
        .collect();
    assert_eq!(signatures.len(), 2);
    assert_ne!(signatures[0]["index"], signatures[1]["index"]);
    assert_eq!(signatures[0]["delta"]["signature"], "provider-signature");
}

#[tokio::test]
async fn claude_thinking_usage_preserves_zero_and_omission_through_responses() {
    for tokens in [None, Some(0), Some(7)] {
        let (log, context, _) = setup_responses_stream().await;
        let mut usage = json!({"output_tokens":12});
        if let Some(tokens) = tokens {
            usage["output_tokens_details"] = json!({"thinking_tokens":tokens});
        }
        let input = [
            json!({"type":"message_start","message":{"id":"msg","model":"claude","usage":{"input_tokens":8,"output_tokens":0}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":usage}),
            json!({"type":"message_stop"}),
        ].into_iter().map(|value|Ok::<_,std::io::Error>(Bytes::from(format!("data: {value}\n\n"))));
        let events = super::super::anthropic_to_responses::stream_anthropic_to_responses(
            futures_util::stream::iter(input),
            context,
            log,
            crate::proxy::token_rate::RequestTokenTracker::disabled(),
        )
        .collect::<Vec<_>>()
        .await;
        let terminal = events
            .iter()
            .filter_map(|chunk| parse_sse_json(chunk.as_ref().unwrap()))
            .find(|event| event["type"] == "response.completed")
            .unwrap();
        assert_eq!(
            terminal
                .pointer("/response/usage/output_tokens_details/reasoning_tokens")
                .and_then(Value::as_u64),
            tokens
        );
    }
}
