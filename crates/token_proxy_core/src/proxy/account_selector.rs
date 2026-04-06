use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::http::{header::RETRY_AFTER, HeaderMap, StatusCode};

#[derive(Hash, PartialEq, Eq)]
struct AccountCooldownKey {
    provider: String,
    account_id: String,
}

impl AccountCooldownKey {
    fn new(provider: &str, account_id: &str) -> Self {
        Self {
            provider: provider.to_string(),
            account_id: account_id.to_string(),
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

    pub(crate) fn order_accounts(&self, provider: &str, account_ids: &[String]) -> Vec<String> {
        let now = Instant::now();
        let mut ready = Vec::with_capacity(account_ids.len());
        let mut cooled = Vec::new();
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");

        for (position, account_id) in account_ids.iter().enumerate() {
            let key = AccountCooldownKey::new(provider, account_id);
            match cooldowns.get(&key).copied() {
                Some(until) if until > now => cooled.push((position, account_id.clone(), until)),
                Some(_) => {
                    cooldowns.remove(&key);
                    ready.push(account_id.clone());
                }
                None => ready.push(account_id.clone()),
            }
        }

        if cooled.is_empty() {
            return ready;
        }

        cooled.sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0)));
        let cooled_ids = cooled.into_iter().map(|(_, account_id, _)| account_id);
        if ready.is_empty() {
            return cooled_ids.collect();
        }

        ready.extend(cooled_ids);
        ready
    }

    pub(crate) fn mark_retryable_failure(&self, provider: &str, account_id: &str) -> Option<u128> {
        let Some(until) = Instant::now().checked_add(self.retryable_failure_cooldown) else {
            return None;
        };
        self.mark_cooldown_until(provider, account_id, until)
    }

    pub(crate) fn mark_response_status(
        &self,
        provider: &str,
        account_id: &str,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<u128> {
        let Some(until) = self.cooldown_until_for_status(status, headers) else {
            return None;
        };
        self.mark_cooldown_until(provider, account_id, until)
    }

    pub(crate) fn clear_cooldown(&self, provider: &str, account_id: &str) -> bool {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        cooldowns
            .remove(&AccountCooldownKey::new(provider, account_id))
            .is_some()
    }

    pub(crate) fn is_cooling_down(&self, provider: &str, account_id: &str) -> bool {
        let now = Instant::now();
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        let key = AccountCooldownKey::new(provider, account_id);
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
        until: Instant,
    ) -> Option<u128> {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .expect("account selector cooldown lock poisoned");
        let key = AccountCooldownKey::new(provider, account_id);
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
