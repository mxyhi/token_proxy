use super::*;
use crate::proxy::account_selector::AccountSelectorRuntime;
use crate::proxy::log::{build_log_entry, LogContext, UsageSnapshot};
use std::time::Instant;

#[test]
fn xai_free_usage_hint_applies_explicit_cooldown_when_default_is_disabled() {
    let selector = AccountSelectorRuntime::new_with_cooldown(std::time::Duration::ZERO);
    let mut response = http::error_response(StatusCode::OK, "free usage exhausted");
    response.extensions_mut().insert(AccountCooldownHint {
        duration: std::time::Duration::from_secs(24 * 60 * 60),
        reason: "free_usage_exhausted",
    });

    update_account_cooldown_from_response(
        &selector,
        "xai",
        Some("xai-a"),
        StatusCode::TOO_MANY_REQUESTS,
        &reqwest::header::HeaderMap::new(),
        &response,
        &CooldownScope::Global,
    );

    // 权威 free-usage cooldown 写入后应可被 is_cooling_down 查询到。
    assert!(selector.is_cooling_down_scoped("xai", "xai-a", &CooldownScope::Global));
}

#[test]
fn local_upstream_diagnostic_does_not_create_billable_attempt() {
    let billing = ClientRequestBilling::default();
    let context = LogContext {
        client_ip: None,
        path: "/v1/messages".to_string(),
        provider: "anthropic".to_string(),
        upstream_id: LOCAL_UPSTREAM_ID.to_string(),
        account_id: None,
        model: Some("claude-haiku-4-5".to_string()),
        mapped_model: None,
        stream: false,
        status: 404,
        upstream_request_id: None,
        request_headers: None,
        request_body: None,
        ttfb_ms: None,
        timings: request_timings_for_upstream(LOCAL_UPSTREAM_ID, &billing),
        start: Instant::now(),
    };

    let entry = build_log_entry(
        &context,
        UsageSnapshot::default(),
        Some("model unsupported".to_string()),
    );
    assert!(entry.client_request_id.is_none());
    assert!(entry.attempt_index.is_none());
}
