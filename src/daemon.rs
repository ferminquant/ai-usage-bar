use crate::model::{
    AdapterError, ErrorCode, Freshness, MetricKind, Provider, ProviderAdapter, Source,
    UsageSnapshot, WindowKind,
};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::sync::{
    mpsc::{self, RecvTimeoutError},
    Arc, Mutex, RwLock,
};
use std::thread;
use std::time::Duration;

/// Supplies the observation time used by refresh and cache decisions.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshPolicy {
    /// Maximum age for a cached snapshot to retain the `cached` state.
    pub cache_ttl: Duration,
    /// Maximum age for retaining a snapshot after a refresh failure.
    pub stale_after: Duration,
    /// Maximum time to wait for one provider adapter.
    pub provider_timeout: Duration,
    /// Maximum number of providers fetched at once.
    pub max_concurrency: usize,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            cache_ttl: Duration::from_secs(60),
            stale_after: Duration::from_secs(15 * 60),
            provider_timeout: Duration::from_secs(10),
            max_concurrency: 4,
        }
    }
}

impl RefreshPolicy {
    fn normalized(self) -> Self {
        Self {
            stale_after: self.stale_after.max(self.cache_ttl),
            max_concurrency: self.max_concurrency.max(1),
            ..self
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("provider {0} is already registered")]
    DuplicateProvider(Provider),
    #[error("provider {0} is not registered")]
    UnknownProvider(Provider),
}

pub type SharedAdapter = Arc<dyn ProviderAdapter + Send + Sync>;

struct RegisteredProvider {
    provider: Provider,
    adapter: Option<SharedAdapter>,
    enabled: bool,
}

impl Clone for RegisteredProvider {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            adapter: self.adapter.clone(),
            enabled: self.enabled,
        }
    }
}

/// Thread-safe provider registry used by the refresh service.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    entries: Arc<RwLock<Vec<RegisteredProvider>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<A>(&self, adapter: A) -> Result<(), RegistryError>
    where
        A: ProviderAdapter + Send + Sync + 'static,
    {
        self.register_shared(Arc::new(adapter))
    }

    pub fn register_shared(&self, adapter: SharedAdapter) -> Result<(), RegistryError> {
        let provider = adapter.provider();
        let mut entries = self.entries.write().expect("provider registry poisoned");
        if entries.iter().any(|entry| entry.provider == provider) {
            return Err(RegistryError::DuplicateProvider(provider));
        }
        entries.push(RegisteredProvider {
            provider,
            adapter: Some(adapter),
            enabled: true,
        });
        Ok(())
    }

    pub fn register_not_configured(&self, provider: Provider) -> Result<(), RegistryError> {
        let mut entries = self.entries.write().expect("provider registry poisoned");
        if entries.iter().any(|entry| entry.provider == provider) {
            return Err(RegistryError::DuplicateProvider(provider));
        }
        entries.push(RegisteredProvider {
            provider,
            adapter: None,
            enabled: true,
        });
        Ok(())
    }

    pub fn set_enabled(&self, provider: &Provider, enabled: bool) -> Result<(), RegistryError> {
        let mut entries = self.entries.write().expect("provider registry poisoned");
        let Some(entry) = entries.iter_mut().find(|entry| &entry.provider == provider) else {
            return Err(RegistryError::UnknownProvider(provider.clone()));
        };
        entry.enabled = enabled;
        Ok(())
    }

    /// Return providers known to this registry in registration order.
    pub fn registered_providers(&self) -> Vec<Provider> {
        self.entries
            .read()
            .expect("provider registry poisoned")
            .iter()
            .map(|entry| entry.provider.clone())
            .collect()
    }

    /// Return whether a registered provider is enabled.
    pub fn is_enabled(&self, provider: &Provider) -> Result<bool, RegistryError> {
        self.entries
            .read()
            .expect("provider registry poisoned")
            .iter()
            .find(|entry| &entry.provider == provider)
            .map(|entry| entry.enabled)
            .ok_or_else(|| RegistryError::UnknownProvider(provider.clone()))
    }

    fn is_enabled_internal(&self, provider: &Provider) -> bool {
        self.entries
            .read()
            .expect("provider registry poisoned")
            .iter()
            .any(|entry| &entry.provider == provider && entry.enabled)
    }

    fn enabled_entries(&self) -> Vec<RegisteredProvider> {
        self.entries
            .read()
            .expect("provider registry poisoned")
            .iter()
            .filter(|entry| entry.enabled)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SnapshotKey {
    provider: String,
    account_id: String,
    metric_kind: String,
    window_kind: String,
}

impl SnapshotKey {
    fn from_snapshot(snapshot: &UsageSnapshot) -> Self {
        Self {
            provider: snapshot.provider.as_str().to_string(),
            account_id: snapshot.account_id.clone(),
            metric_kind: format!("{:?}", snapshot.metric_kind),
            window_kind: format!("{:?}", snapshot.window_kind),
        }
    }

    fn from_parts(
        provider: &Provider,
        account_id: &str,
        metric_kind: MetricKind,
        window_kind: WindowKind,
    ) -> Self {
        Self {
            provider: provider.as_str().to_string(),
            account_id: account_id.to_string(),
            metric_kind: format!("{metric_kind:?}"),
            window_kind: format!("{window_kind:?}"),
        }
    }
}

/// Why [`SnapshotCache::try_store_live`] refused to write a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLiveReject {
    /// Snapshot failed provider-neutral contract validation.
    InvalidContract,
    /// Only `freshness=live` snapshots may enter the cache.
    NotLive,
    /// Live snapshots must not carry an error payload.
    HasError,
    /// An existing cache entry already has a newer `observed_at`.
    OlderThanCached,
}

/// Outcome of attempting to store a live snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLiveResult {
    Stored,
    Rejected(StoreLiveReject),
}

/// In-memory cache keyed by the complete provider snapshot identity.
#[derive(Clone, Default)]
pub struct SnapshotCache {
    entries: Arc<Mutex<BTreeMap<SnapshotKey, UsageSnapshot>>>,
}

impl SnapshotCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a successful live snapshot only when it is newer than the cached value.
    pub fn store_live(&self, snapshot: UsageSnapshot) -> bool {
        matches!(self.try_store_live(snapshot), StoreLiveResult::Stored)
    }

    /// Store a live snapshot, reporting why the write was rejected when it is not stored.
    pub fn try_store_live(&self, snapshot: UsageSnapshot) -> StoreLiveResult {
        // Check shape gates first so callers see the most specific reject reason.
        if snapshot.freshness != Freshness::Live {
            return StoreLiveResult::Rejected(StoreLiveReject::NotLive);
        }
        if snapshot.error.is_some() {
            return StoreLiveResult::Rejected(StoreLiveReject::HasError);
        }
        if snapshot.validate().is_err() {
            return StoreLiveResult::Rejected(StoreLiveReject::InvalidContract);
        }

        let key = SnapshotKey::from_snapshot(&snapshot);
        let mut entries = self.entries.lock().expect("snapshot cache poisoned");
        if entries
            .get(&key)
            .is_some_and(|existing| existing.observed_at > snapshot.observed_at)
        {
            return StoreLiveResult::Rejected(StoreLiveReject::OlderThanCached);
        }
        entries.insert(key, snapshot);
        StoreLiveResult::Stored
    }

    pub fn get(
        &self,
        provider: &Provider,
        account_id: &str,
        metric_kind: MetricKind,
        window_kind: WindowKind,
    ) -> Option<UsageSnapshot> {
        let key = SnapshotKey::from_parts(provider, account_id, metric_kind, window_kind);
        self.entries
            .lock()
            .expect("snapshot cache poisoned")
            .get(&key)
            .cloned()
    }

    fn all(&self) -> Vec<UsageSnapshot> {
        self.entries
            .lock()
            .expect("snapshot cache poisoned")
            .values()
            .cloned()
            .collect()
    }

    fn for_provider(&self, provider: &Provider) -> Vec<UsageSnapshot> {
        self.entries
            .lock()
            .expect("snapshot cache poisoned")
            .values()
            .filter(|snapshot| &snapshot.provider == provider)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshDiagnostic {
    pub provider: Provider,
    pub freshness: Freshness,
    pub error_code: Option<ErrorCode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshReport {
    pub snapshots: Vec<UsageSnapshot>,
    pub diagnostics: Vec<RefreshDiagnostic>,
}

#[derive(Clone)]
pub struct RefreshService {
    registry: ProviderRegistry,
    cache: SnapshotCache,
    policy: RefreshPolicy,
    clock: Arc<dyn Clock>,
}

impl RefreshService {
    pub fn new(registry: ProviderRegistry, policy: RefreshPolicy) -> Self {
        Self::with_clock(registry, policy, Arc::new(SystemClock))
    }

    pub fn with_clock(
        registry: ProviderRegistry,
        policy: RefreshPolicy,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            registry,
            cache: SnapshotCache::new(),
            policy: policy.normalized(),
            clock,
        }
    }

    pub fn cache(&self) -> SnapshotCache {
        self.cache.clone()
    }

    /// Return every provider known to this service, including disabled and
    /// not-configured entries so the shell can offer a complete control panel.
    pub fn registered_providers(&self) -> Vec<Provider> {
        self.registry.registered_providers()
    }

    /// Update one provider's refresh enablement without rebuilding the service.
    pub fn set_provider_enabled(
        &self,
        provider: &Provider,
        enabled: bool,
    ) -> Result<(), RegistryError> {
        self.registry.set_enabled(provider, enabled)
    }

    /// Return whether a provider is currently enabled for refresh.
    pub fn provider_enabled(&self, provider: &Provider) -> Result<bool, RegistryError> {
        self.registry.is_enabled(provider)
    }

    pub fn refresh_all(&self) -> Vec<UsageSnapshot> {
        self.refresh_all_with_report().snapshots
    }

    pub fn refresh_all_with_report(&self) -> RefreshReport {
        let now = self.clock.now();
        let entries = self.registry.enabled_entries();
        let mut runs = Vec::new();

        for (batch_index, batch) in entries.chunks(self.policy.max_concurrency).enumerate() {
            let handles: Vec<_> = batch
                .iter()
                .cloned()
                .map(|entry| {
                    let cache = self.cache.clone();
                    let policy = self.policy;
                    thread::spawn(move || run_provider(entry, cache, policy, now))
                })
                .collect();

            for (entry_index, handle) in handles.into_iter().enumerate() {
                let provider_run = handle.join().unwrap_or_else(|_| ProviderRun {
                    snapshots: vec![unavailable_snapshot(
                        &entries[batch_index * self.policy.max_concurrency + entry_index].provider,
                        now,
                        AdapterError {
                            code: ErrorCode::Unknown,
                            message: None,
                        },
                    )],
                    diagnostic: RefreshDiagnostic {
                        provider: entries[batch_index * self.policy.max_concurrency + entry_index]
                            .provider
                            .clone(),
                        freshness: Freshness::Unavailable,
                        error_code: Some(ErrorCode::Unknown),
                    },
                });
                runs.push((
                    batch_index * self.policy.max_concurrency + entry_index,
                    provider_run,
                ));
            }
        }

        runs.sort_by_key(|(index, _)| *index);
        let mut snapshots = Vec::new();
        let mut diagnostics = Vec::new();
        for (_, run) in runs {
            snapshots.extend(run.snapshots);
            diagnostics.push(run.diagnostic);
        }
        RefreshReport {
            snapshots,
            diagnostics,
        }
    }

    pub fn cached_snapshots(&self) -> Vec<UsageSnapshot> {
        let now = self.clock.now();
        self.cache
            .all()
            .into_iter()
            .filter(|snapshot| self.registry.is_enabled_internal(&snapshot.provider))
            .filter_map(|snapshot| {
                if age_since(now, snapshot.observed_at) > self.policy.stale_after {
                    None
                } else {
                    Some(classify_cached(snapshot, now, self.policy))
                }
            })
            .collect()
    }
}

struct ProviderRun {
    snapshots: Vec<UsageSnapshot>,
    diagnostic: RefreshDiagnostic,
}

fn run_provider(
    entry: RegisteredProvider,
    cache: SnapshotCache,
    policy: RefreshPolicy,
    now: DateTime<Utc>,
) -> ProviderRun {
    let provider = entry.provider;
    let Some(adapter) = entry.adapter else {
        return ProviderRun {
            snapshots: vec![not_configured_snapshot(&provider, now)],
            diagnostic: RefreshDiagnostic {
                provider,
                freshness: Freshness::NotConfigured,
                error_code: None,
            },
        };
    };

    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(adapter.fetch());
    });

    let result = match receiver.recv_timeout(policy.provider_timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(AdapterError {
            code: ErrorCode::Timeout,
            message: None,
        }),
        Err(RecvTimeoutError::Disconnected) => Err(AdapterError {
            code: ErrorCode::Unknown,
            message: None,
        }),
    };

    match result {
        Ok(snapshots) if snapshots.is_empty() => fallback(
            &provider,
            cache,
            policy,
            now,
            AdapterError {
                code: ErrorCode::SchemaDrift,
                message: None,
            },
        ),
        Ok(snapshots)
            if snapshots
                .iter()
                .any(|snapshot| snapshot.provider != provider) =>
        {
            fallback(
                &provider,
                cache,
                policy,
                now,
                AdapterError {
                    code: ErrorCode::SchemaDrift,
                    message: None,
                },
            )
        }
        Ok(snapshots) => successful_refresh(&provider, snapshots, cache, policy, now),
        Err(error) => fallback(&provider, cache, policy, now, error),
    }
}

fn successful_refresh(
    provider: &Provider,
    snapshots: Vec<UsageSnapshot>,
    cache: SnapshotCache,
    policy: RefreshPolicy,
    now: DateTime<Utc>,
) -> ProviderRun {
    // Validate each raw adapter snapshot before redaction so contradictory
    // freshness/error pairs cannot be rewritten into a successful shape.
    // Invalid windows become redacted schema_drift (or stale cache for that
    // key); valid windows are still stored and rendered.
    let mut output = Vec::new();
    let mut saw_schema_drift = false;

    for snapshot in snapshots {
        if let Err(_reason) = snapshot.validate() {
            saw_schema_drift = true;
            output.push(invalid_window_outcome(
                provider, &snapshot, &cache, policy, now,
            ));
            continue;
        }

        let snapshot = sanitize_adapter_snapshot(snapshot);
        if snapshot.freshness != Freshness::Live {
            output.push(snapshot);
            continue;
        }

        let key = SnapshotKey::from_snapshot(&snapshot);
        match cache.try_store_live(snapshot) {
            StoreLiveResult::Stored => {
                let effective = cache
                    .entries
                    .lock()
                    .expect("snapshot cache poisoned")
                    .get(&key)
                    .cloned()
                    .expect("live snapshot was stored");
                output.push(effective);
            }
            StoreLiveResult::Rejected(StoreLiveReject::OlderThanCached) => {
                let effective = cache
                    .entries
                    .lock()
                    .expect("snapshot cache poisoned")
                    .get(&key)
                    .cloned()
                    .expect("newer live snapshot remains cached");
                output.push(classify_cached(effective, now, policy));
            }
            StoreLiveResult::Rejected(reason) => {
                // validate() already passed; NotLive/HasError/InvalidContract
                // here means an invariant was broken after sanitization.
                saw_schema_drift = true;
                let _ = reason;
                output.push(unavailable_snapshot(
                    provider,
                    now,
                    AdapterError {
                        code: ErrorCode::SchemaDrift,
                        message: None,
                    },
                ));
            }
        }
    }

    let freshness = aggregate_freshness(&output);
    ProviderRun {
        snapshots: output,
        diagnostic: RefreshDiagnostic {
            provider: provider.clone(),
            freshness,
            error_code: saw_schema_drift.then_some(ErrorCode::SchemaDrift),
        },
    }
}

/// Prefer a still-fresh cached window for an invalid adapter result; otherwise
/// emit a redacted unavailable snapshot for that window identity.
fn invalid_window_outcome(
    provider: &Provider,
    snapshot: &UsageSnapshot,
    cache: &SnapshotCache,
    policy: RefreshPolicy,
    now: DateTime<Utc>,
) -> UsageSnapshot {
    if snapshot.has_safe_account_id() {
        if let Some(previous) = cache.get(
            &snapshot.provider,
            &snapshot.account_id,
            snapshot.metric_kind,
            snapshot.window_kind,
        ) {
            if age_since(now, previous.observed_at) <= policy.stale_after {
                return set_freshness(previous, Freshness::Stale);
            }
        }
    }

    window_schema_drift_snapshot(provider, snapshot, now)
}

fn window_schema_drift_snapshot(
    provider: &Provider,
    snapshot: &UsageSnapshot,
    now: DateTime<Utc>,
) -> UsageSnapshot {
    let account_id = if snapshot.has_safe_account_id() {
        snapshot.account_id.clone()
    } else {
        format!("{}-unavailable", provider.as_str())
    };
    let metric_kind = snapshot.metric_kind;
    let unit = if snapshot.unit.trim().is_empty() {
        "status".to_string()
    } else {
        snapshot.unit.clone()
    };

    UsageSnapshot {
        provider: provider.clone(),
        account_id,
        metric_kind,
        window_kind: snapshot.window_kind,
        unit,
        observed_at: now,
        source: Source::System,
        freshness: Freshness::Unavailable,
        confidence: crate::model::Confidence::Unknown,
        used: None,
        remaining: None,
        limit: None,
        unlimited: false,
        resets_at: None,
        window_label: snapshot.window_label.clone(),
        error: Some(AdapterError {
            code: ErrorCode::SchemaDrift,
            message: None,
        }),
    }
}

/// Redact human-readable error text. Call only after [`UsageSnapshot::validate`].
fn sanitize_adapter_snapshot(mut snapshot: UsageSnapshot) -> UsageSnapshot {
    if let Some(error) = snapshot.error.take() {
        snapshot.error = (snapshot.freshness == Freshness::Unavailable).then_some(AdapterError {
            code: error.code,
            message: None,
        });
    }
    snapshot
}

fn fallback(
    provider: &Provider,
    cache: SnapshotCache,
    policy: RefreshPolicy,
    now: DateTime<Utc>,
    error: AdapterError,
) -> ProviderRun {
    // Preserve only the stable code. Adapter messages may contain paths,
    // command arguments, or provider payload text.
    let safe_error = AdapterError {
        code: error.code.clone(),
        message: None,
    };
    let previous = cache
        .for_provider(provider)
        .into_iter()
        .filter(|snapshot| age_since(now, snapshot.observed_at) <= policy.stale_after)
        .map(|snapshot| set_freshness(snapshot, Freshness::Stale))
        .collect::<Vec<_>>();

    if previous.is_empty() {
        ProviderRun {
            snapshots: vec![unavailable_snapshot(provider, now, safe_error)],
            diagnostic: RefreshDiagnostic {
                provider: provider.clone(),
                freshness: Freshness::Unavailable,
                error_code: Some(error.code),
            },
        }
    } else {
        ProviderRun {
            snapshots: previous,
            diagnostic: RefreshDiagnostic {
                provider: provider.clone(),
                freshness: Freshness::Stale,
                error_code: Some(error.code),
            },
        }
    }
}

fn not_configured_snapshot(provider: &Provider, observed_at: DateTime<Utc>) -> UsageSnapshot {
    UsageSnapshot {
        provider: provider.clone(),
        account_id: format!("{}-not-configured", provider.as_str()),
        metric_kind: MetricKind::Health,
        window_kind: WindowKind::None,
        unit: "status".to_string(),
        observed_at,
        source: Source::System,
        freshness: Freshness::NotConfigured,
        confidence: crate::model::Confidence::Unknown,
        used: None,
        remaining: None,
        limit: None,
        unlimited: false,
        resets_at: None,
        window_label: None,
        error: None,
    }
}

fn unavailable_snapshot(
    provider: &Provider,
    observed_at: DateTime<Utc>,
    error: AdapterError,
) -> UsageSnapshot {
    UsageSnapshot {
        provider: provider.clone(),
        account_id: format!("{}-unavailable", provider.as_str()),
        metric_kind: MetricKind::Health,
        window_kind: WindowKind::None,
        unit: "status".to_string(),
        observed_at,
        source: Source::System,
        freshness: Freshness::Unavailable,
        confidence: crate::model::Confidence::Unknown,
        used: None,
        remaining: None,
        limit: None,
        unlimited: false,
        resets_at: None,
        window_label: None,
        error: Some(AdapterError {
            code: error.code,
            message: None,
        }),
    }
}

fn set_freshness(mut snapshot: UsageSnapshot, freshness: Freshness) -> UsageSnapshot {
    snapshot.freshness = freshness;
    snapshot.error = None;
    snapshot
}

fn classify_cached(
    snapshot: UsageSnapshot,
    now: DateTime<Utc>,
    policy: RefreshPolicy,
) -> UsageSnapshot {
    let freshness = if age_since(now, snapshot.observed_at) <= policy.cache_ttl {
        Freshness::Cached
    } else {
        Freshness::Stale
    };
    set_freshness(snapshot, freshness)
}

fn aggregate_freshness(snapshots: &[UsageSnapshot]) -> Freshness {
    if snapshots.is_empty() {
        return Freshness::Unavailable;
    }
    if snapshots
        .iter()
        .any(|snapshot| snapshot.freshness == Freshness::Unavailable)
    {
        Freshness::Unavailable
    } else if snapshots
        .iter()
        .any(|snapshot| snapshot.freshness == Freshness::Stale)
    {
        Freshness::Stale
    } else if snapshots
        .iter()
        .any(|snapshot| snapshot.freshness == Freshness::Cached)
    {
        Freshness::Cached
    } else if snapshots
        .iter()
        .all(|snapshot| snapshot.freshness == Freshness::NotConfigured)
    {
        Freshness::NotConfigured
    } else if snapshots
        .iter()
        .all(|snapshot| snapshot.freshness == Freshness::NotApplicable)
    {
        Freshness::NotApplicable
    } else {
        Freshness::Live
    }
}

fn age_since(now: DateTime<Utc>, observed_at: DateTime<Utc>) -> Duration {
    now.signed_duration_since(observed_at)
        .to_std()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Confidence, MetricKind, Source, WindowKind};
    use chrono::{Duration as ChronoDuration, TimeZone};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct FixedClock {
        now: Arc<Mutex<DateTime<Utc>>>,
    }

    impl FixedClock {
        fn new(now: DateTime<Utc>) -> Self {
            Self {
                now: Arc::new(Mutex::new(now)),
            }
        }

        fn set(&self, now: DateTime<Utc>) {
            *self.now.lock().unwrap() = now;
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().unwrap()
        }
    }

    struct SequenceAdapter {
        provider: Provider,
        responses: Mutex<VecDeque<Result<Vec<UsageSnapshot>, AdapterError>>>,
        calls: Arc<AtomicUsize>,
        delay: Duration,
    }

    impl SequenceAdapter {
        fn new(
            provider: Provider,
            responses: Vec<Result<Vec<UsageSnapshot>, AdapterError>>,
        ) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    provider,
                    responses: Mutex::new(responses.into()),
                    calls: calls.clone(),
                    delay: Duration::ZERO,
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
            if !self.delay.is_zero() {
                thread::sleep(self.delay);
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(Vec::new()))
        }
    }

    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 2, 12, 0, 0).single().unwrap()
    }

    fn snapshot(
        provider: Provider,
        observed_at: DateTime<Utc>,
        metric_kind: MetricKind,
        window_kind: WindowKind,
        used: f64,
    ) -> UsageSnapshot {
        UsageSnapshot {
            provider,
            account_id: "test-account".into(),
            metric_kind,
            window_kind,
            unit: "percent".into(),
            observed_at,
            source: Source::Fixture,
            freshness: Freshness::Live,
            confidence: Confidence::Exact,
            used: Some(used),
            remaining: Some(100.0 - used),
            limit: Some(100.0),
            unlimited: false,
            resets_at: Some(observed_at + ChronoDuration::hours(1)),
            window_label: Some("primary".into()),
            error: None,
        }
    }

    fn service(
        registry: ProviderRegistry,
        clock: &FixedClock,
        policy: RefreshPolicy,
    ) -> RefreshService {
        RefreshService::with_clock(registry, policy, Arc::new(clock.clone()))
    }

    #[test]
    fn fake_adapter_refreshes_without_a_live_account() {
        let now = instant();
        let clock = FixedClock::new(now);
        let registry = ProviderRegistry::new();
        let (adapter, calls) = SequenceAdapter::new(
            Provider::Codex,
            vec![Ok(vec![snapshot(
                Provider::Codex,
                now,
                MetricKind::Quota,
                WindowKind::Weekly,
                25.0,
            )])],
        );
        registry.register(adapter).unwrap();

        let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.snapshots[0].freshness, Freshness::Live);
        assert_eq!(report.snapshots[0].used, Some(25.0));
        assert_eq!(report.diagnostics[0].freshness, Freshness::Live);
    }

    #[test]
    fn not_configured_and_disabled_providers_are_not_scheduled() {
        let now = instant();
        let clock = FixedClock::new(now);
        let registry = ProviderRegistry::new();
        registry.register_not_configured(Provider::Kimi).unwrap();
        let (adapter, calls) = SequenceAdapter::new(
            Provider::Codex,
            vec![Ok(vec![snapshot(
                Provider::Codex,
                now,
                MetricKind::Quota,
                WindowKind::Weekly,
                25.0,
            )])],
        );
        registry.register(adapter).unwrap();
        registry.set_enabled(&Provider::Codex, false).unwrap();

        let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.snapshots[0].freshness, Freshness::NotConfigured);
        assert!(report.snapshots[0].error.is_none());
    }

    #[test]
    fn provider_failure_preserves_last_good_value_and_marks_it_stale() {
        let first = instant();
        let second = first + ChronoDuration::minutes(2);
        let clock = FixedClock::new(first);
        let registry = ProviderRegistry::new();
        let original = snapshot(
            Provider::Codex,
            first,
            MetricKind::Quota,
            WindowKind::Weekly,
            25.0,
        );
        let reset_at = original.resets_at;
        let (adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![
                Ok(vec![original]),
                Err(AdapterError {
                    code: ErrorCode::Timeout,
                    message: Some("secret-token should not escape".into()),
                }),
            ],
        );
        registry.register(adapter).unwrap();
        let service = service(registry, &clock, RefreshPolicy::default());

        service.refresh_all();
        clock.set(second);
        let report = service.refresh_all_with_report();

        assert_eq!(report.snapshots[0].used, Some(25.0));
        assert_eq!(report.snapshots[0].freshness, Freshness::Stale);
        assert_eq!(report.snapshots[0].observed_at, first);
        assert_eq!(report.snapshots[0].resets_at, reset_at);
        assert!(report.snapshots[0].error.is_none());
        assert_eq!(report.diagnostics[0].error_code, Some(ErrorCode::Timeout));
    }

    #[test]
    fn cached_snapshots_transition_to_stale_without_changing_observed_time() {
        let first = instant();
        let clock = FixedClock::new(first);
        let registry = ProviderRegistry::new();
        let (adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![Ok(vec![snapshot(
                Provider::Codex,
                first,
                MetricKind::Quota,
                WindowKind::Weekly,
                25.0,
            )])],
        );
        registry.register(adapter).unwrap();
        let policy = RefreshPolicy {
            cache_ttl: Duration::from_secs(60),
            stale_after: Duration::from_secs(300),
            ..RefreshPolicy::default()
        };
        let service = service(registry, &clock, policy);
        service.refresh_all();

        clock.set(first + ChronoDuration::seconds(30));
        let cached = service.cached_snapshots();
        assert_eq!(cached[0].freshness, Freshness::Cached);
        assert_eq!(cached[0].observed_at, first);

        clock.set(first + ChronoDuration::seconds(90));
        let stale = service.cached_snapshots();
        assert_eq!(stale[0].freshness, Freshness::Stale);
        assert_eq!(stale[0].observed_at, first);
    }

    #[test]
    fn disabled_provider_cache_is_hidden_until_reenabled() {
        let first = instant();
        let clock = FixedClock::new(first);
        let registry = ProviderRegistry::new();
        let (adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![Ok(vec![snapshot(
                Provider::Codex,
                first,
                MetricKind::Quota,
                WindowKind::Weekly,
                25.0,
            )])],
        );
        registry.register(adapter).unwrap();
        let service = service(registry.clone(), &clock, RefreshPolicy::default());

        service.refresh_all();
        registry.set_enabled(&Provider::Codex, false).unwrap();
        assert!(service.cached_snapshots().is_empty());

        registry.set_enabled(&Provider::Codex, true).unwrap();
        let cached = service.cached_snapshots();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].freshness, Freshness::Cached);
        assert_eq!(cached[0].observed_at, first);
    }

    #[test]
    fn refresh_service_exposes_provider_controls() {
        let registry = ProviderRegistry::new();
        registry.register_not_configured(Provider::Kimi).unwrap();
        let service = RefreshService::new(registry, RefreshPolicy::default());

        assert_eq!(service.registered_providers(), vec![Provider::Kimi]);
        assert!(service.provider_enabled(&Provider::Kimi).unwrap());

        service
            .set_provider_enabled(&Provider::Kimi, false)
            .unwrap();
        assert!(!service.provider_enabled(&Provider::Kimi).unwrap());
        assert_eq!(
            service.provider_enabled(&Provider::Codex),
            Err(RegistryError::UnknownProvider(Provider::Codex))
        );
        assert_eq!(
            service.set_provider_enabled(&Provider::Codex, true),
            Err(RegistryError::UnknownProvider(Provider::Codex))
        );
    }

    #[test]
    fn cache_expiry_turns_a_failed_refresh_into_unavailable() {
        let first = instant();
        let clock = FixedClock::new(first);
        let registry = ProviderRegistry::new();
        let original = snapshot(
            Provider::Codex,
            first,
            MetricKind::Quota,
            WindowKind::Weekly,
            25.0,
        );
        let (adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![
                Ok(vec![original]),
                Err(AdapterError {
                    code: ErrorCode::Timeout,
                    message: None,
                }),
            ],
        );
        registry.register(adapter).unwrap();
        let policy = RefreshPolicy {
            cache_ttl: Duration::from_secs(30),
            stale_after: Duration::from_secs(60),
            ..RefreshPolicy::default()
        };
        let service = service(registry, &clock, policy);
        service.refresh_all();

        clock.set(first + ChronoDuration::seconds(120));
        let report = service.refresh_all_with_report();

        assert_eq!(report.snapshots[0].freshness, Freshness::Unavailable);
        assert_eq!(report.diagnostics[0].error_code, Some(ErrorCode::Timeout));
    }

    #[test]
    fn older_observations_cannot_replace_newer_cache_entries() {
        let cache = SnapshotCache::new();
        let newer = snapshot(
            Provider::Codex,
            instant() + ChronoDuration::minutes(2),
            MetricKind::Quota,
            WindowKind::Weekly,
            70.0,
        );
        let older = snapshot(
            Provider::Codex,
            instant(),
            MetricKind::Quota,
            WindowKind::Weekly,
            20.0,
        );

        assert!(cache.store_live(newer));
        assert!(!cache.store_live(older));
        assert_eq!(
            cache
                .get(
                    &Provider::Codex,
                    "test-account",
                    MetricKind::Quota,
                    WindowKind::Weekly,
                )
                .unwrap()
                .used,
            Some(70.0)
        );
    }

    #[test]
    fn cache_key_keeps_provider_metric_and_window_dimensions_separate() {
        let cache = SnapshotCache::new();
        let now = instant();
        let quota = snapshot(
            Provider::Codex,
            now,
            MetricKind::Quota,
            WindowKind::Weekly,
            20.0,
        );
        let daily = snapshot(
            Provider::Codex,
            now,
            MetricKind::Quota,
            WindowKind::Daily,
            30.0,
        );
        let credits = snapshot(
            Provider::Codex,
            now,
            MetricKind::Credits,
            WindowKind::None,
            40.0,
        );
        assert!(cache.store_live(quota));
        assert!(cache.store_live(daily));
        assert!(cache.store_live(credits));

        assert_eq!(cache.all().len(), 3);
        assert_eq!(
            cache
                .get(
                    &Provider::Codex,
                    "test-account",
                    MetricKind::Quota,
                    WindowKind::Daily,
                )
                .unwrap()
                .used,
            Some(30.0)
        );
    }

    #[test]
    fn timeout_without_cache_is_unavailable_and_diagnostics_are_redacted() {
        let now = instant();
        let clock = FixedClock::new(now);
        let registry = ProviderRegistry::new();
        let (mut adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![Err(AdapterError {
                code: ErrorCode::Timeout,
                message: Some("token=secret-token".into()),
            })],
        );
        adapter.delay = Duration::from_millis(100);
        registry.register(adapter).unwrap();
        let policy = RefreshPolicy {
            provider_timeout: Duration::from_millis(10),
            ..RefreshPolicy::default()
        };

        let report = service(registry, &clock, policy).refresh_all_with_report();

        assert_eq!(report.snapshots[0].freshness, Freshness::Unavailable);
        assert_eq!(
            report.snapshots[0].error.as_ref().unwrap().code,
            ErrorCode::Timeout
        );
        assert!(report.snapshots[0]
            .error
            .as_ref()
            .unwrap()
            .message
            .is_none());
        assert_eq!(report.diagnostics[0].error_code, Some(ErrorCode::Timeout));
    }

    #[test]
    fn refreshes_are_bounded_by_max_concurrency() {
        struct TrackingAdapter {
            provider: Provider,
            active: Arc<AtomicUsize>,
            maximum: Arc<AtomicUsize>,
        }

        impl ProviderAdapter for TrackingAdapter {
            fn provider(&self) -> Provider {
                self.provider.clone()
            }

            fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(30));
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![snapshot(
                    self.provider.clone(),
                    instant(),
                    MetricKind::Quota,
                    WindowKind::Weekly,
                    25.0,
                )])
            }
        }

        let registry = ProviderRegistry::new();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        for provider in [Provider::Codex, Provider::Kimi, Provider::OllamaCloud] {
            registry
                .register(TrackingAdapter {
                    provider,
                    active: active.clone(),
                    maximum: maximum.clone(),
                })
                .unwrap();
        }
        let policy = RefreshPolicy {
            max_concurrency: 2,
            ..RefreshPolicy::default()
        };

        let report = RefreshService::new(registry, policy).refresh_all_with_report();

        assert_eq!(report.snapshots.len(), 3);
        assert!(maximum.load(Ordering::SeqCst) <= 2);
    }
}
