use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::http::{header::RETRY_AFTER, HeaderMap, StatusCode};

use super::cooldown_scope::CooldownScope;

#[derive(Hash, PartialEq, Eq)]
struct AccountCooldownKey {
    provider: String,
    account_id: String,
    scope: CooldownScope,
}

impl AccountCooldownKey {
    fn new(provider: &str, account_id: &str, scope: &CooldownScope) -> Self {
        Self {
            provider: provider.to_string(),
            account_id: account_id.to_string(),
            scope: scope.clone(),
        }
    }
}

pub(crate) struct AccountSelectorRuntime {
    retryable_failure_cooldown: Duration,
    cooldowns: Mutex<HashMap<AccountCooldownKey, Instant>>,
}

impl AccountSelectorRuntime {
    pub(crate) fn new_with_cooldown(retryable_failure_cooldown: Duration) -> Self {
        Self {
            retryable_failure_cooldown,
            cooldowns: Mutex::new(HashMap::new()),
        }
    }

    /// 可重试失败后写入账号 cooldown（生产路径：retry / result）。
    pub(crate) fn mark_retryable_failure_scoped(
        &self,
        provider: &str,
        account_id: &str,
        scope: &CooldownScope,
    ) -> Option<u128> {
        if self.retryable_failure_cooldown.is_zero() {
            return None;
        }
        let Some(until) = Instant::now().checked_add(self.retryable_failure_cooldown) else {
            return None;
        };
        self.mark_cooldown_until(provider, account_id, scope, until)
    }

    /// Provider 给出权威恢复窗口时直接采用，不受通用短冷却开关影响。
    pub(crate) fn mark_explicit_cooldown_scoped(
        &self,
        provider: &str,
        account_id: &str,
        duration: Duration,
        scope: &CooldownScope,
    ) -> Option<u128> {
        let until = Instant::now().checked_add(duration)?;
        self.mark_cooldown_until(provider, account_id, scope, until)
    }

    /// 按 HTTP 状态 / Retry-After 写入 cooldown（生产路径：upstream result）。
    pub(crate) fn mark_response_status_scoped(
        &self,
        provider: &str,
        account_id: &str,
        status: StatusCode,
        headers: &HeaderMap,
        scope: &CooldownScope,
    ) -> Option<u128> {
        let Some(until) = self.cooldown_until_for_status(status, headers) else {
            return None;
        };
        self.mark_cooldown_until(provider, account_id, scope, until)
    }

    pub(crate) fn clear_cooldown_scoped(
        &self,
        provider: &str,
        account_id: &str,
        scope: &CooldownScope,
    ) -> bool {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        prune_expired_cooldowns(&mut cooldowns, Instant::now());
        cooldowns
            .remove(&AccountCooldownKey::new(provider, account_id, scope))
            .is_some()
    }

    pub(crate) fn clear_provider_scope(&self, provider: &str, scope: &CooldownScope) {
        if scope.is_global() {
            return;
        }
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        prune_expired_cooldowns(&mut cooldowns, Instant::now());
        cooldowns.retain(|key, _| key.provider != provider || &key.scope != scope);
    }

    pub(crate) fn is_cooling_down(&self, provider: &str, account_id: &str) -> bool {
        self.is_cooling_down_scoped(provider, account_id, &CooldownScope::Global)
    }

    /// 查询指定 scope 下账户是否处于 cooldown（健康保护，不产生候选序列）。
    pub(crate) fn is_cooling_down_scoped(
        &self,
        provider: &str,
        account_id: &str,
        scope: &CooldownScope,
    ) -> bool {
        let now = Instant::now();
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        prune_expired_cooldowns(&mut cooldowns, now);
        let key = AccountCooldownKey::new(provider, account_id, scope);
        match cooldowns.get(&key).copied() {
            Some(until) if until > now => true,
            Some(_) => {
                cooldowns.remove(&key);
                false
            }
            None => false,
        }
    }

    fn cooldown_until_for_status(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Instant> {
        if self.retryable_failure_cooldown.is_zero() {
            return None;
        }
        let now = Instant::now();
        if status == StatusCode::TOO_MANY_REQUESTS {
            if let Some(retry_after_until) = retry_after_deadline(now, headers) {
                return Some(retry_after_until);
            }
            let Some(until) = now.checked_add(self.retryable_failure_cooldown) else {
                return None;
            };
            return Some(until);
        }
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::REQUEST_TIMEOUT
            || status.is_server_error()
        {
            return now.checked_add(self.retryable_failure_cooldown);
        }
        None
    }

    fn mark_cooldown_until(
        &self,
        provider: &str,
        account_id: &str,
        scope: &CooldownScope,
        until: Instant,
    ) -> Option<u128> {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        let now = Instant::now();
        if until <= now {
            return None;
        }
        prune_expired_cooldowns(&mut cooldowns, now);
        let key = AccountCooldownKey::new(provider, account_id, scope);
        match cooldowns.get_mut(&key) {
            Some(existing) if *existing >= until => None,
            Some(existing) => {
                *existing = until;
                instant_to_epoch_ms(until)
            }
            None => {
                cooldowns.insert(key, until);
                instant_to_epoch_ms(until)
            }
        }
    }
}

fn prune_expired_cooldowns(cooldowns: &mut HashMap<AccountCooldownKey, Instant>, now: Instant) {
    cooldowns.retain(|_, until| *until > now);
}

fn retry_after_deadline(now: Instant, headers: &HeaderMap) -> Option<Instant> {
    let raw_value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    let seconds = raw_value.parse::<u64>().ok()?;
    now.checked_add(Duration::from_secs(seconds))
}

fn instant_to_epoch_ms(until: Instant) -> Option<u128> {
    let remaining = until.checked_duration_since(Instant::now())?;
    let wall_clock = SystemTime::now().checked_add(remaining)?;
    wall_clock
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_millis())
}

#[cfg(test)]
mod tests;
