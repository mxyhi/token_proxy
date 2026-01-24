use super::*;

use axum::body::Bytes;

#[test]
fn force_openai_chat_stream_usage_inserts_stream_options_include_usage() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let input = Bytes::from_static(br#"{"stream":true,"messages":[]}"#);
        let meta = RequestMeta {
            stream: true,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            estimated_input_tokens: None,
        };
        let body = ReplayableBody::from_bytes(input);
        let output =
            maybe_force_openai_stream_options_include_usage(PROVIDER_CHAT, CHAT_PATH, &meta, body)
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
        )
        .await;
        assert!(meta.stream);
        assert_eq!(meta.original_model.as_deref(), Some("gemini-1.5-flash"));
    });
}

#[test]
fn meta_parses_reasoning_suffix_and_strips_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let body = ReplayableBody::from_bytes(Bytes::from_static(
            br#"{"model":"gpt-4.1-reasoning-high","messages":[]}"#,
        ));
        let meta = parse_request_meta_best_effort(CHAT_PATH, &body).await;
        assert_eq!(meta.original_model.as_deref(), Some("gpt-4.1"));
        assert_eq!(meta.reasoning_effort.as_deref(), Some("high"));
    });
}

#[test]
fn apply_reasoning_suffix_for_chat_sets_reasoning_effort_and_model() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let meta = RequestMeta {
            stream: false,
            original_model: Some("gpt-4.1".to_string()),
            mapped_model: None,
            reasoning_effort: Some("high".to_string()),
            estimated_input_tokens: None,
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
            stream: false,
            original_model: Some("gpt-4.1".to_string()),
            mapped_model: None,
            reasoning_effort: Some("high".to_string()),
            estimated_input_tokens: None,
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
            stream: false,
            original_model: Some("gpt-4.1".to_string()),
            mapped_model: Some("o3-mini".to_string()),
            reasoning_effort: Some("high".to_string()),
            estimated_input_tokens: None,
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
