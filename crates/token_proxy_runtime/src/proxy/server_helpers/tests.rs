use super::*;

use axum::body::{Body, Bytes};

const TEST_MAX_REQUEST_BODY_BYTES: usize = 100 * 1024 * 1024;

#[test]
fn debug_header_snapshot_redacts_credentials_but_keeps_safe_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer secret".parse().expect("header"));
    headers.insert(
        "proxy-authorization",
        "Basic secret".parse().expect("header"),
    );
    headers.insert("cookie", "session=secret".parse().expect("header"));
    headers.insert("x-api-key", "secret-key".parse().expect("header"));
    headers.insert("x-xai-token-auth", "xai-grok-cli".parse().expect("header"));
    headers.insert("content-type", "application/json".parse().expect("header"));

    let snapshot = snapshot_headers_raw(&headers);
    let value_for = |name: &str| {
        snapshot
            .iter()
            .find_map(|(header_name, value)| (header_name == name).then_some(value.as_str()))
    };
    for name in [
        "authorization",
        "proxy-authorization",
        "cookie",
        "x-api-key",
        "x-xai-token-auth",
    ] {
        assert_eq!(value_for(name), Some("[redacted]"));
    }
    assert_eq!(value_for("content-type"), Some("application/json"));
    assert!(snapshot.iter().all(|(_, value)| !value.contains("secret")));
}

#[test]
fn force_openai_chat_stream_usage_inserts_stream_options_include_usage() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let input = Bytes::from_static(br#"{"stream":true,"messages":[]}"#);
        let meta = RequestMeta {
            client_ip: None,
            stream: true,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
            estimated_input_tokens: None,
            billing: Default::default(),
        };
        let body = ReplayableBody::from_bytes(input);
        let output = maybe_force_openai_stream_options_include_usage(
            PROVIDER_CHAT,
            CHAT_PATH,
            &meta,
            body,
            TEST_MAX_REQUEST_BODY_BYTES,
        )
        .await
        .expect("ok");
        let bytes = output
            .read_bytes_if_small(1024)
            .await
            .expect("read")
            .expect("bytes");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["stream_options"]["include_usage"], Value::Bool(true));
    });
}

#[test]
fn gemini_meta_prefers_path_for_stream_and_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let body = ReplayableBody::from_bytes(Bytes::from_static(b"{}"));
        let meta = parse_request_meta_best_effort(
            "/v1beta/models/gemini-1.5-flash:streamGenerateContent",
            &body,
            TEST_MAX_REQUEST_BODY_BYTES,
        )
        .await;
        assert!(meta.stream);
        assert_eq!(meta.original_model.as_deref(), Some("gemini-1.5-flash"));
    });
}

#[test]
fn gemini_meta_treats_generate_content_alt_sse_as_stream() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let body = ReplayableBody::from_bytes(Bytes::from_static(b"{}"));
        let meta = parse_request_meta_best_effort(
            "/v1beta/models/gemini-1.5-flash:generateContent?alt=sse",
            &body,
            TEST_MAX_REQUEST_BODY_BYTES,
        )
        .await;
        assert!(meta.stream);
        assert_eq!(meta.original_model.as_deref(), Some("gemini-1.5-flash"));
    });
}

#[test]
fn meta_parses_large_request_body_for_stream_and_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let padding = "x".repeat((2 * 1024 * 1024) + 128);
        let request =
            format!(r#"{{"model":"gpt-5.4","stream":true,"input":"hello","padding":"{padding}"}}"#);
        let body = ReplayableBody::from_body(Body::from(request))
            .await
            .expect("body");

        let meta =
            parse_request_meta_best_effort(RESPONSES_PATH, &body, TEST_MAX_REQUEST_BODY_BYTES)
                .await;

        assert!(meta.stream);
        assert_eq!(meta.original_model.as_deref(), Some("gpt-5.4"));
    });
}

#[test]
fn meta_parses_reasoning_suffix_and_strips_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let body = ReplayableBody::from_bytes(Bytes::from_static(
            br#"{"model":"gpt-4.1-reasoning-high","messages":[]}"#,
        ));
        let meta =
            parse_request_meta_best_effort(CHAT_PATH, &body, TEST_MAX_REQUEST_BODY_BYTES).await;
        assert_eq!(meta.original_model.as_deref(), Some("gpt-4.1"));
        assert_eq!(meta.reasoning_effort.as_deref(), Some("high"));
    });
}

#[test]
fn anthropic_meta_strips_repeated_case_insensitive_one_meg_suffixes() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let body = ReplayableBody::from_bytes(Bytes::from_static(
            br#"{"model":"claude-opus-4-6[1M][1m]","messages":[]}"#,
        ));

        let meta = parse_request_meta_best_effort(
            ANTHROPIC_MESSAGES_PREFIX,
            &body,
            TEST_MAX_REQUEST_BODY_BYTES,
        )
        .await;

        assert_eq!(meta.original_model.as_deref(), Some("claude-opus-4-6"));
    });
}

#[test]
fn apply_reasoning_suffix_for_chat_sets_reasoning_effort_and_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let meta = RequestMeta {
            client_ip: None,
            stream: false,
            original_model: Some("gpt-4.1".to_string()),
            mapped_model: None,
            reasoning_effort: Some("high".to_string()),
            response_format: None,
            estimated_input_tokens: None,
            billing: Default::default(),
        };
        let body = ReplayableBody::from_bytes(Bytes::from_static(
            br#"{"model":"gpt-4.1-reasoning-high","messages":[]}"#,
        ));
        let rewritten = maybe_rewrite_openai_reasoning_effort_from_model_suffix(
            PROVIDER_CHAT,
            CHAT_PATH,
            &meta,
            &body,
        )
        .await
        .expect("ok")
        .expect("should rewrite");
        let bytes = rewritten
            .read_bytes_if_small(1024)
            .await
            .expect("read")
            .expect("bytes");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["model"], Value::String("gpt-4.1".to_string()));
        assert_eq!(value["reasoning_effort"], Value::String("high".to_string()));
    });
}

#[test]
fn apply_reasoning_suffix_for_responses_sets_reasoning_object_and_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let meta = RequestMeta {
            client_ip: None,
            stream: false,
            original_model: Some("gpt-4.1".to_string()),
            mapped_model: None,
            reasoning_effort: Some("high".to_string()),
            response_format: None,
            estimated_input_tokens: None,
            billing: Default::default(),
        };
        let body = ReplayableBody::from_bytes(Bytes::from_static(
            br#"{"model":"gpt-4.1-reasoning-high","input":"hi"}"#,
        ));
        let rewritten = maybe_rewrite_openai_reasoning_effort_from_model_suffix(
            PROVIDER_RESPONSES,
            RESPONSES_PATH,
            &meta,
            &body,
        )
        .await
        .expect("ok")
        .expect("should rewrite");
        let bytes = rewritten
            .read_bytes_if_small(1024)
            .await
            .expect("read")
            .expect("bytes");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["model"], Value::String("gpt-4.1".to_string()));
        assert_eq!(
            value["reasoning"]["effort"],
            Value::String("high".to_string())
        );
    });
}

#[test]
fn apply_reasoning_suffix_prefers_mapped_model_as_upstream_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let meta = RequestMeta {
            client_ip: None,
            stream: false,
            original_model: Some("gpt-4.1".to_string()),
            mapped_model: Some("o3-mini".to_string()),
            reasoning_effort: Some("high".to_string()),
            response_format: None,
            estimated_input_tokens: None,
            billing: Default::default(),
        };
        let body = ReplayableBody::from_bytes(Bytes::from_static(
            br#"{"model":"gpt-4.1-reasoning-high","messages":[]}"#,
        ));
        let rewritten = maybe_rewrite_openai_reasoning_effort_from_model_suffix(
            PROVIDER_CHAT,
            CHAT_PATH,
            &meta,
            &body,
        )
        .await
        .expect("ok")
        .expect("should rewrite");
        let bytes = rewritten
            .read_bytes_if_small(1024)
            .await
            .expect("read")
            .expect("bytes");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["model"], Value::String("o3-mini".to_string()));
        assert_eq!(value["reasoning_effort"], Value::String("high".to_string()));
    });
}
