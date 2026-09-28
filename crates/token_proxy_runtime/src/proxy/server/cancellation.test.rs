use super::*;
use std::time::Duration;
use tokio::{net::TcpStream, sync::oneshot};

async fn start_proxy(state: ProxyStateHandle) -> (SocketAddr, JoinHandle<()>) {
    let app = build_router(state.clone(), 20 * 1024 * 1024).with_state::<()>(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy");
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, task)
}

async fn open_client(addr: SocketAddr, streaming: bool) -> TcpStream {
    let body = json!({"model":"gpt-5.6-sol", "stream":streaming, "messages":[{"role":"user","content":"hi"}]}).to_string();
    let mut socket = TcpStream::connect(addr).await.expect("connect proxy");
    socket.write_all(format!("POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    socket
}

async fn gated_upstream(
    streaming: bool,
) -> (
    String,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
    JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = oneshot::channel();
    let (closed_tx, closed_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_scripted_wire_request(&mut socket, 0).await;
        if streaming {
            let chunk = "data: {\"id\":\"chat-1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5.6-sol\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"cancel-me\"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":1,\"total_tokens\":8}}\n\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{chunk}\r\n",chunk.len()).as_bytes()).await.unwrap();
        }
        let _ = seen_tx.send(());
        let mut byte = [0; 1];
        let _ = socket.read(&mut byte).await;
        let _ = closed_tx.send(());
    });
    (format!("http://{addr}"), seen_rx, closed_rx, task)
}

async fn read_until(socket: &mut TcpStream, marker: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        while !String::from_utf8_lossy(&bytes).contains(marker) {
            let mut buf = [0; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "response ended before marker");
            bytes.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .expect("client receives first output");
}

#[test]
fn client_cancellation_before_headers_and_during_sse_is_499_without_retry() {
    run_async(async {
        for streaming in [false, true] {
            let (url, seen, closed, upstream_task) = gated_upstream(streaming).await;
            let fallback = spawn_mock_upstream(StatusCode::OK, json!({"choices":[]})).await;
            let mut config = config_with_runtime_upstreams(&[
                (PROVIDER_CHAT, 10, "gated", &url, FORMATS_CHAT),
                (
                    PROVIDER_CHAT,
                    0,
                    "fallback",
                    &fallback.base_url,
                    FORMATS_CHAT,
                ),
            ]);
            config.same_upstream_retry_count = 0;
            let dir = next_test_data_dir("client_cancel");
            let (state, pool) = build_test_state_handle_with_sqlite_log(config, dir.clone()).await;
            let (addr, proxy_task) = start_proxy(state.clone()).await;
            let mut client = open_client(addr, streaming).await;
            tokio::time::timeout(Duration::from_secs(3), seen)
                .await
                .unwrap()
                .unwrap();
            if streaming {
                read_until(&mut client, "cancel-me").await;
            }
            drop(client);
            tokio::time::timeout(Duration::from_secs(3), closed)
                .await
                .expect("upstream request canceled")
                .unwrap();
            wait_for_request_log_count(&pool, 1).await;
            let rows = sqlx::query("SELECT status, response_error, input_tokens FROM request_logs")
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get::<i64, _>("status"), 499);
            assert_eq!(
                rows[0].get::<String, _>("response_error"),
                crate::proxy::client_lifecycle::CLIENT_CANCELED_ERROR
            );
            if streaming {
                assert_eq!(rows[0].get::<Option<i64>, _>("input_tokens"), Some(7));
            }
            assert!(
                fallback.requests().is_empty(),
                "cancel must not start fallback"
            );
            let current = state.read().await;
            let items: Vec<_> = current.config.upstreams[PROVIDER_CHAT]
                .groups
                .iter()
                .flat_map(|group| group.items.iter().cloned())
                .collect();
            assert_eq!(
                current.upstream_selector.order_group_scoped(
                    UpstreamOrderStrategy::FillFirst,
                    PROVIDER_CHAT,
                    &items,
                    0,
                    &crate::proxy::cooldown_scope::CooldownScope::Global,
                ),
                vec![0, 1],
                "client cancellation must not cool down the upstream"
            );
            drop(current);
            proxy_task.abort();
            upstream_task.abort();
            fallback.abort();
            pool.close().await;
            let _ = std::fs::remove_dir_all(dir);
        }
    });
}

#[test]
fn client_cancellation_preserves_prior_http_and_transport_failures() {
    run_async(async {
        for (transport, streaming) in [(false, false), (false, true), (true, false), (true, true)] {
            let http_failure = spawn_mock_upstream(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":{"message":"upstream failed"}}),
            )
            .await;
            let transport_failure = spawn_disconnect_before_headers_upstream().await;
            let primary = if transport {
                &transport_failure.base_url
            } else {
                &http_failure.base_url
            };
            let (url, seen, closed, upstream_task) = gated_upstream(streaming).await;
            let mut config = config_with_runtime_upstreams(&[
                (PROVIDER_CHAT, 10, "failed", primary, FORMATS_CHAT),
                (PROVIDER_CHAT, 0, "gated", &url, FORMATS_CHAT),
            ]);
            config.same_upstream_retry_count = 0;
            let dir = next_test_data_dir("client_cancel_error");
            let (state, pool) = build_test_state_handle_with_sqlite_log(config, dir.clone()).await;
            let (addr, proxy_task) = start_proxy(state).await;
            let mut client = open_client(addr, streaming).await;
            tokio::time::timeout(Duration::from_secs(5), seen)
                .await
                .unwrap()
                .unwrap();
            if streaming {
                read_until(&mut client, "cancel-me").await;
            }
            drop(client);
            tokio::time::timeout(Duration::from_secs(3), closed)
                .await
                .unwrap()
                .unwrap();
            let failure_count = if transport {
                transport_failure.requests().len()
            } else {
                1
            };
            let expected_logs = failure_count + usize::from(streaming);
            wait_for_request_log_count(&pool, expected_logs as i64).await;
            let rows = sqlx::query("SELECT status, upstream_id, is_billable, input_tokens, cost_nano_usd FROM request_logs")
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(rows.len(), expected_logs);
            let failures: Vec<_> = rows
                .iter()
                .filter(|row| row.get::<String, _>("upstream_id") == "failed")
                .collect();
            assert_eq!(failures.len(), failure_count);
            for failure in &failures {
                assert_eq!(
                    failure.get::<i64, _>("status"),
                    if transport { 502 } else { 503 }
                );
            }
            assert_eq!(
                rows.iter()
                    .filter(|row| row.get::<i64, _>("is_billable") == 1)
                    .count(),
                1
            );
            if streaming {
                let canceled = rows
                    .iter()
                    .find(|row| row.get::<String, _>("upstream_id") == "gated")
                    .expect("canceled stream log");
                assert_eq!(canceled.get::<i64, _>("status"), 499);
                assert_eq!(canceled.get::<i64, _>("is_billable"), 1);
                assert_eq!(canceled.get::<Option<i64>, _>("input_tokens"), Some(7));
                assert!(canceled
                    .get::<Option<i64>, _>("cost_nano_usd")
                    .is_some_and(|cost| cost > 0));
                assert!(failures
                    .iter()
                    .all(|row| row.get::<i64, _>("is_billable") == 0));
            }
            proxy_task.abort();
            upstream_task.abort();
            http_failure.abort();
            transport_failure.abort();
            pool.close().await;
            let _ = std::fs::remove_dir_all(dir);
        }
    });
}

#[test]
fn completed_race_and_hedge_requests_do_not_log_client_cancellation() {
    run_async(async {
        for dispatch in [
            UpstreamDispatchRuntime::Race { max_parallel: 2 },
            UpstreamDispatchRuntime::Hedged {
                delay: Duration::from_millis(10),
                max_parallel: 2,
            },
        ] {
            let (slow_url, seen, closed, slow_task) = gated_upstream(false).await;
            let fast = spawn_mock_upstream_with_delay(StatusCode::OK, json!({
                "id":"winner", "object":"chat.completion", "model":"gpt-5.6-sol",
                "choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}
            }), 80).await;
            let mut config = config_with_runtime_upstreams(&[
                (PROVIDER_CHAT, 10, "slow", &slow_url, FORMATS_CHAT),
                (PROVIDER_CHAT, 10, "fast", &fast.base_url, FORMATS_CHAT),
            ]);
            config.upstream_strategy = UpstreamStrategyRuntime {
                order: UpstreamOrderStrategy::FillFirst,
                dispatch,
            };
            let dir = next_test_data_dir("client_cancel_race");
            let (state, pool) = build_test_state_handle_with_sqlite_log(config, dir.clone()).await;
            let (addr, proxy_task) = start_proxy(state).await;
            let response = reqwest::Client::new()
                .post(format!("http://{addr}/v1/chat/completions"))
                .json(&json!({"model":"gpt-5.6-sol","messages":[{"role":"user","content":"hi"}]}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.json::<Value>().await.unwrap()["choices"][0]["message"]["content"],
                "done"
            );
            tokio::time::timeout(Duration::from_secs(3), seen)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(3), closed)
                .await
                .expect("loser canceled")
                .unwrap();
            wait_for_request_log_count(&pool, 1).await;
            let statuses: Vec<i64> = sqlx::query_scalar("SELECT status FROM request_logs")
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(statuses, vec![200]);
            proxy_task.abort();
            slow_task.abort();
            fast.abort();
            pool.close().await;
            let _ = std::fs::remove_dir_all(dir);
        }
    });
}

#[test]
fn forwarded_service_tier_is_logged_without_request_detail_capture() {
    run_async(async {
        let upstream = spawn_mock_upstream(StatusCode::OK, json!({
            "id":"chat-1","object":"chat.completion","model":"gpt-5.6-terra",
            "choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}
        })).await;
        let config = config_with_runtime_upstreams(&[(
            PROVIDER_CHAT,
            0,
            "fast",
            &upstream.base_url,
            FORMATS_CHAT,
        )]);
        let dir = next_test_data_dir("tier_without_capture");
        let (state, pool) = build_test_state_handle_with_sqlite_log(config, dir.clone()).await;
        assert!(!state.read().await.request_detail.should_capture());
        let (addr, proxy_task) = start_proxy(state).await;
        let response = reqwest::Client::new().post(format!("http://{addr}/v1/chat/completions"))
            .json(&json!({"model":"gpt-5.6-terra","service_tier":"fast","messages":[{"role":"user","content":"hi"}]}))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.bytes().await.unwrap();
        wait_for_request_log_count(&pool, 1).await;
        let row = sqlx::query("SELECT service_tier, request_body, cost_nano_usd FROM request_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<Option<String>, _>("request_body"), None);
        assert_eq!(row.get::<String, _>("service_tier"), "fast");
        let settings = token_proxy_storage::pricing::default_model_pricing_settings();
        let expected = token_proxy_storage::pricing::calculate_request_cost(
            &settings,
            Some("gpt-5.6-terra"),
            None,
            Some("priority"),
            &token_proxy_storage::pricing::BillableUsage {
                uncached_input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            row.get::<i64, _>("cost_nano_usd"),
            expected.cost_nano_usd as i64
        );
        proxy_task.abort();
        upstream.abort();
        pool.close().await;
        let _ = std::fs::remove_dir_all(dir);
    });
}
