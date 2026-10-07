use super::*;
use std::time::Duration;

#[test]
fn oversized_inbound_body_is_rejected_before_dispatch() {
    run_async(async {
        let mut config = config_with_providers(&[]);
        config.max_request_body_bytes = 1024;
        let dir = next_test_data_dir("body_limit");
        let state = build_test_state_handle(config, dir.clone()).await;
        for chunked in [false, true] {
            let mut headers = HeaderMap::new();
            let body = if chunked {
                Body::from_stream(futures_util::stream::iter(vec![
                    Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 512])),
                    Ok(Bytes::from(vec![b'x'; 513])),
                ]))
            } else {
                headers.insert("content-length", HeaderValue::from_static("1025"));
                Body::from(vec![b'x'; 1025])
            };
            let response = proxy_request(
                State(state.clone()),
                Method::POST,
                Uri::from_static("/v1/chat/completions"),
                headers,
                body,
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::PAYLOAD_TOO_LARGE,
                "chunked={chunked}"
            );
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("limit"));
        }
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn real_router_enforces_body_limit_for_fixed_and_chunked_uploads() {
    run_async(async {
        let mut config = config_with_providers(&[]);
        config.max_request_body_bytes = 1024;
        let dir = next_test_data_dir("router_body_limit");
        let state = build_test_state_handle(config, dir.clone()).await;
        let app = super::super::build_router(state.clone(), 1024).with_state::<()>(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        for size in [1024, 1025] {
            for chunked in [false, true] {
                let payload = vec![b'x'; size];
                let body = if chunked {
                    reqwest::Body::wrap_stream(futures_util::stream::iter(vec![Ok::<
                        _,
                        std::io::Error,
                    >(
                        payload
                    )]))
                } else {
                    reqwest::Body::from(payload)
                };
                let response = client
                    .post(format!("http://{addr}/v1/chat/completions"))
                    .body(body)
                    .send()
                    .await
                    .unwrap();
                // 无上游配置时合法大小进入路由后为 502；超限必须在路由前成为 413。
                let expected = if size > 1024 {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_GATEWAY
                };
                assert_eq!(
                    response.status(),
                    expected,
                    "size={size}, chunked={chunked}"
                );
                let _ = response.bytes().await.unwrap();
            }
        }
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    });
}
