#![allow(dead_code)]

use ai_usage_bar::{
    AdapterError, Clock, Confidence, Freshness, MetricKind, Provider, ProviderAdapter,
    RefreshPolicy, RefreshService, UsageSnapshot, WindowKind,
};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Clone)]
pub struct FixedClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl FixedClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().expect("test clock poisoned") = now;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("test clock poisoned")
    }
}

pub fn instant() -> DateTime<Utc> {
    Utc.timestamp_opt(1_786_000_000, 0).unwrap()
}

pub fn percent_snapshot(
    provider: Provider,
    observed_at: DateTime<Utc>,
    used: Option<f64>,
    freshness: Freshness,
    label: Option<&str>,
) -> UsageSnapshot {
    metric_snapshot(
        provider,
        observed_at,
        MetricKind::Quota,
        WindowKind::Weekly,
        "percent",
        used,
        used.map(|value| 100.0 - value),
        Some(100.0),
        freshness,
        label,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn metric_snapshot(
    provider: Provider,
    observed_at: DateTime<Utc>,
    metric_kind: MetricKind,
    window_kind: WindowKind,
    unit: &str,
    used: Option<f64>,
    remaining: Option<f64>,
    limit: Option<f64>,
    freshness: Freshness,
    label: Option<&str>,
) -> UsageSnapshot {
    UsageSnapshot {
        provider,
        account_id: "test-account".to_string(),
        metric_kind,
        window_kind,
        unit: unit.to_string(),
        observed_at,
        source: ai_usage_bar::Source::Fixture,
        freshness,
        confidence: Confidence::Exact,
        used,
        remaining,
        limit,
        unlimited: false,
        resets_at: Some(observed_at + chrono::Duration::days(7)),
        window_label: label.map(str::to_string),
        error: None,
    }
}

pub struct SequenceAdapter {
    provider: Provider,
    responses: Mutex<VecDeque<Result<Vec<UsageSnapshot>, AdapterError>>>,
    pub calls: Arc<AtomicUsize>,
}

impl SequenceAdapter {
    pub fn new(
        provider: Provider,
        responses: Vec<Result<Vec<UsageSnapshot>, AdapterError>>,
    ) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                provider,
                responses: Mutex::new(responses.into()),
                calls: calls.clone(),
            },
            calls,
        )
    }
}

impl ProviderAdapter for SequenceAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .expect("test adapter poisoned")
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }
}

pub fn service(
    registry: ai_usage_bar::ProviderRegistry,
    clock: &FixedClock,
    policy: RefreshPolicy,
) -> RefreshService {
    RefreshService::with_clock(registry, policy, Arc::new(clock.clone()))
}
