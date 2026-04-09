use super::*;

#[test]
fn cooled_accounts_are_excluded_when_ready_accounts_exist() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string(), "c".to_string()];

    selector.mark_retryable_failure("codex", "a");

    let ordered = selector.order_accounts("codex", &accounts);

    assert_eq!(ordered, vec!["b".to_string(), "c".to_string()]);
}

#[test]
fn all_cooled_accounts_are_excluded_during_cooldown_window() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string()];

    selector.mark_retryable_failure("codex", "a");
    selector.mark_retryable_failure("codex", "b");

    let ordered = selector.order_accounts("codex", &accounts);

    assert!(ordered.is_empty());
}
