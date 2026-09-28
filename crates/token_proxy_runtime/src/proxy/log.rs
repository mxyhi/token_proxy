use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) use token_proxy_storage::log::{
    attach_response_body, LogEntry, LogWriter, TokenUsage, UsageSnapshot,
};
use token_proxy_storage::pricing::{calculate_request_cost, default_model_pricing_settings};

#[derive(Clone)]
pub(crate) struct ClientRequestBilling {
    request_id: Arc<str>,
    next_completion_index: Arc<AtomicU64>,
    pub(crate) lifecycle: Arc<super::client_lifecycle::ClientLifecycle>,
}

impl Default for ClientRequestBilling {
    fn default() -> Self {
        Self {
            request_id: format!("{:032x}", rand::random::<u128>()).into(),
            next_completion_index: Arc::new(AtomicU64::new(0)),
            lifecycle: Arc::default(),
        }
    }
}

impl ClientRequestBilling {
    fn complete_attempt(&self) -> BillingAttempt {
        BillingAttempt {
            request_id: self.request_id.to_string(),
            index: self.next_completion_index.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[derive(Clone)]
struct BillingAttempt {
    request_id: String,
    index: u64,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct RequestTimingSnapshot {
    pub(crate) upstream_first_byte_ms: Option<u128>,
    pub(crate) upstream_response_headers_ms: Option<u128>,
    pub(crate) upstream_first_body_chunk_ms: Option<u128>,
    pub(crate) first_client_flush_ms: Option<u128>,
    pub(crate) first_output_ms: Option<u128>,
}

#[derive(Clone, Default)]
pub(crate) struct RequestTimings {
    inner: Arc<Mutex<RequestTimingSnapshot>>,
    billing: Option<ClientRequestBilling>,
    billing_attempt: Arc<OnceLock<BillingAttempt>>,
    sent_service_tier: Arc<OnceLock<Option<String>>>,
}

impl RequestTimings {
    pub(crate) fn with_billing(billing: ClientRequestBilling) -> Self {
        Self {
            inner: Arc::default(),
            billing: Some(billing),
            billing_attempt: Arc::default(),
            sent_service_tier: Arc::default(),
        }
    }

    pub(crate) fn record_sent_service_tier(&self, body: &[u8]) {
        #[derive(serde::Deserialize)]
        struct TierMetadata {
            service_tier: Option<String>,
        }
        // 仅保存最终发出请求的档位，不保存正文；不受临时 Request Detail 开关影响。
        let tier = serde_json::from_slice::<TierMetadata>(body)
            .ok()
            .and_then(|metadata| metadata.service_tier)
            .map(|tier| tier.trim().to_string())
            .filter(|tier| !tier.is_empty());
        let _ = self.sent_service_tier.set(tier);
    }

    pub(crate) fn mark_upstream_response_headers(&self, value: u128) {
        self.mark_once(|snapshot| &mut snapshot.upstream_response_headers_ms, value);
    }

    pub(crate) fn mark_upstream_first_body_chunk(&self, value: u128) {
        self.mark_once(|snapshot| &mut snapshot.upstream_first_body_chunk_ms, value);
        self.mark_once(|snapshot| &mut snapshot.upstream_first_byte_ms, value);
    }

    fn mark_upstream_first_byte(&self, value: u128) {
        self.mark_upstream_first_body_chunk(value);
    }

    fn mark_first_client_flush(&self, value: u128) {
        self.mark_once(|snapshot| &mut snapshot.first_client_flush_ms, value);
    }

    fn mark_first_output(&self, value: u128) {
        self.mark_once(|snapshot| &mut snapshot.first_output_ms, value);
    }

    fn snapshot(&self) -> RequestTimingSnapshot {
        self.inner.lock().map(|guard| *guard).unwrap_or_default()
    }

    fn billing_attempt(&self) -> Option<&BillingAttempt> {
        let billing = self.billing.as_ref()?;
        Some(
            self.billing_attempt
                .get_or_init(|| billing.complete_attempt()),
        )
    }

    fn mark_once(
        &self,
        select: impl FnOnce(&mut RequestTimingSnapshot) -> &mut Option<u128>,
        value: u128,
    ) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let slot = select(&mut guard);
        if slot.is_none() {
            *slot = Some(value);
        }
    }
}

#[derive(Clone)]
pub(crate) struct LogContext {
    pub(crate) client_ip: Option<String>,
    pub(crate) path: String,
    pub(crate) provider: String,
    pub(crate) upstream_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) mapped_model: Option<String>,
    pub(crate) stream: bool,
    pub(crate) status: u16,
    pub(crate) upstream_request_id: Option<String>,
    pub(crate) request_headers: Option<String>,
    pub(crate) request_body: Option<String>,
    // Legacy field name: this records first upstream body chunk, not response headers.
    pub(crate) ttfb_ms: Option<u128>,
    pub(crate) timings: RequestTimings,
    pub(crate) start: Instant,
}

impl LogContext {
    pub(crate) fn mark_upstream_first_byte(&mut self) {
        let value = self.start.elapsed().as_millis();
        if self.ttfb_ms.is_none() {
            self.ttfb_ms = Some(value);
        }
        self.timings.mark_upstream_first_byte(value);
    }

    pub(crate) fn mark_first_client_flush(&mut self) {
        self.timings
            .mark_first_client_flush(self.start.elapsed().as_millis());
    }

    pub(crate) fn mark_first_output(&mut self) {
        self.timings
            .mark_first_output(self.start.elapsed().as_millis());
    }

    pub(crate) fn timing_snapshot(&self) -> RequestTimingSnapshot {
        self.timings.snapshot()
    }
}

pub(crate) fn build_log_entry(
    context: &LogContext,
    usage: UsageSnapshot,
    response_error: Option<String>,
) -> LogEntry {
    let canceled = context
        .timings
        .billing
        .as_ref()
        .is_some_and(|billing| billing.lifecycle.is_canceled())
        && context.status < 400
        && response_error.as_deref() == Some(super::response::STREAM_DROPPED_ERROR);
    if let Some(billing) = &context.timings.billing {
        billing.lifecycle.note_log();
    }
    let response_error = if canceled {
        Some(super::client_lifecycle::CLIENT_CANCELED_ERROR.to_string())
    } else {
        response_error
    };
    let timing = context.timing_snapshot();
    let upstream_first_body_chunk_ms = timing
        .upstream_first_body_chunk_ms
        .or(timing.upstream_first_byte_ms)
        .or(context.ttfb_ms);
    let upstream_first_byte_ms = timing
        .upstream_first_byte_ms
        .or(upstream_first_body_chunk_ms);
    let latency_ms = timing
        .first_output_ms
        .or(timing.first_client_flush_ms)
        .or(upstream_first_body_chunk_ms)
        .or(timing.upstream_response_headers_ms)
        .unwrap_or_else(|| context.start.elapsed().as_millis());
    let service_tier =
        usage
            .service_tier
            .clone()
            .or_else(|| match context.timings.sent_service_tier.get() {
                Some(tier) => tier.clone(),
                None => service_tier_from_request_body(context.request_body.as_deref()),
            });
    let pricing_settings = default_model_pricing_settings();
    let request_cost = calculate_request_cost(
        &pricing_settings,
        context.model.as_deref(),
        context.mapped_model.as_deref(),
        service_tier.as_deref(),
        &usage.billable_usage,
    );
    let billing_attempt = context.timings.billing_attempt();
    log_upstream_response_model_mismatch(
        context.model.as_deref(),
        context.mapped_model.as_deref(),
        usage.response_model.as_deref(),
    );
    LogEntry {
        ts_ms: now_ms(),
        client_ip: context.client_ip.clone(),
        path: context.path.clone(),
        provider: context.provider.clone(),
        upstream_id: context.upstream_id.clone(),
        account_id: context.account_id.clone(),
        model: context.model.clone(),
        mapped_model: context.mapped_model.clone(),
        upstream_response_model: usage.response_model.clone(),
        stream: context.stream,
        status: if canceled { 499 } else { context.status },
        usage: usage.usage,
        billable_usage: usage.billable_usage,
        service_tier,
        usage_json: usage.usage_json,
        upstream_request_id: context.upstream_request_id.clone(),
        request_headers: context.request_headers.clone(),
        request_body: context.request_body.clone(),
        response_body: None,
        response_error,
        latency_ms,
        upstream_first_byte_ms,
        upstream_response_headers_ms: timing.upstream_response_headers_ms,
        upstream_first_body_chunk_ms,
        first_client_flush_ms: timing.first_client_flush_ms,
        first_output_ms: timing.first_output_ms,
        cost_nano_usd: request_cost.as_ref().map(|cost| cost.cost_nano_usd),
        pricing_version: pricing_settings.version,
        pricing_model: request_cost.as_ref().map(|cost| cost.pricing_model.clone()),
        pricing_context_tier: request_cost
            .as_ref()
            .map(|cost| cost.context_tier.as_str().to_string()),
        client_request_id: billing_attempt.map(|attempt| attempt.request_id.clone()),
        attempt_index: billing_attempt.map(|attempt| attempt.index),
    }
}

fn log_upstream_response_model_mismatch(
    requested: Option<&str>,
    mapped: Option<&str>,
    response_model: Option<&str>,
) {
    let Some(response_model) = response_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return;
    };
    let sent = mapped
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .or_else(|| requested.map(str::trim).filter(|model| !model.is_empty()))
        .unwrap_or("");
    if sent.eq_ignore_ascii_case(response_model) {
        return;
    }
    // 上游把请求模型换成了另一个 id。表格会标「模型不一致」，这里留一条可检索日志。
    tracing::info!(
        requested = requested.unwrap_or(""),
        mapped = mapped.unwrap_or(""),
        upstream_response_model = response_model,
        "upstream response model differs from the model sent upstream"
    );
}

fn service_tier_from_request_body(request_body: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<Value>(request_body?).ok()?;
    value
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use token_proxy_storage::pricing::BillableUsage;

    fn context(timings: RequestTimings) -> LogContext {
        LogContext {
            client_ip: Some("203.0.113.9".to_string()),
            path: "/v1/responses".to_string(),
            provider: "openai-response".to_string(),
            upstream_id: "airouter".to_string(),
            account_id: None,
            model: Some("alias".to_string()),
            mapped_model: Some("gpt-5.4".to_string()),
            stream: true,
            status: 200,
            upstream_request_id: None,
            request_headers: None,
            request_body: None,
            ttfb_ms: None,
            timings,
            start: Instant::now() - Duration::from_millis(300),
        }
    }

    #[test]
    fn build_log_entry_keeps_timing_dimensions_separate() {
        let timings = RequestTimings::default();
        timings.mark_upstream_response_headers(25);
        timings.mark_upstream_first_body_chunk(120);
        timings.mark_first_output(220);

        let entry = build_log_entry(&context(timings), UsageSnapshot::default(), None);

        assert_eq!(entry.upstream_response_headers_ms, Some(25));
        assert_eq!(entry.upstream_first_body_chunk_ms, Some(120));
        assert_eq!(entry.first_output_ms, Some(220));
        assert_eq!(entry.latency_ms, 220);
    }

    #[test]
    fn build_log_entry_assigns_attempts_by_completion_order() {
        let billing = ClientRequestBilling::default();
        let first = build_log_entry(
            &context(RequestTimings::with_billing(billing.clone())),
            UsageSnapshot::default(),
            Some("first".to_string()),
        );
        let second = build_log_entry(
            &context(RequestTimings::with_billing(billing)),
            UsageSnapshot::default(),
            Some("second".to_string()),
        );

        assert_eq!(first.client_request_id, second.client_request_id);
        assert_eq!(first.attempt_index, Some(0));
        assert_eq!(second.attempt_index, Some(1));
    }

    #[test]
    fn build_log_entry_calculates_request_cost() {
        let usage = UsageSnapshot {
            usage: Some(TokenUsage {
                input_tokens: Some(1_000_000),
                output_tokens: Some(10_000),
                total_tokens: Some(1_010_000),
            }),
            billable_usage: BillableUsage {
                uncached_input_tokens: 800_000,
                cache_read_tokens: 200_000,
                output_tokens: 10_000,
                ..BillableUsage::default()
            },
            service_tier: None,
            usage_json: None,
            response_model: None,
        };

        let entry = build_log_entry(&context(RequestTimings::default()), usage, None);

        assert_eq!(entry.cost_nano_usd, Some(4_325_000_000));
        assert_eq!(entry.pricing_model.as_deref(), Some("gpt-5.4"));
        assert_eq!(entry.pricing_context_tier.as_deref(), Some("long"));
    }

    #[test]
    fn actual_service_tier_overrides_requested_fast_for_billing() {
        let mut context = context(RequestTimings::default());
        context.model = Some("gpt-5.6-terra".to_string());
        context.request_body = Some(r#"{"service_tier":"fast"}"#.to_string());
        context
            .timings
            .record_sent_service_tier(br#"{"service_tier":"priority"}"#);
        let usage = UsageSnapshot {
            billable_usage: BillableUsage {
                uncached_input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let requested = build_log_entry(&context, usage.clone(), None);
        let actual = build_log_entry(
            &context,
            UsageSnapshot {
                service_tier: Some("default".to_string()),
                ..usage
            },
            None,
        );
        assert_eq!(requested.service_tier.as_deref(), Some("priority"));
        assert_eq!(actual.service_tier.as_deref(), Some("default"));
        assert_eq!(
            requested.cost_nano_usd,
            actual.cost_nano_usd.map(|cost| cost * 2)
        );
        context.timings = RequestTimings::default();
        context.timings.record_sent_service_tier(b"{}");
        let omitted = build_log_entry(&context, UsageSnapshot::default(), None);
        assert_eq!(
            omitted.service_tier, None,
            "removed tier must not return from captured input"
        );
    }
}
