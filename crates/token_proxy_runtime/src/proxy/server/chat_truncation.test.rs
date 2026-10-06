use super::*;

#[test]
fn chat_truncation_after_output_never_replays_the_request() {
    run_async(async {
        for path in [CHAT_PATH, RESPONSES_PATH] {
            let primary = spawn_mock_raw_upstream(
                StatusCode::OK,
                Bytes::from(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"partial before EOF\"}}]}\n\n",
                ),
                "text/event-stream",
            )
            .await;
            let fallback = spawn_mock_raw_upstream(
                StatusCode::OK,
                Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"fallback\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"),
                "text/event-stream",
            ).await;
            let mut config = config_with_runtime_upstreams(&[
                (PROVIDER_CHAT, 10, "primary", &primary.base_url, FORMATS_ALL),
                (
                    PROVIDER_CHAT,
                    0,
                    "fallback",
                    &fallback.base_url,
                    FORMATS_ALL,
                ),
            ]);
            config.same_upstream_retry_count = 2;
            let data_dir = next_test_data_dir("chat_truncation_no_replay");
            let state = build_test_state_handle(config, data_dir.clone()).await;
            let request = if path == CHAT_PATH {
                json!({"model":"unit-model","messages":[{"role":"user","content":"hi"}],"stream":true})
            } else {
                json!({"model":"unit-model","input":"hi","stream":true})
            };
            let response = proxy_request(
                State(state),
                Method::POST,
                Uri::from_static(path),
                axum::http::HeaderMap::new(),
                Body::from(request.to_string()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert!(text.contains("partial before EOF"), "{text}");
            assert!(text.contains("truncated"), "{text}");
            if path == RESPONSES_PATH {
                assert!(text.contains("response.failed"), "{text}");
            }
            assert_eq!(primary.requests().len(), 1);
            assert!(fallback.requests().is_empty());
            primary.abort();
            fallback.abort();
            let _ = std::fs::remove_dir_all(&data_dir);
        }
    });
}
