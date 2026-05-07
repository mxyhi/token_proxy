use super::*;

#[test]
fn cooled_accounts_are_excluded_when_ready_accounts_exist() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string(), "c".to_string()];

    selector.mark_retryable_failure_scoped("codex", "a", &CooldownScope::Global);

    let ordered = selector.order_accounts_scoped("codex", &accounts, &CooldownScope::Global);

    assert_eq!(ordered, vec!["b".to_string(), "c".to_string()]);
}

#[test]
fn all_cooled_accounts_are_excluded_during_cooldown_window() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string()];

    selector.mark_retryable_failure_scoped("codex", "a", &CooldownScope::Global);
    selector.mark_retryable_failure_scoped("codex", "b", &CooldownScope::Global);

    let ordered = selector.order_accounts_scoped("codex", &accounts, &CooldownScope::Global);

    assert!(ordered.is_empty());
}

#[test]
fn scoped_cooldown_does_not_exclude_other_sessions() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string()];
    let session_a = CooldownScope::CodexSession("session-a".to_string());
    let session_b = CooldownScope::CodexSession("session-b".to_string());

    selector.mark_retryable_failure_scoped("codex", "a", &session_a);

    let ordered = selector.order_accounts_scoped("codex", &accounts, &session_b);

    assert_eq!(ordered, accounts);
}

#[test]
fn ordering_prunes_expired_cooldowns_from_other_scopes() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string()];
    let expired_scope = CooldownScope::CodexSession("expired-session".to_string());
    let next_scope = CooldownScope::CodexSession("next-session".to_string());
    let past = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .expect("test clock should support a one second rewind");

    selector
        .cooldowns
        .lock()
        .expect("account selector cooldown lock poisoned")
        .insert(AccountCooldownKey::new("codex", "a", &expired_scope), past);

    let ordered = selector.order_accounts_scoped("codex", &accounts, &next_scope);

    assert_eq!(ordered, accounts);
    assert!(selector
        .cooldowns
        .lock()
        .expect("account selector cooldown lock poisoned")
        .is_empty());
}

#[test]
fn zero_retryable_failure_cooldown_does_not_store_cooldowns() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::ZERO);
    let scope = CooldownScope::CodexSession("session-a".to_string());

    let marked = selector.mark_retryable_failure_scoped("codex", "a", &scope);

    assert!(marked.is_none());
    assert!(selector
        .cooldowns
        .lock()
        .expect("account selector cooldown lock poisoned")
        .is_empty());
}

#[test]
fn clear_provider_scope_restores_scoped_accounts() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let accounts = vec!["a".to_string(), "b".to_string()];
    let scope = CooldownScope::CodexSession("session-a".to_string());

    selector.mark_retryable_failure_scoped("codex", "a", &scope);
    selector.clear_provider_scope("codex", &scope);

    let ordered = selector.order_accounts_scoped("codex", &accounts, &scope);

    assert_eq!(ordered, accounts);
}
