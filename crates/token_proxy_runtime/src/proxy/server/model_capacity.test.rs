use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn capacity_retry_after_blocks_next_request_for_only_the_affected_model() {
    run_async(async {
        async fn handler(State(hits): State<Arc<AtomicUsize>>) -> axum::response::Response {
            if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                return axum::response::Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .header("retry-after", "60")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"error":{"code":"turn_rate_limit_exceeded","message":"Model at capacity"}}).to_string())).unwrap();
            }
            axum::Json(
                json!({"id":"resp_ok","output":[],"usage":{"input_tokens":1,"output_tokens":1}}),
            )
            .into_response()
        }
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/{*path}", any(handler))
            .with_state(hits.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "capacity",
            &url,
            FORMATS_RESPONSES,
        )]);
        let state =
            build_test_state_handle(config, next_test_data_dir("model-capacity-wait")).await;
        for _ in 0..2 {
            let response = proxy_request(
                State(state.clone()),
                Method::POST,
                Uri::from_static(RESPONSES_PATH),
                HeaderMap::new(),
                Body::from(json!({"model":"gpt-5","input":"hi"}).to_string()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(response.headers().contains_key("retry-after"));
            assert_eq!(
                hits.load(Ordering::SeqCst),
                1,
                "capacity wait must span requests"
            );
        }
        let (status, _) =
            send_responses_request_with_body(state, json!({"model":"another-model","input":"hi"}))
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "other models remain usable");
        task.abort();
    });
}
