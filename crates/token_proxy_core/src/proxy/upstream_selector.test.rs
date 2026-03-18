use super::*;
use crate::proxy::config::UpstreamRuntime;

fn runtime(id: &str, selector_key: &str) -> UpstreamRuntime {
    UpstreamRuntime {
        id: id.to_string(),
        selector_key: selector_key.to_string(),
        base_url: "https://example.com".to_string(),
        api_key: Some("test-key".to_string()),
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        rewrite_developer_role_to_system: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        model_mappings: None,
        header_overrides: None,
        allowed_inbound_formats: Default::default(),
    }
}

#[test]
fn cooled_upstream_moves_behind_ready_candidates() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b"), runtime("c", "c")];

    selector.mark_cooldown_until("responses", "a", Instant::now() + Duration::from_secs(10));

    let order = selector.order_group(UpstreamOrderStrategy::FillFirst, "responses", &items, 0);

    assert_eq!(order, vec![1, 2, 0]);
}

#[test]
fn all_cooled_upstreams_probe_earliest_expiry_first() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b"), runtime("c", "c")];

    selector.mark_cooldown_until("responses", "a", Instant::now() + Duration::from_secs(30));
    selector.mark_cooldown_until("responses", "b", Instant::now() + Duration::from_secs(5));
    selector.mark_cooldown_until("responses", "c", Instant::now() + Duration::from_secs(10));

    let order = selector.order_group(UpstreamOrderStrategy::FillFirst, "responses", &items, 0);

    assert_eq!(order, vec![1, 2, 0]);
}

#[test]
fn clear_cooldown_restores_base_order() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("a", "a"), runtime("b", "b")];

    selector.mark_cooldown_until("responses", "a", Instant::now() + Duration::from_secs(10));
    selector.clear_cooldown("responses", "a");

    let order = selector.order_group(UpstreamOrderStrategy::FillFirst, "responses", &items, 0);

    assert_eq!(order, vec![0, 1]);
}

#[test]
fn zero_retryable_failure_cooldown_disables_cross_request_cooling() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::ZERO);
    let items = vec![runtime("a", "a"), runtime("b", "b")];

    selector.mark_retryable_failure("responses", "a");

    let order = selector.order_group(UpstreamOrderStrategy::FillFirst, "responses", &items, 0);

    assert_eq!(order, vec![0, 1]);
}

#[test]
fn extreme_retryable_failure_cooldown_does_not_panic() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(u64::MAX));
    let items = vec![runtime("a", "a"), runtime("b", "b")];

    let result = std::panic::catch_unwind(|| {
        selector.mark_retryable_failure("responses", "a");
        selector.order_group(UpstreamOrderStrategy::FillFirst, "responses", &items, 0)
    });

    assert!(result.is_ok());
}

#[test]
fn cooldown_distinguishes_runtime_items_with_same_logical_upstream_id() {
    let selector = UpstreamSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let items = vec![runtime("shared", "shared#1"), runtime("shared", "shared#2")];

    selector.mark_retryable_failure("responses", "shared#1");

    let order = selector.order_group(UpstreamOrderStrategy::FillFirst, "responses", &items, 0);

    assert_eq!(order, vec![1, 0]);
}
