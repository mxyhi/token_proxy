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
    let upstream = futures_util::stream::iter(
        events
            .into_iter()
            .map(|event| Ok::<_, std::io::Error>(Bytes::from(format!("data: {event}\n\n")))),
    );
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
