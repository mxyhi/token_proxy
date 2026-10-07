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

#[derive(Clone, Copy)]
struct AccountCooldown {
    until: Instant,
    mandatory: bool,
}

pub(crate) struct AccountSelectorRuntime {
    retryable_failure_cooldown: Duration,
    cooldowns: Mutex<HashMap<AccountCooldownKey, AccountCooldown>>,
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
        self.mark_cooldown_until(provider, account_id, scope, until, false)
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
        self.mark_cooldown_until(provider, account_id, scope, until, true)
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
        self.mark_cooldown_until(
            provider,
            account_id,
            scope,
            until,
            retry_after_deadline(Instant::now(), headers).is_some(),
        )
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
        let key = AccountCooldownKey::new(provider, account_id, scope);
        if cooldowns.get(&key).is_some_and(|entry| entry.mandatory) {
            return false;
        }
        cooldowns.remove(&key).is_some()
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
        cooldowns.retain(|key, entry| {
            entry.mandatory || key.provider != provider || &key.scope != scope
        });
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
            Some(entry) if entry.until > now => true,
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
        let now = Instant::now();
        if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
            && retry_after_deadline(now, headers).is_some()
        {
            return retry_after_deadline(now, headers);
        }
        if self.retryable_failure_cooldown.is_zero() {
            return None;
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            if let Some(retry_after_until) = retry_after_deadline(now, headers) {
                return Some(retry_after_until);
            }
            let Some(until) = now.checked_add(self.retryable_failure_cooldown) else {
                return None;
            };
            return Some(until);
        }
        if status == StatusCode::PAYMENT_REQUIRED
            || status == StatusCode::UNAUTHORIZED
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
        mandatory: bool,
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
            Some(existing) => {
                // 普通错误不能解除已有强制等待，即便其截止时间更短。
                existing.mandatory |= mandatory;
                if existing.until >= until {
                    return None;
                }
                existing.until = until;
                instant_to_epoch_ms(until)
            }
            None => {
                cooldowns.insert(key, AccountCooldown { until, mandatory });
                instant_to_epoch_ms(until)
            }
        }
    }
}

fn prune_expired_cooldowns(
    cooldowns: &mut HashMap<AccountCooldownKey, AccountCooldown>,
    now: Instant,
) {
    cooldowns.retain(|_, entry| entry.until > now);
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
