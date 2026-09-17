use super::*;

fn success_response(id: &str) -> Value {
    json!({
        "id": id, "object": "response", "created_at": 123,
        "model": "gpt-5", "status": "completed",
        "output": [{"type": "message", "role": "assistant", "content": [
            {"type": "output_text", "text": id}
        ]}],
        "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
    })
}

/// 通过真实 HTTP attempt 和 SQLite 验证 A(102) -> B(101) -> C(99)，包含 A 原地重试。
async fn assert_global_priority(codex_status: StatusCode) {
    let primary = spawn_mock_upstream(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "primary overloaded"}}),
    )
    .await;
    let middle = spawn_mock_upstream(
        codex_status,
        if codex_status.is_success() {
            success_response("resp_middle")
        } else {
            json!({"error": {"message": "middle overloaded"}})
        },
    )
    .await;
    let last = spawn_mock_upstream(StatusCode::OK, success_response("resp_last")).await;
    let config = config_with_runtime_upstreams(&[
        (
            PROVIDER_RESPONSES,
            102,
            "primary",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_CODEX,
            101,
            "middle",
            middle.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            99,
            "last",
            last.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    let data_dir = next_test_data_dir(&format!("global_priority_{}", codex_status.as_u16()));
    let (state, pool) = build_test_state_handle_with_sqlite_log(config, data_dir.clone()).await;
    let (status, response) = send_responses_request(state).await;
    let expected = if codex_status.is_success() {
        vec!["primary", "primary", "middle"]
    } else {
        vec!["primary", "primary", "middle", "middle", "last"]
    };
    // 按当前实际发送数等待，旧实现应立即在顺序断言处失败，不为预期缺失的日志长时间轮询。
    let actual_count = primary.requests().len() + middle.requests().len() + last.requests().len();
    wait_for_request_log_count(&pool, actual_count as i64).await;
    let rows = sqlx::query(
        "SELECT upstream_id, client_request_id, attempt_index, is_billable FROM request_logs ORDER BY attempt_index ASC"
    ).fetch_all(&pool).await.expect("query attempt sequence");
    let actual: Vec<String> = rows.iter().map(|row| row.get("upstream_id")).collect();
    let middle_requests = middle.requests();
    let last_requests = last.requests();
    primary.abort();
    middle.abort();
    last.abort();
    pool.close().await;
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(actual, expected, "priority must interleave providers");
    assert_eq!(
        response["id"],
        if codex_status.is_success() {
            "resp_middle"
        } else {
            "resp_last"
        }
    );
    assert_eq!(middle_requests[0].path, CODEX_RESPONSES_PATH);
    assert!(
        middle_requests[0].body["input"].is_array(),
        "Codex uses its own request conversion"
    );
    if !codex_status.is_success() {
        assert_eq!(last_requests[0].path, RESPONSES_PATH);
        assert_eq!(
            last_requests[0].body["input"], "hi",
            "returning to Responses reuses the original body"
        );
    }
    let request_id: String = rows[0].get("client_request_id");
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.get::<String, _>("client_request_id"), request_id);
        assert_eq!(row.get::<i64, _>("attempt_index"), index as i64);
        assert_eq!(
            row.get::<i64, _>("is_billable"),
            i64::from(index + 1 == rows.len())
        );
    }
}

#[test]
fn global_priority_codex_success_precedes_lower_responses_upstream() {
    run_async(assert_global_priority(StatusCode::OK));
}

#[test]
fn global_priority_returns_to_responses_after_codex_failure() {
    run_async(assert_global_priority(StatusCode::SERVICE_UNAVAILABLE));
}

/// 中间 Codex 不符合模型或被冷却时必须跳过，且低优先级上游仍能接管。
async fn assert_middle_skipped(cooling: bool) {
    let primary = spawn_mock_upstream(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error":{"message":"overloaded"}}),
    )
    .await;
    let middle = spawn_mock_upstream(StatusCode::OK, success_response("resp_middle")).await;
    let last = spawn_mock_upstream(StatusCode::OK, success_response("resp_last")).await;
    let mut config = config_with_runtime_upstreams(&[
        (
            PROVIDER_RESPONSES,
            102,
            "primary",
            primary.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_CODEX,
            101,
            "middle",
            middle.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            99,
            "last",
            last.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    if !cooling {
        config.upstreams.get_mut(PROVIDER_CODEX).unwrap().groups[0].items[0].available_models =
            vec!["different-model".into()];
    }
    let data_dir = next_test_data_dir(&format!("global_priority_skip_{cooling}"));
    let state = build_test_state_handle(config, data_dir.clone()).await;
    if cooling {
        state
            .read()
            .await
            .account_selector
            .mark_explicit_cooldown_scoped(
                PROVIDER_CODEX,
                "codex-middle.json",
                std::time::Duration::from_secs(60),
                &crate::proxy::cooldown_scope::CooldownScope::Global,
            );
    }
    let (status, response) = send_responses_request(state).await;
    let counts = (
        primary.requests().len(),
        middle.requests().len(),
        last.requests().len(),
    );
    primary.abort();
    middle.abort();
    last.abort();
    let _ = std::fs::remove_dir_all(&data_dir);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["id"], "resp_last");
    assert_eq!(counts, (2, 0, 1));
}

#[test]
fn global_priority_skips_model_mismatch() {
    run_async(assert_middle_skipped(false));
}

#[test]
fn global_priority_skips_cooling_codex_account() {
    run_async(assert_middle_skipped(true));
}

async fn assert_cross_provider_parallel(dispatch: UpstreamDispatchRuntime, label: &str) {
    let slow =
        spawn_mock_upstream_with_delay(StatusCode::OK, success_response("resp_slow"), 250).await;
    let fast = spawn_mock_upstream(StatusCode::OK, success_response("resp_fast")).await;
    let lower = spawn_mock_upstream(StatusCode::OK, success_response("resp_lower")).await;
    let mut config = config_with_runtime_upstreams(&[
        (
            PROVIDER_RESPONSES,
            101,
            "a-slow",
            slow.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_CODEX,
            101,
            "b-fast",
            fast.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
        (
            PROVIDER_RESPONSES,
            99,
            "lower",
            lower.base_url.as_str(),
            FORMATS_RESPONSES,
        ),
    ]);
    config.upstream_strategy.dispatch = dispatch;
    let data_dir = next_test_data_dir(label);
    let state = build_test_state_handle(config, data_dir.clone()).await;
    let (status, response) = send_responses_request(state).await;
    let counts = (
        slow.requests().len(),
        fast.requests().len(),
        lower.requests().len(),
    );
    slow.abort();
    fast.abort();
    lower.abort();
    let _ = std::fs::remove_dir_all(&data_dir);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["id"], "resp_fast");
    assert_eq!(
        counts,
        (1, 1, 0),
        "race/hedge must stay inside the current priority group"
    );
}

#[test]
fn global_priority_races_different_providers_at_same_priority() {
    run_async(assert_cross_provider_parallel(
        UpstreamDispatchRuntime::Race { max_parallel: 2 },
        "global_priority_race",
    ));
}

#[test]
fn global_priority_hedges_different_providers_at_same_priority() {
    run_async(assert_cross_provider_parallel(
        UpstreamDispatchRuntime::Hedged {
            delay: std::time::Duration::from_millis(30),
            max_parallel: 2,
        },
        "global_priority_hedge",
    ));
}

#[test]
fn global_priority_round_robin_spans_providers() {
    run_async(async {
        let first = spawn_mock_upstream(StatusCode::OK, success_response("resp_first")).await;
        let second = spawn_mock_upstream(StatusCode::OK, success_response("resp_second")).await;
        let mut config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                101,
                "a-first",
                first.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_CODEX,
                101,
                "b-second",
                second.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        config.upstream_strategy.order = UpstreamOrderStrategy::RoundRobin;
        let data_dir = next_test_data_dir("global_priority_round_robin");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let first_result = send_responses_request(state.clone()).await;
        let second_result = send_responses_request(state).await;
        let counts = (first.requests().len(), second.requests().len());
        first.abort();
        second.abort();
        let _ = std::fs::remove_dir_all(&data_dir);
        assert_eq!(first_result.1["id"], "resp_first");
        assert_eq!(second_result.1["id"], "resp_second");
        assert_eq!(counts, (1, 1));
    });
}

#[test]
fn global_priority_honors_target_prefix_across_providers() {
    run_async(async {
        let primary = spawn_mock_upstream(StatusCode::OK, success_response("resp_primary")).await;
        let target = spawn_mock_upstream(StatusCode::OK, success_response("resp_target")).await;
        let config = config_with_runtime_upstreams(&[
            (
                PROVIDER_RESPONSES,
                102,
                "primary",
                primary.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
            (
                PROVIDER_CODEX,
                101,
                "target",
                target.base_url.as_str(),
                FORMATS_RESPONSES,
            ),
        ]);
        let data_dir = next_test_data_dir("global_priority_target");
        let state = build_test_state_handle(config, data_dir.clone()).await;
        let (status, _) =
            send_responses_request_with_body(state, json!({"model":"target/gpt-5","input":"hi"}))
                .await;
        let target_requests = target.requests();
        let primary_count = primary.requests().len();
        primary.abort();
        target.abort();
        let _ = std::fs::remove_dir_all(&data_dir);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(primary_count, 0);
        assert_eq!(target_requests.len(), 1);
        assert_eq!(target_requests[0].body["model"], "gpt-5");
    });
}
