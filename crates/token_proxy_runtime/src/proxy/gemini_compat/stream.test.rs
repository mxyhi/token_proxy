use super::*;

async fn converted(events: Vec<Value>, gemini: bool) -> String {
    let context = LogContext {
        client_ip: None,
        path: "/v1/chat/completions".into(),
        provider: "gemini".into(),
        upstream_id: "test".into(),
        account_id: None,
        model: Some("test".into()),
        mapped_model: None,
        stream: true,
        status: 200,
        upstream_request_id: None,
        request_headers: None,
        request_body: None,
        client_request_body: None,
        ttfb_ms: None,
        timings: Default::default(),
        start: std::time::Instant::now(),
    };
    let tracker = crate::proxy::token_rate::TokenRateTracker::new()
        .register(None)
        .await;
    let upstream = futures_util::stream::iter(events.into_iter().map(|event| {
        let data = if event == "[DONE]" {
            "[DONE]".to_string()
        } else {
            event.to_string()
        };
        Ok::<_, std::io::Error>(Bytes::from(format!("data: {data}\n\n")))
    }));
    let log = Arc::new(LogWriter::new(None));
    let mut stream = if gemini {
        stream_gemini_to_chat(upstream, context, log, tracker).boxed()
    } else {
        stream_chat_to_gemini(upstream, context, log, tracker).boxed()
    };
    let mut body = String::new();
    while let Some(chunk) = stream.next().await {
        body.push_str(&String::from_utf8(chunk.unwrap().to_vec()).unwrap());
    }
    body
}

#[tokio::test]
async fn gemini_truncation_and_malformed_call_fail_without_success() {
    for tail in [
        None,
        Some(json!({"error":{"message":"private upstream detail"}})),
        Some(json!({"candidates":[{"finishReason":"MALFORMED_FUNCTION_CALL"}]})),
    ] {
        let mut events = vec![json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]})];
        events.extend(tail);
        let body = converted(events, true).await;
        assert!(body.contains("partial"));
        assert!(body.contains("upstream_stream_error"), "{body}");
        assert!(!body.contains("[DONE]"));
        assert!(!body.contains("private upstream detail"));
    }
    assert!(converted(vec![], true)
        .await
        .contains("upstream_stream_error"));
}

#[tokio::test]
async fn gemini_terminal_keeps_tail_usage() {
    let body = converted(vec![json!({"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}]}),json!({"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13}})],true).await;
    assert!(body.contains("\"completion_tokens\":3"), "{body}");
    assert_eq!(body.matches("[DONE]").count(), 1);
    assert!(!body.contains("upstream_stream_error"));
}

#[tokio::test]
async fn chat_to_gemini_truncated_stream_is_not_stop() {
    let body = converted(
        vec![json!({"choices":[{"delta":{"content":"partial"}}]})],
        false,
    )
    .await;
    assert!(body.contains("error"), "{body}");
    assert!(!body.contains("\"finishReason\":\"STOP\""));
}

#[tokio::test]
async fn gemini_prompt_block_is_an_error_and_terminal_reason_survives_tool_calls() {
    let blocked = converted(vec![json!({"promptFeedback":{"blockReason":"SAFETY"},"usageMetadata":{"promptTokenCount":12}})], true).await;
    assert!(blocked.contains("upstream_stream_error"));
    assert!(!blocked.contains("[DONE]"));
    for (reason, expected) in [("IMAGE_SAFETY", "content_filter"), ("MAX_TOKENS", "length")] {
        let body = converted(vec![json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"f","args":{}}}]}}]}), json!({"candidates":[{"finishReason":reason}]})], true).await;
        assert!(
            body.contains(&format!("\"finish_reason\":\"{expected}\"")),
            "{body}"
        );
    }
}

#[tokio::test]
async fn vertex_partial_args_emit_one_call_and_keep_signature_and_indices() {
    let body = converted(vec![
        json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"f","partialArgs":[{"jsonPath":"$.text","stringValue":"你","willContinue":true}],"willContinue":true},"thoughtSignature":"opaque"}]}}]}),
        json!({"candidates":[{"content":{"parts":[{"functionCall":{"partialArgs":[{"jsonPath":"$.text","stringValue":"好"}],"willContinue":false}},{"functionCall":{"name":"g","args":{"n":2}}}]},"finishReason":"STOP"}]}),
    ],true).await;
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let calls: Vec<&Value> = events
        .iter()
        .filter_map(|event| event.pointer("/choices/0/delta/tool_calls/0"))
        .collect();
    assert_eq!(calls.len(), 2, "{body}");
    assert_eq!(calls[0]["index"], 0);
    assert_eq!(calls[1]["index"], 1);
    assert_eq!(calls[0]["function"]["arguments"], "{\"text\":\"你好\"}");
    assert_eq!(
        calls[0]["provider_specific_fields"]["thought_signature"],
        "opaque"
    );
    assert!(body.contains("[DONE]"));
}

#[tokio::test]
async fn vertex_unfinished_partial_call_is_not_an_empty_successful_call() {
    let body = converted(vec![json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"f","willContinue":true,"partialArgs":[]}}]},"finishReason":"STOP"}]})],true).await;
    assert!(body.contains("upstream_stream_error"));
    assert!(!body.contains("\"tool_calls\""));
    assert!(!body.contains("[DONE]"));
}

#[tokio::test]
async fn chat_empty_tool_announcement_waits_for_arguments() {
    let body = converted(vec![
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"f","arguments":""}}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"value\":1}"}}]}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
    ],false).await;
    assert_eq!(body.matches("\"functionCall\"").count(), 1, "{body}");
    assert!(body.contains("\"args\":{\"value\":1}"), "{body}");
}

#[tokio::test]
async fn chat_to_gemini_preserves_non_normal_finish_reasons() {
    for (reason, expected) in [("length", "MAX_TOKENS"), ("content_filter", "SAFETY")] {
        let body = converted(
            vec![json!({"choices":[{"delta":{"content":"partial"},"finish_reason":reason}]})],
            false,
        )
        .await;
        assert!(
            body.contains(&format!("\"finishReason\":\"{expected}\"")),
            "{body}"
        );
        assert!(!body.contains("\"finishReason\":\"STOP\""));
    }
}

#[tokio::test]
async fn done_sentinel_does_not_make_truncated_stream_successful() {
    let gemini = converted(
        vec![
            json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}),
            json!("[DONE]"),
        ],
        true,
    )
    .await;
    assert!(gemini.contains("upstream_stream_error"));
    assert!(!gemini.contains("\"finish_reason\":\"stop\""));
    let chat = converted(
        vec![
            json!({"choices":[{"delta":{"content":"partial"}}]}),
            json!("[DONE]"),
        ],
        false,
    )
    .await;
    assert!(chat.contains("\"error\""));
    assert!(!chat.contains("\"finishReason\":\"STOP\""));
}
