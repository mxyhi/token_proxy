use super::*;
use crate::proxy::config::UpstreamRuntime;

fn runtime(id: &str) -> UpstreamRuntime {
    UpstreamRuntime {
        id: id.to_string(),
        base_url: "https://example.com".to_string(),
        api_key: Some("test-key".to_string()),
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
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
    let selector = UpstreamSelectorRuntime::new();
    let items = vec![runtime("a"), runtime("b"), runtime("c")];

    selector.mark_cooldown_until("responses", "a", Instant::now() + Duration::from_secs(10));

    let order = selector.order_group(UpstreamStrategy::PriorityFillFirst, "responses", &items, 0);

    assert_eq!(order, vec![1, 2, 0]);
}

#[test]
fn all_cooled_upstreams_probe_earliest_expiry_first() {
    let selector = UpstreamSelectorRuntime::new();
    let items = vec![runtime("a"), runtime("b"), runtime("c")];

    selector.mark_cooldown_until("responses", "a", Instant::now() + Duration::from_secs(30));
    selector.mark_cooldown_until("responses", "b", Instant::now() + Duration::from_secs(5));
    selector.mark_cooldown_until("responses", "c", Instant::now() + Duration::from_secs(10));

    let order = selector.order_group(UpstreamStrategy::PriorityFillFirst, "responses", &items, 0);

    assert_eq!(order, vec![1, 2, 0]);
}

#[test]
fn clear_cooldown_restores_base_order() {
    let selector = UpstreamSelectorRuntime::new();
    let items = vec![runtime("a"), runtime("b")];

    selector.mark_cooldown_until("responses", "a", Instant::now() + Duration::from_secs(10));
    selector.clear_cooldown("responses", "a");

    let order = selector.order_group(UpstreamStrategy::PriorityFillFirst, "responses", &items, 0);

    assert_eq!(order, vec![0, 1]);
}
