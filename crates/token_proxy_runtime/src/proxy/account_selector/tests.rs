use super::*;

#[test]
fn cooled_account_is_reported_while_ready_accounts_are_not() {
    // 覆盖：cooldown 写入后 is_cooling_down 查询（原 order 过滤语义）。
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));

    selector.mark_retryable_failure_scoped("codex", "a", &CooldownScope::Global);

    assert!(selector.is_cooling_down_scoped("codex", "a", &CooldownScope::Global));
    assert!(!selector.is_cooling_down_scoped("codex", "b", &CooldownScope::Global));
    assert!(!selector.is_cooling_down_scoped("codex", "c", &CooldownScope::Global));
}

#[test]
fn all_cooled_accounts_report_cooling_during_window() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));

    selector.mark_retryable_failure_scoped("codex", "a", &CooldownScope::Global);
    selector.mark_retryable_failure_scoped("codex", "b", &CooldownScope::Global);

    assert!(selector.is_cooling_down_scoped("codex", "a", &CooldownScope::Global));
    assert!(selector.is_cooling_down_scoped("codex", "b", &CooldownScope::Global));
}

#[test]
fn scoped_cooldown_does_not_affect_other_sessions() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
    let session_a = CooldownScope::CodexSession("session-a".to_string());
    let session_b = CooldownScope::CodexSession("session-b".to_string());

    selector.mark_retryable_failure_scoped("codex", "a", &session_a);

    assert!(selector.is_cooling_down_scoped("codex", "a", &session_a));
    assert!(!selector.is_cooling_down_scoped("codex", "a", &session_b));
    assert!(!selector.is_cooling_down_scoped("codex", "b", &session_b));
}

#[test]
fn cooldown_query_prunes_expired_entries_from_other_scopes() {
    let selector = AccountSelectorRuntime::new_with_cooldown(Duration::from_secs(15));
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

    // 查询会 prune 过期项；其它 scope 的过期 cooldown 不应污染当前判断。
    assert!(!selector.is_cooling_down_scoped("codex", "a", &next_scope));
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
    let scope = CooldownScope::CodexSession("session-a".to_string());

    selector.mark_retryable_failure_scoped("codex", "a", &scope);
    assert!(selector.is_cooling_down_scoped("codex", "a", &scope));
    selector.clear_provider_scope("codex", &scope);

    assert!(!selector.is_cooling_down_scoped("codex", "a", &scope));
    assert!(!selector.is_cooling_down_scoped("codex", "b", &scope));
}
