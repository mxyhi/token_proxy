use super::*;
use token_proxy_config::{LocalApiKey, LocalApiKeyScope};

fn scoped_key(id: &str, upstreams: &[&str]) -> LocalApiKey {
    LocalApiKey {
        id: id.into(),
        name: id.into(),
        key: format!("secret-{id}"),
        enabled: true,
        scope: LocalApiKeyScope::Selected {
            upstream_ids: upstreams.iter().map(|id| (*id).into()).collect(),
        },
    }
}

async fn keyed_request(
    state: ProxyStateHandle,
    key: &str,
    path: &str,
    model: Option<&str>,
) -> (StatusCode, Value) {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {key}")).unwrap(),
    );
    let body = model.map(|model| json!({"model": model, "input": "hi", "messages": [{"role": "user", "content": "hi"}]}));
    let response = proxy_request(
        State(state),
        if model.is_some() {
            Method::POST
        } else {
            Method::GET
        },
        path.parse().unwrap(),
        headers,
        body.map(|body| Body::from(body.to_string()))
            .unwrap_or_else(Body::empty),
    )
    .await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn local_keys_isolate_routing_and_explicit_prefixes() {
    run_async(async {
        let a = spawn_mock_upstream(StatusCode::OK, json!({"source": "a"})).await;
        let b = spawn_mock_upstream(StatusCode::OK, json!({"source": "b"})).await;
        let mut config = config_with_runtime_upstreams(&[
            (PROVIDER_RESPONSES, 10, "a", &a.base_url, FORMATS_RESPONSES),
            (PROVIDER_RESPONSES, 0, "b", &b.base_url, FORMATS_RESPONSES),
        ]);
        config.local_api_keys = vec![scoped_key("a", &["a"]), scoped_key("b", &["b"])];
        let state =
            build_test_state_handle(config, next_test_data_dir("local-key-isolation")).await;
        for id in ["a", "b"] {
            let (status, body) = keyed_request(
                state.clone(),
                &format!("secret-{id}"),
                RESPONSES_PATH,
                Some("gpt-5"),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["source"], id);
        }
        assert_eq!(
            keyed_request(state.clone(), "secret-a", RESPONSES_PATH, Some("b/gpt-5"))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            keyed_request(state.clone(), "invalid", RESPONSES_PATH, Some("gpt-5"))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(a.requests().len(), 1);
        assert_eq!(b.requests().len(), 1);
        assert_eq!(
            a.requests()[0].authorization.as_deref(),
            Some("Bearer test-key")
        );
        a.abort();
        b.abort();
    });
}

#[test]
fn local_key_scope_survives_retries_race_hedging_and_provider_fallback() {
    run_async(async {
        for dispatch in [
            UpstreamDispatchRuntime::Serial,
            UpstreamDispatchRuntime::Race { max_parallel: 3 },
            UpstreamDispatchRuntime::Hedged {
                delay: std::time::Duration::from_millis(1),
                max_parallel: 3,
            },
        ] {
            let failed = spawn_mock_upstream(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": {"message": "retry"}}),
            )
            .await;
            let denied = spawn_mock_upstream(StatusCode::OK, json!({"source": "denied"})).await;
            let mut config = config_with_runtime_upstreams(&[
                (PROVIDER_CHAT, 0, "allowed", &failed.base_url, FORMATS_CHAT),
                (PROVIDER_CHAT, 0, "denied", &denied.base_url, FORMATS_CHAT),
                // 未授权 Chat 入站的 Responses 上游不得作为回退候选。
                (
                    PROVIDER_RESPONSES,
                    0,
                    "fallback-denied",
                    &denied.base_url,
                    FORMATS_RESPONSES,
                ),
            ]);
            config.same_upstream_retry_count = 1;
            config.upstream_strategy.dispatch = dispatch;
            config.local_api_keys = vec![scoped_key("limited", &["allowed"])];
            let state =
                build_test_state_handle(config, next_test_data_dir("local-key-retry")).await;
            assert_ne!(
                keyed_request(state, "secret-limited", CHAT_PATH, Some("gpt-5"))
                    .await
                    .0,
                StatusCode::OK
            );
            assert_eq!(
                failed.requests().len(),
                2,
                "same-upstream retry stays within scope"
            );
            assert!(
                denied.requests().is_empty(),
                "unauthorized native and fallback candidates must never run"
            );
            failed.abort();
            denied.abort();
        }
    });
}

#[test]
fn local_key_cached_catalog_and_compatible_index_are_isolated() {
    run_async(async {
        let a =
            spawn_model_catalog_upstream(json!({"data": [{"id": "only-a", "object": "model"}]}))
                .await;
        let b =
            spawn_model_catalog_upstream(json!({"data": [{"id": "only-b", "object": "model"}]}))
                .await;
        let mut config = config_with_runtime_upstreams(&[
            (PROVIDER_RESPONSES, 0, "a", &a.base_url, FORMATS_RESPONSES),
            (PROVIDER_RESPONSES, 0, "b", &b.base_url, FORMATS_RESPONSES),
        ]);
        config.model_list_prefix = true;
        config.local_api_keys = vec![scoped_key("a", &["a"]), scoped_key("b", &["b"])];
        let state = build_test_state_handle(config, next_test_data_dir("local-key-catalog")).await;
        refresh_model_catalog(&state).await;
        for path in ["/v1/models", "/v1beta/openai/models"] {
            for (key, own, other) in [
                ("secret-a", "only-a", "only-b"),
                ("secret-b", "only-b", "only-a"),
            ] {
                let (status, body) = keyed_request(state.clone(), key, path, None).await;
                assert_eq!(status, StatusCode::OK);
                assert!(body.to_string().contains(own));
                assert!(!body.to_string().contains(other));
            }
        }
        let access = http::RequestAuth {
            local_access: Some(http::LocalAccess {
                key_id: "a".into(),
                scope: scoped_key("a", &["a"]).scope,
            }),
            ..Default::default()
        };
        let entries = crate::proxy::upstream::collect_model_catalog_entries_for_manifest(
            state.read().await.as_ref(),
            &access,
        )
        .await;
        assert!(entries.iter().any(|(id, _)| id == "only-a"));
        assert!(!entries.iter().any(|(id, _)| id == "only-b"));
        assert_eq!(
            a.requests().len(),
            1,
            "catalog requests reuse discovery cache"
        );
        assert_eq!(b.requests().len(), 1);
        a.abort();
        b.abort();
    });
}

#[test]
fn local_key_deleted_or_disabled_binding_never_falls_back() {
    run_async(async {
        let other = spawn_mock_upstream(StatusCode::OK, json!({"source": "other"})).await;
        let mut config = config_with_runtime_upstreams(&[(
            PROVIDER_RESPONSES,
            0,
            "other",
            &other.base_url,
            FORMATS_RESPONSES,
        )]);
        config.upstream_ids.insert("disabled".into());
        config.local_api_keys = vec![
            scoped_key("limited", &["deleted", "disabled", "other"]),
            scoped_key("stale", &["deleted"]),
        ];
        let state = build_test_state_handle(config, next_test_data_dir("local-key-deleted")).await;
        for model in ["deleted/gpt-5", "disabled/gpt-5"] {
            assert_eq!(
                keyed_request(state.clone(), "secret-limited", RESPONSES_PATH, Some(model))
                    .await
                    .0,
                StatusCode::BAD_GATEWAY
            );
        }
        assert_eq!(
            keyed_request(state, "secret-stale", RESPONSES_PATH, Some("gpt-5"))
                .await
                .0,
            StatusCode::BAD_GATEWAY
        );
        assert!(other.requests().is_empty());
        other.abort();
    });
}

#[test]
fn local_key_config_snapshot_preserves_inflight_and_revokes_new_requests() {
    run_async(async {
        let a = spawn_mock_upstream_with_delay(StatusCode::OK, json!({"source": "a"}), 100).await;
        let b = spawn_mock_upstream(StatusCode::OK, json!({"source": "b"})).await;
        let mut config = config_with_runtime_upstreams(&[
            (PROVIDER_RESPONSES, 0, "a", &a.base_url, FORMATS_RESPONSES),
            (PROVIDER_RESPONSES, 0, "b", &b.base_url, FORMATS_RESPONSES),
        ]);
        config.local_api_keys = vec![scoped_key("active", &["a"])];
        let state =
            build_test_state_handle(config.clone(), next_test_data_dir("local-key-snapshot")).await;
        let running = tokio::spawn(keyed_request(
            state.clone(),
            "secret-active",
            RESPONSES_PATH,
            Some("gpt-5"),
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while a.requests().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        config.local_api_keys[0].scope = scoped_key("active", &["b"]).scope;
        let replacement =
            build_test_state_handle(config.clone(), next_test_data_dir("local-key-snapshot-new"))
                .await;
        *state.write().await = replacement.read().await.clone();
        assert_eq!(
            keyed_request(
                state.clone(),
                "secret-active",
                RESPONSES_PATH,
                Some("gpt-5")
            )
            .await
            .1["source"],
            "b"
        );
        assert_eq!(running.await.unwrap().1["source"], "a");
        config.local_api_keys[0].enabled = false;
        let revoked =
            build_test_state_handle(config, next_test_data_dir("local-key-revoked")).await;
        *state.write().await = revoked.read().await.clone();
        assert_eq!(
            keyed_request(state, "secret-active", RESPONSES_PATH, Some("gpt-5"))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(a.requests().len(), 1);
        assert_eq!(b.requests().len(), 1);
        a.abort();
        b.abort();
    });
}

#[test]
fn local_key_auto_includes_added_upstreams_and_native_routes_use_scope() {
    run_async(async {
        let a = spawn_model_catalog_upstream(json!({"source": "a"})).await;
        let b = spawn_model_catalog_upstream(json!({"source": "b"})).await;
        let mut config =
            config_with_runtime_upstreams(&[(PROVIDER_CHAT, 0, "a", &a.base_url, FORMATS_CHAT)]);
        let mut auto = scoped_key("auto", &[]);
        auto.scope = LocalApiKeyScope::Auto;
        config.local_api_keys = vec![auto.clone()];
        let state = build_test_state_handle(config, next_test_data_dir("local-key-auto-old")).await;
        assert_eq!(
            keyed_request(state.clone(), "secret-auto", "/v1/files", None)
                .await
                .1["source"],
            "a"
        );
        let mut config = config_with_runtime_upstreams(&[
            (PROVIDER_CHAT, 0, "a", &a.base_url, FORMATS_CHAT),
            (PROVIDER_RESPONSES, 10, "b", &b.base_url, FORMATS_RESPONSES),
        ]);
        config.local_api_keys = vec![auto, scoped_key("a", &["a"])];
        let replacement =
            build_test_state_handle(config, next_test_data_dir("local-key-auto-new")).await;
        *state.write().await = replacement.read().await.clone();
        assert_eq!(
            keyed_request(state.clone(), "secret-auto", "/v1/files", None)
                .await
                .1["source"],
            "b"
        );
        // 原生 Provider 选择不能被优先级更高但未授权的 Provider 占用。
        assert_eq!(
            keyed_request(state, "secret-a", "/v1/files", None).await.1["source"],
            "a"
        );
        assert_eq!(a.requests().len(), 2);
        assert_eq!(b.requests().len(), 1);
        a.abort();
        b.abort();
    });
}

#[test]
fn local_key_query_is_removed_from_converted_outbound_paths() {
    let uri: Uri = "/v1beta/models/gpt-5:generateContent?key=local-secret&alt=sse"
        .parse()
        .unwrap();
    let auth = http::RequestAuth {
        local_auth_enabled: true,
        ..Default::default()
    };
    assert_eq!(
        super::super::prepared::build_authorized_outbound_path("/v1/responses", &uri, &auth),
        "/v1/responses?alt=sse"
    );
}

#[test]
fn request_logs_attribute_every_attempt_to_the_authenticated_key() {
    run_async(async {
        let failed = spawn_mock_upstream(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": {"message": "retry"}}),
        )
        .await;
        let ok = spawn_mock_upstream(StatusCode::OK, json!({"source": "ok"})).await;
        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                10,
                "failed",
                &failed.base_url,
                FORMATS_RESPONSES,
            ),
            (PROVIDER_RESPONSES, 0, "ok", &ok.base_url, FORMATS_RESPONSES),
        ]);
        config.local_api_keys = vec![scoped_key("laptop", &["failed", "ok"])];
        config.same_upstream_retry_count = 1;
        let data_dir = next_test_data_dir("local-key-request-log");
        let (state, pool) = build_test_state_handle_with_sqlite_log(config, data_dir.clone()).await;

        let (status, body) = keyed_request(
            state.clone(),
            "secret-laptop",
            RESPONSES_PATH,
            Some("gpt-5"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "ok");
        let (status, _) =
            keyed_request(state.clone(), "invalid", RESPONSES_PATH, Some("gpt-5")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        wait_for_request_log_count(&pool, 4).await;
        let mut logged = sqlx::query("SELECT status, local_api_key_id FROM request_logs;")
            .fetch_all(&pool)
            .await
            .expect("query request logs")
            .into_iter()
            .map(|row| {
                (
                    row.get::<i64, _>("status"),
                    row.get::<Option<String>, _>("local_api_key_id"),
                )
            })
            .collect::<Vec<_>>();
        logged.sort();
        // 失败重试与最终成功都归属到同一 Key；鉴权失败的请求没有 Key。
        assert_eq!(
            logged,
            vec![
                (200, Some("laptop".to_string())),
                (401, None),
                (503, Some("laptop".to_string())),
                (503, Some("laptop".to_string())),
            ]
        );

        failed.abort();
        ok.abort();
        pool.close().await;
        let _ = std::fs::remove_dir_all(&data_dir);
    });
}
