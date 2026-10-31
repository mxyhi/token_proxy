use super::*;

fn encrypted_request() -> Value {
    json!({"model":"gpt-5","stream":true,"input":[{"type":"reasoning","encrypted_content":"cipher-one"},{"type":"message","role":"user","content":"hello"}]})
}
fn encrypted_error() -> Value {
    json!({"error":{"code":"invalid_encrypted_content","message":"The encrypted content could not be verified."}})
}
fn encrypted_sse() -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"type":"response.failed","response":encrypted_error()})
    ))
}
fn encrypted_sse_rejection(id: &str) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({
            "type": "response.failed",
            "response": {
                "error": {
                    "code": "invalid_encrypted_content",
                    "message": format!("The encrypted content for item {id} could not be verified. Reason: Encrypted content could not be decrypted or parsed.")
                }
            }
        })
    ))
}
fn successful_response() -> Bytes {
    Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_ok\",\"object\":\"response\",\"created_at\":123,\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[]}}\n\n")
}
async fn raw_request(state: ProxyStateHandle, request: Value) -> (StatusCode, Bytes) {
    let response = proxy_request(
        State(state),
        Method::POST,
        Uri::from_static(RESPONSES_PATH),
        HeaderMap::new(),
        Body::from(request.to_string()),
    )
    .await;
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
    )
}
#[test]
fn encrypted_recovery_http_and_sse_use_same_sent_credential() {
    run_async(async {
        for mode in 0..3 {
            let sse = mode > 0;
            let upstream = spawn_mock_raw_sequence_upstream(vec![
                if sse {
                    (
                        StatusCode::OK,
                        if mode == 2 {
                            let error = encrypted_sse();
                            error.slice(..error.len() - 2)
                        } else {
                            encrypted_sse()
                        },
                        "text/event-stream",
                    )
                } else {
                    (
                        StatusCode::BAD_REQUEST,
                        Bytes::from(encrypted_error().to_string()),
                        "application/json",
                    )
                },
                (StatusCode::OK, successful_response(), "text/event-stream"),
            ])
            .await;
            let mut config = config_with_runtime_upstreams(&[(
                PROVIDER_RESPONSES,
                10,
                "encrypted-repair",
                upstream.base_url.as_str(),
                FORMATS_RESPONSES,
            )]);
            config.same_upstream_retry_count = 0;
            let dir = next_test_data_dir("encrypted-recovery-sent-token");
            let state = build_test_state_handle(config, dir.clone()).await;
            let (status, _) = raw_request(state, encrypted_request()).await;
            let requests = upstream.requests();
            upstream.abort();
            let _ = std::fs::remove_dir_all(dir);
            assert_eq!(status, StatusCode::OK);
            assert_eq!(requests.len(), 2, "recover exactly once for sse={sse}");
            assert!(requests[0].authorization.is_some());
            assert_eq!(requests[0].authorization, requests[1].authorization);
            assert_eq!(requests[1].body["input"].as_array().unwrap().len(), 1);
            assert_eq!(requests[1].body["input"][0]["type"], "message");
        }
    });
}

#[test]
fn encrypted_recovery_accepts_api_key_header_without_authorization() {
    run_async(async {
        for sse in [false, true] {
            let upstream = spawn_mock_raw_sequence_upstream(vec![
                if sse {
                    (StatusCode::OK, encrypted_sse(), "text/event-stream")
                } else {
                    (
                        StatusCode::BAD_REQUEST,
                        Bytes::from(encrypted_error().to_string()),
                        "application/json",
                    )
                },
                (StatusCode::OK, successful_response(), "text/event-stream"),
            ])
            .await;
            let mut config = config_with_runtime_upstreams(&[(
                PROVIDER_RESPONSES,
                10,
                "encrypted-api-key-repair",
                upstream.base_url.as_str(),
                FORMATS_RESPONSES,
            )]);
            config.same_upstream_retry_count = 0;
            config
                .upstreams
                .get_mut(PROVIDER_RESPONSES)
                .expect("responses provider")
                .groups[0]
                .items[0]
                .header_overrides = Some(vec![
                token_proxy_config::HeaderOverride {
                    name: axum::http::header::AUTHORIZATION,
                    value: None,
                },
                token_proxy_config::HeaderOverride {
                    name: axum::http::HeaderName::from_static("api-key"),
                    value: Some(axum::http::HeaderValue::from_static("test-key")),
                },
            ]);
            let dir = next_test_data_dir("encrypted-recovery-api-key-header");
            let state = build_test_state_handle(config, dir.clone()).await;
            let (status, _) = raw_request(state, encrypted_request()).await;
            let requests = upstream.requests();
            upstream.abort();
            let _ = std::fs::remove_dir_all(dir);

            assert_eq!(status, StatusCode::OK, "recovery should work for sse={sse}");
            assert_eq!(requests.len(), 2, "recover exactly once for sse={sse}");
            assert!(requests
                .iter()
                .all(|request| request.authorization.is_none()));
            assert_eq!(requests[1].body["input"].as_array().unwrap().len(), 1);
        }
    });
}

#[test]
fn encrypted_recovery_http_then_sse_does_not_get_second_budget_or_failover() {
    run_async(async {
        let primary = spawn_mock_raw_sequence_upstream(vec![
            (
                StatusCode::BAD_REQUEST,
                Bytes::from(encrypted_error().to_string()),
                "application/json",
            ),
            (StatusCode::OK, encrypted_sse(), "text/event-stream"),
        ])
        .await;
        let secondary = spawn_mock_upstream(StatusCode::OK, json!({"output":[]})).await;
        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_RESPONSES,
                10,
                "secondary",
                secondary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.same_upstream_retry_count = 3;
        config.upstream_strategy = UpstreamStrategyRuntime {
            order: UpstreamOrderStrategy::FillFirst,
            dispatch: UpstreamDispatchRuntime::Serial,
        };
        let dir = next_test_data_dir("encrypted-recovery-shared-budget");
        let state = build_test_state_handle(config, dir.clone()).await;
        let _ = raw_request(state, encrypted_request()).await;
        assert_eq!(primary.requests().len(), 2);
        assert!(secondary.requests().is_empty());
        primary.abort();
        secondary.abort();
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn encrypted_recovery_handles_more_than_64_rejected_reasoning_items() {
    run_async(async {
        const ITEM_COUNT: usize = 65;
        let rejection = |index: usize| {
            Bytes::from(
                json!({
                    "error": {
                        "code": "invalid_encrypted_content",
                        "message": format!(
                            "The encrypted content for item rs_{index} could not be verified. Reason: Encrypted content could not be decrypted or parsed."
                        )
                    }
                })
                .to_string(),
            )
        };
        let mut responses = (0..ITEM_COUNT)
            .map(|index| {
                (
                    StatusCode::BAD_REQUEST,
                    rejection(index),
                    "application/json",
                )
            })
            .collect::<Vec<_>>();
        responses.push((StatusCode::OK, successful_response(), "text/event-stream"));
        let upstream = spawn_mock_raw_sequence_upstream(responses).await;
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            10,
            "encrypted-long-history",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        config.same_upstream_retry_count = 0;
        let dir = next_test_data_dir("encrypted-recovery-long-history");
        let state = build_test_state_handle(config, dir.clone()).await;
        let input = (0..ITEM_COUNT)
            .map(|index| {
                json!({
                    "type": "reasoning",
                    "id": format!("rs_{index}"),
                    "encrypted_content": format!("cipher-{index}"),
                    "summary": []
                })
            })
            .chain(std::iter::once(json!({
                "type": "message",
                "role": "user",
                "content": "hello"
            })))
            .collect::<Vec<_>>();
        let (status, _) = raw_request(
            state,
            json!({"model":"gpt-6-astra","stream":true,"input":input}),
        )
        .await;
        let requests = upstream.requests();
        upstream.abort();
        let _ = std::fs::remove_dir_all(dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(requests.len(), ITEM_COUNT + 1);
        assert_eq!(
            requests.last().unwrap().body["input"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn session_policy_http_and_sse_do_not_retry_or_cool_down() {
    run_async(async {
        for sse in [false, true] {
            let error = json!({"error":{"code":"session_blocked_by_cyber_policy","message":"session blocked"}});
            let primary = spawn_mock_raw_sequence_upstream(vec![if sse {
                (
                    StatusCode::OK,
                    Bytes::from(format!(
                        "data: {}\n\n",
                        json!({"type":"response.failed","response":error})
                    )),
                    "text/event-stream",
                )
            } else {
                (
                    StatusCode::FORBIDDEN,
                    Bytes::from(error.to_string()),
                    "application/json",
                )
            }])
            .await;
            let secondary = spawn_mock_upstream(StatusCode::OK, json!({"output":[]})).await;
            let mut config = config_with_runtime_upstreams(&[
                (
                    PROVIDER_RESPONSES,
                    10,
                    "primary",
                    primary.base_url.as_str(),
                    FORMATS_RESPONSES,
                ),
                (
                    PROVIDER_RESPONSES,
                    10,
                    "secondary",
                    secondary.base_url.as_str(),
                    FORMATS_RESPONSES,
                ),
            ]);
            config.same_upstream_retry_count = 3;
            config.upstream_strategy = UpstreamStrategyRuntime {
                order: UpstreamOrderStrategy::FillFirst,
                dispatch: UpstreamDispatchRuntime::Serial,
            };
            let dir = next_test_data_dir("session-policy-no-retry");
            let state = build_test_state_handle(config, dir.clone()).await;
            for _ in 0..2 {
                let _ = raw_request(
                    state.clone(),
                    json!({"model":"gpt-5","input":"hello","stream":true}),
                )
                .await;
            }
            assert_eq!(
                primary.requests().len(),
                2,
                "each new request reaches primary once"
            );
            assert!(secondary.requests().is_empty());
            primary.abort();
            secondary.abort();
            let _ = std::fs::remove_dir_all(dir);
        }
    });
}

#[test]
fn encrypted_recovery_sse_repairs_multiple_rejected_reasoning_items_in_order() {
    run_async(async {
        let upstream = spawn_mock_raw_sequence_upstream(vec![
            (
                StatusCode::OK,
                encrypted_sse_rejection("rs_first"),
                "text/event-stream",
            ),
            (
                StatusCode::OK,
                encrypted_sse_rejection("rs_second"),
                "text/event-stream",
            ),
            (StatusCode::OK, successful_response(), "text/event-stream"),
        ])
        .await;
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            10,
            "encrypted-sse-multi-repair",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        config.same_upstream_retry_count = 3;
        let dir = next_test_data_dir("encrypted-sse-multi-repair");
        let state = build_test_state_handle(config, dir.clone()).await;
        let (status, _) = raw_request(
            state,
            json!({"model":"gpt-6-astra","stream":true,"input":[
                {"type":"reasoning","id":"rs_first","encrypted_content":"first"},
                {"type":"reasoning","id":"rs_second","encrypted_content":"second"},
                {"type":"message","role":"user","content":"hello"}
            ]}),
        )
        .await;
        let requests = upstream.requests();
        upstream.abort();
        let _ = std::fs::remove_dir_all(dir);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].body["input"].as_array().unwrap().len(), 3);
        assert_eq!(requests[1].body["input"].as_array().unwrap().len(), 2);
        assert_eq!(requests[2].body["input"].as_array().unwrap().len(), 1);
        assert_eq!(requests[2].body["input"][0]["type"], "message");
    });
}

#[test]
fn encrypted_recovery_never_replays_after_business_output_in_same_chunk() {
    run_async(async {
        let mut body =
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"already sent\"}\n\n"
                .to_vec();
        body.extend_from_slice(&encrypted_sse());
        let upstream = spawn_mock_raw_sequence_upstream(vec![(
            StatusCode::OK,
            Bytes::from(body),
            "text/event-stream",
        )])
        .await;
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            10,
            "primary",
            upstream.base_url.as_str(),
            FORMATS_RESPONSES,
        )]);
        config.same_upstream_retry_count = 3;
        let dir = next_test_data_dir("encrypted-after-business");
        let state = build_test_state_handle(config, dir.clone()).await;
        let (_, body) = raw_request(state, encrypted_request()).await;
        assert!(String::from_utf8_lossy(&body).contains("already sent"));
        assert_eq!(upstream.requests().len(), 1);
        upstream.abort();
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn encrypted_recovery_stops_before_send_when_account_token_changed() {
    run_async(async {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let app = Router::new().route(
            "/{*path}",
            any({
                let entered = entered.clone();
                let release = release.clone();
                let calls = calls.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        entered.notify_one();
                        release.notified().await;
                        axum::http::Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .header("content-type", "application/json")
                            .body(Body::from(encrypted_error().to_string()))
                            .unwrap()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_CODEX,
            0,
            "encrypted-credential",
            &base_url,
            FORMATS_RESPONSES,
        )]);
        config.same_upstream_retry_count = 0;
        config.upstreams.get_mut(PROVIDER_CODEX).unwrap().groups[0].items[0].codex_account_id =
            Some("account".into());
        let dir = next_test_data_dir("encrypted-credential-fence");
        let state = build_test_state_handle(config, dir.clone()).await;
        let expiry = (OffsetDateTime::now_utc() + TimeDuration::days(2))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        seed_codex_account(&state, "account", "old-token", "chatgpt", &expiry).await;
        let request = tokio::spawn({
            let state = state.clone();
            async move { raw_request(state, encrypted_request()).await }
        });
        entered.notified().await;
        seed_codex_account(&state, "account", "new-token", "chatgpt", &expiry).await;
        release.notify_one();
        let (status, _) = request.await.unwrap();
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!state
            .read()
            .await
            .account_selector
            .is_cooling_down("codex", "account"));
        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    });
}
