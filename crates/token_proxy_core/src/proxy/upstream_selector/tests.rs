use super::*;
use crate::proxy::config::UpstreamRuntime;

fn runtime(id: &str, selector_key: &str) -> UpstreamRuntime {
    UpstreamRuntime {
        id: id.to_string(),
        selector_key: selector_key.to_string(),
        base_url: "https://example.com".to_string(),
        api_key: Some("test-key".to_string()),
        api_key_headers: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        advertised_model_ids: Vec::new(),
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    }
}

#[test]
fn cooled_upstream_moves_behind_ready_candidates() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b"), runtime("c", "c")];

    selector.mark_cooldown_until(
        "responses",
        "a",
        &CooldownScope::Global,
        Instant::now() + Duration::from_secs(10),
    );

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "responses",
        &items,
        0,
        &CooldownScope::Global,
    );

    assert_eq!(order, vec![1, 2, 0]);
}

#[test]
fn all_cooled_upstreams_probe_earliest_expiry_first() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b"), runtime("c", "c")];

    selector.mark_cooldown_until(
        "responses",
        "a",
        &CooldownScope::Global,
        Instant::now() + Duration::from_secs(30),
    );
    selector.mark_cooldown_until(
        "responses",
        "b",
        &CooldownScope::Global,
        Instant::now() + Duration::from_secs(5),
    );
    selector.mark_cooldown_until(
        "responses",
        "c",
        &CooldownScope::Global,
        Instant::now() + Duration::from_secs(10),
    );

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "responses",
        &items,
        0,
        &CooldownScope::Global,
    );

    assert_eq!(order, vec![1, 2, 0]);
}

#[test]
fn clear_cooldown_restores_base_order() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b")];

    selector.mark_cooldown_until(
        "responses",
        "a",
        &CooldownScope::Global,
        Instant::now() + Duration::from_secs(10),
    );
    selector.clear_cooldown_scoped("responses", "a", &CooldownScope::Global);

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "responses",
        &items,
        0,
        &CooldownScope::Global,
    );

    assert_eq!(order, vec![0, 1]);
}

#[test]
fn zero_retryable_failure_cooldown_disables_cross_request_cooling() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::ZERO);
    let items = vec![runtime("a", "a"), runtime("b", "b")];

    selector.mark_retryable_failure_scoped("responses", "a", &CooldownScope::Global);

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "responses",
        &items,
        0,
        &CooldownScope::Global,
    );

    assert_eq!(order, vec![0, 1]);
    assert!(selector
        .cooldowns
        .lock()
        .expect("selector cooldown lock poisoned")
        .is_empty());
}

#[test]
fn extreme_retryable_failure_cooldown_does_not_panic() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(u64::MAX));
    let items = vec![runtime("a", "a"), runtime("b", "b")];

    let result = std::panic::catch_unwind(|| {
        selector.mark_retryable_failure_scoped("responses", "a", &CooldownScope::Global);
        selector.order_group_scoped(
            UpstreamOrderStrategy::FillFirst,
            "responses",
            &items,
            0,
            &CooldownScope::Global,
        )
    });

    assert!(result.is_ok());
}

#[test]
fn cooldown_distinguishes_runtime_items_with_same_logical_upstream_id() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("shared", "shared#1"), runtime("shared", "shared#2")];

    selector.mark_retryable_failure_scoped("responses", "shared#1", &CooldownScope::Global);

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "responses",
        &items,
        0,
        &CooldownScope::Global,
    );

    assert_eq!(order, vec![1, 0]);
}

#[test]
fn scoped_cooldown_does_not_move_other_sessions() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b")];
    let session_a = CooldownScope::CodexSession("session-a".to_string());
    let session_b = CooldownScope::CodexSession("session-b".to_string());

    selector.mark_retryable_failure_scoped("codex", "a", &session_a);

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "codex",
        &items,
        0,
        &session_b,
    );

    assert_eq!(order, vec![0, 1]);
}

#[test]
fn ordering_prunes_expired_cooldowns_from_other_scopes() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b")];
    let expired_scope = CooldownScope::CodexSession("expired-session".to_string());
    let next_scope = CooldownScope::CodexSession("next-session".to_string());
    let past = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .expect("test clock should support a one second rewind");

    selector
        .cooldowns
        .lock()
        .expect("selector cooldown lock poisoned")
        .insert(CooldownKey::new("codex", "a", &expired_scope), past);

    let order = selector.order_group_scoped(
        UpstreamOrderStrategy::FillFirst,
        "codex",
        &items,
        0,
        &next_scope,
    );

    assert_eq!(order, vec![0, 1]);
    assert!(selector
        .cooldowns
        .lock()
        .expect("selector cooldown lock poisoned")
        .is_empty());
}

#[test]
fn clear_provider_scope_restores_scoped_upstreams() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b")];
    let scope = CooldownScope::CodexSession("session-a".to_string());

    selector.mark_retryable_failure_scoped("codex", "a", &scope);
    selector.clear_provider_scope("codex", &scope);

    let order =
        selector.order_group_scoped(UpstreamOrderStrategy::FillFirst, "codex", &items, 0, &scope);

    assert_eq!(order, vec![0, 1]);
}
