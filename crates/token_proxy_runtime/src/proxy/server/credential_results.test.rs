use super::*;

#[test]
fn late_account_429_is_logged_but_cannot_cool_replaced_credentials() {
    run_async(async {
        for replace_credentials in [false, true] {
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let app = Router::new().route(
                "/{*path}",
                any({
                    let entered = entered.clone();
                    let release = release.clone();
                    move || {
                        let entered = entered.clone();
                        let release = release.clone();
                        async move {
                            entered.notify_one();
                            release.notified().await;
                            axum::http::Response::builder()
                                .status(StatusCode::TOO_MANY_REQUESTS)
                                .header("retry-after", "120")
                                .header("content-type", "application/json")
                                .body(Body::from(r#"{"error":{"message":"quota exceeded"}}"#))
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
                "credential-result",
                &base_url,
                FORMATS_RESPONSES,
            )]);
            config.same_upstream_retry_count = 0;
            config.upstreams.get_mut(PROVIDER_CODEX).unwrap().groups[0].items[0].codex_account_id =
                Some("account".into());
            let dir = next_test_data_dir("late_credential_result");
            let (state, pool) = build_test_state_handle_with_sqlite_log(config, dir.clone()).await;
            let expiry = (OffsetDateTime::now_utc() + TimeDuration::days(2))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            seed_codex_account(&state, "account", "old-token", "chatgpt", &expiry).await;
            let request = tokio::spawn({
                let state = state.clone();
                async move {
                    proxy_request(
                        State(state),
                        Method::POST,
                        Uri::from_static(RESPONSES_PATH),
                        HeaderMap::new(),
                        Body::from(r#"{"model":"gpt-5","input":"hello","stream":false}"#),
                    )
                    .await
                }
            });
            entered.notified().await;
            if replace_credentials {
                seed_codex_account(&state, "account", "new-token", "chatgpt", &expiry).await;
            }
            release.notify_one();
            let response = request.await.unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert!(String::from_utf8_lossy(&body).contains("quota exceeded"));
            let current = state.read().await.clone();
            assert_eq!(
                current.account_selector.is_cooling_down("codex", "account"),
                !replace_credentials
            );
            let first = current.config.upstreams[PROVIDER_CODEX].groups[0].items[0].clone();
            let mut standby = first.clone();
            standby.selector_key = "untouched".into();
            let order = current.upstream_selector.order_group_scoped(
                UpstreamOrderStrategy::FillFirst,
                "codex",
                &[first, standby],
                0,
                &crate::proxy::cooldown_scope::CooldownScope::Global,
            );
            assert_eq!(
                order,
                if replace_credentials {
                    vec![0, 1]
                } else {
                    vec![1, 0]
                }
            );
            // 错误仍真实落库，不能用“凭据过时”隐藏发生过的上游失败。
            for _ in 0..100 {
                let count: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM request_logs WHERE status = 429")
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                if count > 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM request_logs WHERE status = 429")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(count > 0);
            server.abort();
            let _ = std::fs::remove_dir_all(dir);
        }
    });
}
