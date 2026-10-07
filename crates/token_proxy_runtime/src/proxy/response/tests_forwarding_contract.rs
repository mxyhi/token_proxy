use super::*;

#[test]
fn native_responses_eof_must_fail_and_log() {
    run_async(async {
        for provider in ["openai-response", "codex"] {
            let (log, mut context, pool) = setup_responses_stream().await;
            context.provider = provider.to_string();
            let tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let chunk = Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            );
            let upstream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(chunk)]);
            let items: Vec<_> =
                super::super::streaming::stream_with_logging(upstream, context, log, tracker)
                    .collect()
                    .await;
            let body: String = items
                .iter()
                .filter_map(|v| v.as_ref().ok())
                .map(|v| String::from_utf8_lossy(v).to_string())
                .collect();
            wait_for_log_rows(&pool, 1).await;
            let row = sqlx::query("SELECT status,response_error FROM request_logs")
                .fetch_one(&pool)
                .await
                .unwrap();
            let status: i64 = row.get("status");
            let error: Option<String> = row.get("response_error");
            println!("provider={provider}; body={body:?}; status={status}; error={error:?}");
            assert!(
                items.iter().any(Result::is_err) || body.contains("response.failed"),
                "premature EOF must be reported as failure"
            );
            assert_eq!(status, 502);
            assert!(error.is_some());
        }
    });
}

#[test]
fn native_stream_rewrite_must_remove_content_length() {
    run_async(async {
        use axum::{body::Body, routing::get, Router};
        let upstream_body = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"output\":[]}}\n\n";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/",
            get(move || async move {
                axum::http::Response::builder()
                    .header("content-type", "text/event-stream")
                    .header("content-length", upstream_body.len())
                    .body(Body::from(upstream_body))
                    .unwrap()
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let upstream = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap();
        let (log, _, _) = setup_responses_stream().await;
        let tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let meta = super::super::RequestMeta {
            client_ip: None,
            stream: true,
            original_model: None,
            mapped_model: None,
            reasoning_effort: None,
            response_format: None,
            estimated_input_tokens: None,
            client_request_body: None,
            billing: Default::default(),
        };
        let response = super::super::build_proxy_response(
            &meta,
            "openai-response",
            "audit",
            None,
            "/v1/responses",
            upstream,
            log,
            tracker,
            Instant::now(),
            Default::default(),
            "http://127.0.0.1",
            None,
            crate::proxy::openai_compat::FormatTransform::None,
            None,
            None,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await;
        assert!(!response.headers().contains_key("content-length"));
        // 将实际 proxy Response 通过 hyper 写入 socket，验证 framing 不会截掉补出的终态。
        let downstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let downstream_addr = downstream_listener.local_addr().unwrap();
        let response = Arc::new(std::sync::Mutex::new(Some(response)));
        let downstream_app = Router::new().route(
            "/",
            get(move || {
                let response = response.lock().unwrap().take().unwrap();
                async move { response }
            }),
        );
        let downstream_server = tokio::spawn(async move {
            axum::serve(downstream_listener, downstream_app)
                .await
                .unwrap()
        });
        let received = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{downstream_addr}/"))
            .send()
            .await
            .unwrap();
        assert!(!received.headers().contains_key("content-length"));
        let body = received
            .bytes()
            .await
            .expect("socket stream must have valid HTTP framing");
        server.abort();
        downstream_server.abort();
        let expected = format!("{upstream_body}data: [DONE]\n\n");
        assert_eq!(body.as_ref(), expected.as_bytes());
    });
}

#[test]
fn response_detail_capture_only_allocates_when_enabled_and_never_truncates() {
    run_async(async {
        for enabled in [false, true] {
            let (log, mut context, pool) = setup_responses_stream().await;
            if enabled {
                context.request_headers = Some("{}".into());
            }
            let chunk = vec![b'x'; 1024 * 1024];
            let mut capture = super::super::body_capture::ResponseBodyCapture::new(&context);
            for _ in 0..10 {
                capture.push(&chunk).await;
            }
            capture.push(b"final-marker").await;
            let entry = crate::proxy::log::build_log_entry(
                &context,
                crate::proxy::log::UsageSnapshot::default(),
                None,
            );
            capture.write_log(log, entry);
            wait_for_log_rows(&pool, 1).await;
            let detail = token_proxy_storage::logs::read_request_log_detail(&pool, 1)
                .await
                .unwrap();
            if !enabled {
                assert!(detail.response_body.is_none());
                assert_eq!(detail.response_body_bytes, 0);
                continue;
            }
            assert_eq!(detail.response_body_bytes, 10 * 1024 * 1024 + 12);
            assert!(detail.response_capture_error.is_none());
            let mut offset = 0;
            loop {
                let page = token_proxy_storage::logs::read_request_log_body_page(&pool, 1, offset)
                    .await
                    .unwrap();
                assert!(page.text.len() <= 64 * 1024);
                if let Some(next) = page.next_offset {
                    assert!(page.text.bytes().all(|b| b == b'x'));
                    offset = next;
                } else {
                    assert!(page.text.ends_with("final-marker"));
                    assert_eq!(offset + page.text.len() as u64, detail.response_body_bytes);
                    break;
                }
            }
        }
    });
}

#[test]
fn native_non_sse_diagnostic_is_bounded_and_reports_truncation() {
    run_async(async {
        let (log, context, pool) = setup_responses_stream().await;
        let tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunk = Bytes::from(vec![b'x'; 96 * 1024]);
        let upstream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(chunk)]);
        let body: String =
            super::super::streaming::stream_with_logging(upstream, context, log, tracker)
                .map(|item| String::from_utf8_lossy(&item.unwrap()).to_string())
                .collect::<Vec<_>>()
                .await
                .concat();
        assert!(body.contains("response.failed"));
        assert!(body.contains("diagnostic truncated to 65536 bytes"));
        assert!(body.len() < 70 * 1024);
        wait_for_log_rows(&pool, 1).await;
        let row = sqlx::query("SELECT status,response_error,response_body FROM request_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("status"), 502);
        assert!(row.get::<Option<String>, _>("response_body").is_none());
        assert!(row.get::<String, _>("response_error").contains("non-SSE"));
    });
}

#[test]
fn oversized_native_sse_persists_parser_failure_instead_of_generic_drop() {
    run_async(async {
        let (log, context, pool) = setup_responses_stream().await;
        let tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let chunk = Bytes::from(vec![
            b'x';
            token_proxy_protocol::sse::MAX_SSE_LINE_BYTES + 1
        ]);
        let upstream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(chunk)]);
        let results: Vec<_> =
            super::super::streaming::stream_with_logging(upstream, context, log, tracker)
                .collect()
                .await;
        assert!(results.iter().any(Result::is_err));
        wait_for_log_rows(&pool, 1).await;
        let row = sqlx::query("SELECT status,response_error FROM request_logs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("status"), 502);
        let error: String = row.get("response_error");
        assert!(error.contains("Failed to parse upstream SSE"), "{error}");
        assert!(!error.contains("dropped"), "{error}");
    });
}

#[test]
fn response_detail_switch_controls_persisted_stream_body() {
    run_async(async {
        for enabled in [false, true] {
            let (log, mut context, pool) = setup_responses_stream().await;
            if enabled {
                context.request_headers = Some("{}".to_string());
            }
            let tracker = crate::proxy::token_rate::TokenRateTracker::new()
                .register(None, None)
                .await;
            let delta = format!(
                "data: {}\n\n",
                json!({"type":"response.output_text.delta", "delta":"x".repeat(80 * 1024)})
            );
            let completed = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"output\":[]}}\n\n";
            let expected_log = format!("{delta}{completed}");
            let upstream = futures_util::stream::iter(vec![
                Ok::<_, std::io::Error>(Bytes::from(delta)),
                Ok(Bytes::from_static(completed.as_bytes())),
            ]);
            let results: Vec<_> =
                super::super::streaming::stream_with_logging(upstream, context, log, tracker)
                    .collect()
                    .await;
            assert!(results.iter().all(Result::is_ok));
            wait_for_log_rows(&pool, 1).await;
            let actual =
                sqlx::query_scalar::<_, Option<String>>("SELECT response_body FROM request_logs")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(actual, enabled.then_some(expected_log));
        }
    });
}

#[test]
fn cancelled_stream_flushes_spooled_detail_without_losing_forwarded_prefix() {
    run_async(async {
        let (log, mut context, pool) = setup_responses_stream().await;
        context.request_headers = Some("{}".into());
        let tracker = crate::proxy::token_rate::TokenRateTracker::new()
            .register(None, None)
            .await;
        let delta = Bytes::from(format!(
            "data: {}\n\n",
            json!({"type":"response.output_text.delta", "delta":"x".repeat(80 * 1024)})
        ));
        let upstream = futures_util::stream::iter(
            (0..8)
                .map(|_| Ok::<_, std::io::Error>(delta.clone()))
                .collect::<Vec<_>>(),
        )
        .chain(futures_util::stream::pending());
        let mut stream = Box::pin(super::super::streaming::stream_with_logging(
            upstream, context, log, tracker,
        ));
        for _ in 0..8 {
            assert_eq!(stream.next().await.unwrap().unwrap(), delta);
        }
        drop(stream);
        wait_for_log_rows(&pool, 1).await;
        let detail = token_proxy_storage::logs::read_request_log_detail(&pool, 1)
            .await
            .unwrap();
        assert_eq!(detail.response_body_bytes, (delta.len() * 8) as u64);
        assert!(detail.response_capture_error.is_none());
        assert!(detail
            .response_error
            .as_deref()
            .unwrap()
            .contains("dropped"));
        let mut offset = 0;
        loop {
            let page = token_proxy_storage::logs::read_request_log_body_page(&pool, 1, offset)
                .await
                .unwrap();
            for (index, byte) in page.text.bytes().enumerate() {
                assert_eq!(byte, delta[(offset as usize + index) % delta.len()]);
            }
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
    });
}
