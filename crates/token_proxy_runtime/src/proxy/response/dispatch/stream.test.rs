use super::*;
use axum::{body::to_bytes, http::header::CONTENT_TYPE};

fn test_context() -> LogContext {
    LogContext {
        client_ip: None,
        path: "/v1/responses".to_string(),
        provider: PROVIDER_XAI.to_string(),
        upstream_id: "xai-default".to_string(),
        account_id: Some("xai-account".to_string()),
        model: Some("grok-4.5".to_string()),
        mapped_model: None,
        stream: true,
        status: StatusCode::OK.as_u16(),
        upstream_request_id: None,
        request_headers: None,
        request_body: None,
        client_request_body: None,
        ttfb_ms: None,
        timings: Default::default(),
        start: std::time::Instant::now(),
    }
}

fn stream_error(
    code: &str,
    message: &str,
    status: StatusCode,
) -> responses_error::ResponsesStreamError {
    responses_error::ResponsesStreamError {
        message: message.to_string(),
        error_type: "invalid_request_error".to_string(),
        code: Some(json!(code)),
        status,
        retryable_before_output: true,
    }
}

#[tokio::test]
async fn xai_request_policy_prelude_becomes_non_retryable_http_forbidden() {
    let mut context = test_context();
    let response = responses_prelude_retry_response(
        StatusCode::OK,
        &HeaderMap::new(),
        FormatTransform::None,
        stream_error(
            "new_sensitive",
            "image is sensitive",
            StatusCode::BAD_GATEWAY,
        ),
        &mut context,
        &Arc::new(LogWriter::new(None)),
    );

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    assert!(response
        .extensions()
        .get::<NonRetryableSemanticResponse>()
        .is_some());
    assert!(response
        .extensions()
        .get::<RetryableStreamResponse>()
        .is_none());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error body");
    let value: Value = serde_json::from_slice(&body).expect("error JSON");
    assert_eq!(
        value["error"]["message"],
        "new_sensitive: image is sensitive"
    );
}

#[tokio::test]
async fn xai_account_and_unknown_prelude_keep_retryable_account_semantics() {
    for error in [
        stream_error(
            "account_suspended",
            "account suspended",
            StatusCode::FORBIDDEN,
        ),
        stream_error(
            "policy_violation",
            "policy violation",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut context = test_context();
        let response = responses_prelude_retry_response(
            StatusCode::OK,
            &HeaderMap::new(),
            FormatTransform::None,
            error,
            &mut context,
            &Arc::new(LogWriter::new(None)),
        );

        assert!(response
            .extensions()
            .get::<NonRetryableSemanticResponse>()
            .is_none());
        assert!(response
            .extensions()
            .get::<RetryableStreamResponse>()
            .is_some());
    }
}
