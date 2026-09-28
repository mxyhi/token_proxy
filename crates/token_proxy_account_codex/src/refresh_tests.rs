#[test]
fn reimport_new_oauth_credentials_recovers_invalid_account_without_changing_settings() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let id = "recover-invalid";
        let mut invalid = oauth_test_record(
            "old-access",
            "old-refresh",
            String::new(),
            false,
            CodexAccountStatus::Invalid,
            "account",
            "test@example.com",
            future_rfc3339(48),
        );
        invalid.quota_threshold_percent = Some(82.0);
        store.save_record(id.into(), invalid).await.unwrap();
        store.token_refresh_cooldowns.lock().await.insert(
            id.into(),
            TokenRefreshCooldown {
                retry_at: tokio::time::Instant::now() + std::time::Duration::from_secs(300),
                invalid_grant_count: 0,
                error: Some("old credential failure".into()),
            },
        );
        for (access, refresh, expected) in [
            ("old-access", "old-refresh", CodexAccountStatus::Invalid),
            ("new-access", "new-refresh", CodexAccountStatus::Active),
        ] {
            let imported = store
                .import_text(
                    &json!({
                        "access_token": access, "refresh_token": refresh,
                        "id_token": build_id_token("test@example.com", "account"),
                        "expires_at": future_rfc3339(48),
                    })
                    .to_string(),
                )
                .await
                .unwrap();
            assert_eq!(imported[0].account_id, id);
            let record = store.load_account(id).await.unwrap();
            assert_eq!(record.status, expected);
            assert_eq!(record.auto_refresh_enabled(), Some(false));
            assert_eq!(record.quota_threshold_percent, Some(82.0));
            assert_eq!(
                store.token_refresh_cooldown(id).await.is_some(),
                expected == CodexAccountStatus::Invalid
            );
        }
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn quota_completion_preserves_concurrently_rotated_credentials_and_settings() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let store = Arc::new(store);
        let account_id = "quota-concurrent-refresh";
        store
            .save_record(
                account_id.into(),
                oauth_test_record(
                    "old-access",
                    "old-refresh",
                    String::new(),
                    true,
                    CodexAccountStatus::Active,
                    "account",
                    "test@example.com",
                    future_rfc3339(48),
                ),
            )
            .await
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route(
            "/usage",
            axum::routing::get({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        axum::Json(json!({"plan_type":"pro", "rate_limit":{}}))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let usage_url = format!("http://{}/usage", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let quota_task = tokio::spawn({
            let store = store.clone();
            async move {
                crate::quota::refresh_quota_cache_with_usage_endpoint(
                    &store, account_id, &usage_url,
                )
                .await
                .unwrap()
            }
        });
        entered.notified().await;
        let (token_url, token_server) = spawn_token_endpoint("new-access").await;
        store.set_test_token_url(&token_url).await;
        store.refresh_account(account_id).await.unwrap();
        store.set_auto_refresh(account_id, false).await.unwrap();
        store.mark_invalid(account_id).await.unwrap();
        release.notify_one();
        quota_task.await.unwrap();
        let record = store.load_account(account_id).await.unwrap();
        server.abort();
        token_server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
        assert_eq!(record.oauth().unwrap().access_token, "new-access");
        assert_eq!(record.oauth().unwrap().refresh_token, "refreshed-token");
        assert!(!record.oauth().unwrap().auto_refresh_enabled);
        assert_eq!(record.status, CodexAccountStatus::Invalid);
        assert_eq!(record.quota.plan_type.as_deref(), Some("pro"));
    });
}

#[test]
fn concurrent_unauthorized_responses_exchange_one_refresh_token() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let store = Arc::new(store);
        let id = "concurrent-401";
        store
            .save_record(
                id.into(),
                oauth_test_record(
                    "old-access",
                    "old-refresh",
                    String::new(),
                    true,
                    CodexAccountStatus::Active,
                    "account",
                    "test@example.com",
                    future_rfc3339(48),
                ),
            )
            .await
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new().route("/token", axum::routing::post({
            let count = count.clone();
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                async { axum::Json(json!({"access_token":"new-access","refresh_token":"new-refresh","id_token":"","expires_in":172800})) }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        store
            .set_test_token_url(&format!("http://{}/token", listener.local_addr().unwrap()))
            .await;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut requests = Vec::new();
        for _ in 0..8 {
            let store = store.clone();
            requests.push(tokio::spawn(async move {
                store
                    .refresh_account_after_unauthorized(id, "old-access")
                    .await
            }));
        }
        for request in requests {
            request.await.unwrap().unwrap();
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            store
                .load_account(id)
                .await
                .unwrap()
                .oauth()
                .unwrap()
                .refresh_token,
            "new-refresh"
        );
        server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn late_token_exchange_result_cannot_replace_or_disable_reimported_credentials() {
    run_async(async {
        for success in [true, false] {
            let (store, data_dir) = create_test_store();
            let store = Arc::new(store);
            let id = "reimport-during-refresh";
            store
                .save_record(
                    id.into(),
                    oauth_test_record(
                        "old-access",
                        "old-refresh",
                        String::new(),
                        true,
                        CodexAccountStatus::Active,
                        "account",
                        "test@example.com",
                        future_rfc3339(48),
                    ),
                )
                .await
                .unwrap();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let app = axum::Router::new().route("/token", axum::routing::post({
                let entered = entered.clone(); let release = release.clone();
                move || {
                    let entered = entered.clone(); let release = release.clone();
                    async move {
                        entered.notify_one(); release.notified().await;
                        if success {
                            (axum::http::StatusCode::OK, axum::Json(json!({"access_token":"late-access","refresh_token":"late-refresh","id_token":"","expires_in":172800})))
                        } else {
                            (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({"error":{"code":"refresh_token_invalidated"}})))
                        }
                    }
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            store
                .set_test_token_url(&format!("http://{}/token", listener.local_addr().unwrap()))
                .await;
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let request = tokio::spawn({
                let store = store.clone();
                async move { store.refresh_account(id).await }
            });
            entered.notified().await;
            let imported = store.import_text(&json!({"access_token":"imported-access","refresh_token":"imported-refresh",
                "id_token":build_id_token("test@example.com", "account"),"expires_at":future_rfc3339(48)}).to_string()).await.unwrap();
            assert_eq!(imported[0].account_id, id);
            release.notify_one();
            request.await.unwrap().unwrap();
            let current = store.load_account(id).await.unwrap();
            assert_eq!(current.oauth().unwrap().access_token, "imported-access");
            assert_eq!(current.oauth().unwrap().refresh_token, "imported-refresh");
            assert_eq!(current.status, CodexAccountStatus::Active);
            server.abort();
            let _ = std::fs::remove_dir_all(data_dir);
        }
    });
}

#[test]
fn late_quota_unauthorized_cannot_disable_rotated_credentials() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let store = Arc::new(store);
        let id = "late-quota-401";
        store
            .save_record(
                id.into(),
                oauth_test_record(
                    "old-access",
                    "old-refresh",
                    String::new(),
                    true,
                    CodexAccountStatus::Active,
                    "account",
                    "test@example.com",
                    future_rfc3339(48),
                ),
            )
            .await
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route(
            "/usage",
            axum::routing::get({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        (
                            axum::http::StatusCode::UNAUTHORIZED,
                            axum::Json(json!({"error":{"code":"token_revoked"}})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        store
            .set_test_usage_url(&format!("http://{}/usage", listener.local_addr().unwrap()))
            .await;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let request = tokio::spawn({
            let store = store.clone();
            async move { store.refresh_quota_cache_now(id).await }
        });
        entered.notified().await;
        let (token_url, token_server) = spawn_token_endpoint("new-access").await;
        store.set_test_token_url(&token_url).await;
        store.refresh_account(id).await.unwrap();
        release.notify_one();
        assert!(request.await.unwrap().is_err());
        let current = store.load_account(id).await.unwrap();
        assert_eq!(current.status, CodexAccountStatus::Active);
        assert_eq!(current.oauth().unwrap().access_token, "new-access");
        server.abort();
        token_server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn manual_quota_is_single_flight_cancellation_safe_and_never_exchanges_oauth() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let store = Arc::new(store);
        let account_id = "manual-quota";
        store
            .save_record(
                account_id.into(),
                oauth_test_record(
                    "access",
                    "refresh",
                    String::new(),
                    true,
                    CodexAccountStatus::Active,
                    "account",
                    "test@example.com",
                    future_rfc3339(1),
                ),
            )
            .await
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let token_count = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new()
            .route(
                "/usage",
                axum::routing::get({
                    let count = count.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    move || {
                        let count = count.clone();
                        let entered = entered.clone();
                        let release = release.clone();
                        async move {
                            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                                entered.notify_one();
                                release.notified().await;
                            }
                            axum::Json(json!({"plan_type":"pro", "rate_limit":{}}))
                        }
                    }
                }),
            )
            .route(
                "/token",
                axum::routing::post({
                    let token_count = token_count.clone();
                    move || {
                        token_count.fetch_add(1, Ordering::SeqCst);
                        async { axum::http::StatusCode::UNAUTHORIZED }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        store
            .set_test_usage_url(&format!("http://{addr}/usage"))
            .await;
        store
            .set_test_token_url(&format!("http://{addr}/token"))
            .await;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let first = tokio::spawn({
            let store = store.clone();
            async move { store.refresh_quota_cache_now(account_id).await }
        });
        entered.notified().await;
        store.refresh_quota_cache_now(account_id).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        first.abort();
        let _ = first.await;
        release.notify_one();
        store.refresh_quota_cache_now(account_id).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(token_count.load(Ordering::SeqCst), 0);
        store.mark_invalid(account_id).await.unwrap();
        assert!(store.refresh_quota_cache_now(account_id).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 2);
        server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn scheduled_refresh_filters_channels_and_cools_down_short_lived_success() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        for id in ["enabled", "disabled"] {
            store
                .save_record(
                    id.into(),
                    oauth_test_record(
                        "old-access",
                        "refresh",
                        String::new(),
                        true,
                        CodexAccountStatus::Expired,
                        "account",
                        "test@example.com",
                        past_rfc3339(1),
                    ),
                )
                .await
                .unwrap();
        }
        let count = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new().route("/token", axum::routing::post({
            let count = count.clone();
            move || {
                let number = count.fetch_add(1, Ordering::SeqCst);
                async move { axum::Json(json!({"access_token":format!("access-{number}"),"refresh_token":"rotated","id_token":"","expires_in":3600})) }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        store
            .set_test_token_url(&format!("http://{}/token", listener.local_addr().unwrap()))
            .await;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let ids = HashSet::from(["enabled".to_string()]);
        for _ in 0..3 {
            store.refresh_due_accounts_for_ids(&ids).await.unwrap();
        }
        let enabled = store.get_account_record("enabled").await.unwrap();
        assert_eq!(enabled.status, CodexAccountStatus::Active);
        assert_eq!(enabled.oauth().unwrap().access_token, "access-0");
        assert_eq!(
            store
                .load_account("disabled")
                .await
                .unwrap()
                .oauth()
                .unwrap()
                .access_token,
            "old-access"
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        store
            .refresh_account_after_unauthorized("enabled", "old-access")
            .await
            .unwrap();
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "late 401 reuses rotated token"
        );
        server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn token_exchange_retries_transient_failure_three_times_and_stops_revoked_sessions() {
    run_async(async {
        for (status, code, expected_requests, expected_status) in [
            (
                503,
                "temporarily_unavailable",
                3,
                CodexAccountStatus::Active,
            ),
            (
                401,
                "refresh_token_invalidated",
                1,
                CodexAccountStatus::Invalid,
            ),
            (401, "refresh_token_expired", 1, CodexAccountStatus::Invalid),
            (401, "refresh_token_reused", 1, CodexAccountStatus::Invalid),
            (401, "refresh_token_revoked", 1, CodexAccountStatus::Invalid),
        ] {
            let (store, data_dir) = create_test_store();
            store
                .save_record(
                    "refresh-errors".into(),
                    oauth_test_record(
                        "live-access",
                        "refresh",
                        String::new(),
                        true,
                        CodexAccountStatus::Active,
                        "account",
                        "test@example.com",
                        future_rfc3339(48),
                    ),
                )
                .await
                .unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let app = axum::Router::new().route(
                "/token",
                axum::routing::post({
                    let count = count.clone();
                    move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        async move {
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                axum::Json(json!({"error":{"code":code}})),
                            )
                        }
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            store
                .set_test_token_url(&format!("http://{}/token", listener.local_addr().unwrap()))
                .await;
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            assert!(store.refresh_account("refresh-errors").await.is_err());
            assert!(store.refresh_account("refresh-errors").await.is_err());
            assert_eq!(count.load(Ordering::SeqCst), expected_requests, "{code}");
            assert_eq!(
                store.load_account("refresh-errors").await.unwrap().status,
                expected_status,
                "{code}"
            );
            server.abort();
            let _ = std::fs::remove_dir_all(data_dir);
        }
    });
}

#[test]
fn invalid_grant_preserves_live_access_token_and_cools_down_refresh() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        store
            .save_record(
                "invalid-grant".into(),
                oauth_test_record(
                    "live-access",
                    "refresh",
                    String::new(),
                    true,
                    CodexAccountStatus::Active,
                    "account",
                    "test@example.com",
                    future_rfc3339(23),
                ),
            )
            .await
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post({
                let count = count.clone();
                move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    async {
                        (
                            axum::http::StatusCode::BAD_REQUEST,
                            axum::Json(json!({"error":"invalid_grant"})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        store
            .set_test_token_url(&format!("http://{}/token", listener.local_addr().unwrap()))
            .await;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        for _ in 0..3 {
            let record = store
                .get_account_record("invalid-grant")
                .await
                .expect("live access token remains usable");
            assert_eq!(record.oauth().unwrap().access_token, "live-access");
            assert_eq!(record.status, CodexAccountStatus::Active);
        }
        server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn account_reads_refresh_only_within_one_day_and_ignore_plan_claim_mismatch() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let (token_url, server) = spawn_token_endpoint("new-access").await;
        store.set_test_token_url(&token_url).await;
        for (id, hours, expected) in [("due", 23, "new-access"), ("not-due", 25, "old-access")] {
            let mut record = oauth_test_record(
                "old-access",
                "refresh",
                String::new(),
                true,
                CodexAccountStatus::Active,
                "account",
                "test@example.com",
                future_rfc3339(hours),
            );
            if id == "not-due" {
                *record.oauth_mut().unwrap().access_token = build_access_token_with_plan("free");
                record.quota.plan_type = Some("pro".into());
            }
            let original = record.oauth().unwrap().access_token.to_string();
            store.save_record(id.into(), record).await.unwrap();
            let loaded = store.get_account_record(id).await.unwrap();
            assert_eq!(
                loaded.oauth().unwrap().access_token,
                if id == "not-due" { &original } else { expected }
            );
        }
        server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
    });
}

#[test]
fn token_completion_preserves_settings_and_invalid_state_changed_during_exchange() {
    run_async(async {
        let (store, data_dir) = create_test_store();
        let store = Arc::new(store);
        let account_id = "token-concurrent-settings";
        store
            .save_record(
                account_id.into(),
                oauth_test_record(
                    "old-access",
                    "old-refresh",
                    String::new(),
                    true,
                    CodexAccountStatus::Active,
                    "account",
                    "test@example.com",
                    future_rfc3339(48),
                ),
            )
            .await
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let app = axum::Router::new().route("/token", axum::routing::post({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    axum::Json(json!({"access_token":"new-access", "refresh_token":"new-refresh", "id_token":"", "expires_in":172800}))
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        store
            .set_test_token_url(&format!("http://{}/token", listener.local_addr().unwrap()))
            .await;
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let refresh_task = tokio::spawn({
            let store = store.clone();
            async move { store.refresh_account(account_id).await }
        });
        entered.notified().await;
        store.set_auto_refresh(account_id, false).await.unwrap();
        store
            .set_quota_threshold(account_id, Some(88.0))
            .await
            .unwrap();
        store.mark_invalid(account_id).await.unwrap();
        release.notify_one();
        let _ = refresh_task.await.unwrap();
        let record = store.load_account(account_id).await.unwrap();
        server.abort();
        let _ = std::fs::remove_dir_all(data_dir);
        assert_eq!(record.oauth().unwrap().access_token, "new-access");
        assert!(!record.oauth().unwrap().auto_refresh_enabled);
        assert_eq!(record.quota_threshold_percent, Some(88.0));
        assert_eq!(record.status, CodexAccountStatus::Invalid);
    });
}
