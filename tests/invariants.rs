mod common;

use ai_usage_bar::{
    build_tray_view, AdapterError, ErrorCode, Freshness, MetricKind, Provider, ProviderRegistry,
    RefreshPolicy, SnapshotCache, StoreLiveReject, StoreLiveResult, WindowKind,
};
use chrono::Duration;
use common::{instant, metric_snapshot, percent_snapshot, service, FixedClock, SequenceAdapter};
use proptest::prelude::*;

fn provider_strategy() -> impl Strategy<Value = Provider> {
    prop_oneof![
        Just(Provider::Codex),
        Just(Provider::Kimi),
        Just(Provider::OllamaCloud),
        Just(Provider::GrokConsumer),
        Just(Provider::GrokApi),
    ]
}
fn metric_strategy() -> impl Strategy<Value = MetricKind> {
    prop_oneof![
        Just(MetricKind::Quota),
        Just(MetricKind::Credits),
        Just(MetricKind::Spend),
        Just(MetricKind::Tokens),
        Just(MetricKind::Requests),
        Just(MetricKind::Health),
    ]
}

fn window_strategy() -> impl Strategy<Value = WindowKind> {
    prop_oneof![
        Just(WindowKind::Rolling),
        Just(WindowKind::Daily),
        Just(WindowKind::Weekly),
        Just(WindowKind::Monthly),
        Just(WindowKind::Session),
        Just(WindowKind::None),
    ]
}

fn unit_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("percent".to_string()),
        Just("tokens".to_string()),
        Just("credits".to_string()),
        proptest::string::string_regex("[a-z]{1,12}").expect("unit strategy regex should compile"),
    ]
}

fn invalid_percentage_strategy() -> impl Strategy<Value = f64> {
    prop_oneof![-1000.0f64..=-0.01, 100.01f64..=1000.0]
}

fn invalid_account_id_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("   ".to_string()),
        Just("has\nnewline".to_string()),
        Just("has\ttab".to_string()),
    ]
}

proptest! {
    #[test]
    fn invariant_generated_fields_are_contract_safe(
        provider in provider_strategy(),
        metric in metric_strategy(),
        window in window_strategy(),
        unit in unit_strategy(),
        used in prop::option::of(0.0f64..=100.0),
        remaining in prop::option::of(0.0f64..=100.0),
        limit in prop::option::of(0.0f64..=100.0),
        offset_seconds in 0i64..=86_400,
    ) {
        let (used, limit) = match (used, limit) {
            (Some(used), Some(limit)) if used > limit => (Some(limit), Some(limit)),
            pair => pair,
        };

        let snapshot = metric_snapshot(
            provider,
            instant() + Duration::seconds(offset_seconds),
            metric,
            window,
            &unit,
            used,
            remaining,
            limit,
            Freshness::Live,
            None,
        );

        prop_assert!(
            snapshot.validate().is_ok(),
            "contract: generated fields must validate: {snapshot:?}"
        );
    }

    #[test]
    fn invariant_invalid_percentages_become_schema_drift(value in invalid_percentage_strategy()) {
        let now = instant();
        let clock = FixedClock::new(now);
        let registry = ProviderRegistry::new();
        let (adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![Ok(vec![percent_snapshot(
                Provider::Codex,
                now,
                Some(value),
                Freshness::Live,
                Some("primary"),
            )])],
        );
        registry.register(adapter).unwrap();

        let report = service(registry, &clock, RefreshPolicy::default())
            .refresh_all_with_report();

        prop_assert_eq!(report.snapshots[0].freshness, Freshness::Unavailable);
        prop_assert_eq!(&report.diagnostics[0].error_code, &Some(ErrorCode::SchemaDrift));
    }

    #[test]
    fn invariant_invalid_account_ids_are_rejected(account_id in invalid_account_id_strategy()) {
        let mut snapshot = percent_snapshot(
            Provider::Codex,
            instant(),
            Some(25.0),
            Freshness::Live,
            Some("primary"),
        );
        snapshot.account_id = account_id;
        prop_assert_eq!(
            snapshot.validate(),
            Err(ai_usage_bar::SnapshotValidationError::InvalidAccountId)
        );
    }

    #[test]
    fn invariant_used_above_limit_is_rejected(
        used in 1.0f64..=1_000.0,
        limit in 0.0f64..=999.0,
    ) {
        prop_assume!(used > limit);
        let snapshot = metric_snapshot(
            Provider::Codex,
            instant(),
            MetricKind::Credits,
            WindowKind::None,
            "credits",
            Some(used),
            None,
            Some(limit),
            Freshness::Live,
            None,
        );
        prop_assert_eq!(
            snapshot.validate(),
            Err(ai_usage_bar::SnapshotValidationError::UsedExceedsLimit)
        );
    }

    #[test]
    fn invariant_cache_age_preserves_observation_time(age_seconds in 0i64..=300) {
        let observed_at = instant();
        let clock = FixedClock::new(observed_at);
        let registry = ProviderRegistry::new();
        let (adapter, _) = SequenceAdapter::new(
            Provider::Codex,
            vec![Ok(vec![percent_snapshot(
                Provider::Codex,
                observed_at,
                Some(25.0),
                Freshness::Live,
                Some("primary"),
            )])],
        );
        registry.register(adapter).unwrap();
        let policy = RefreshPolicy {
            cache_ttl: std::time::Duration::from_secs(60),
            stale_after: std::time::Duration::from_secs(600),
            ..RefreshPolicy::default()
        };
        let service = service(registry, &clock, policy);
        service.refresh_all();

        clock.set(observed_at + Duration::seconds(age_seconds));
        let snapshots = service.cached_snapshots();
        prop_assert_eq!(snapshots.len(), 1);
        prop_assert_eq!(snapshots[0].observed_at, observed_at);
        let expected = if age_seconds <= 60 {
            Freshness::Cached
        } else {
            Freshness::Stale
        };
        prop_assert_eq!(snapshots[0].freshness, expected);
    }
}

// Also covered as an acceptance scenario; kept here as a cross-layer invariant.
#[test]
fn invariant_disabled_provider_is_not_scheduled_or_rendered() {
    let now = instant();
    let clock = FixedClock::new(now);
    let registry = ProviderRegistry::new();
    let (adapter, calls) = SequenceAdapter::new(
        Provider::Codex,
        vec![Ok(vec![percent_snapshot(
            Provider::Codex,
            now,
            Some(25.0),
            Freshness::Live,
            Some("primary"),
        )])],
    );
    registry.register(adapter).unwrap();
    registry.set_enabled(&Provider::Codex, false).unwrap();

    let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();
    let view = build_tray_view(&report.snapshots);

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        report.snapshots.is_empty(),
        "contract: disabled provider ran"
    );
    assert_eq!(view.tooltip, "No provider data");
}

// Redaction is also exercised by daemon unit tests; this invariant locks the
// refresh-report surface used by the tray path.
#[test]
fn invariant_provider_errors_are_redacted_from_snapshots_and_diagnostics() {
    let now = instant();
    let clock = FixedClock::new(now);
    let registry = ProviderRegistry::new();
    let (adapter, _) = SequenceAdapter::new(
        Provider::Codex,
        vec![Err(AdapterError {
            code: ErrorCode::Network,
            message: Some("authorization=secret-token raw payload".into()),
        })],
    );
    registry.register(adapter).unwrap();

    let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();
    let rendered = format!("{report:?}");

    assert_eq!(report.snapshots[0].freshness, Freshness::Unavailable);
    assert!(report.snapshots[0]
        .error
        .as_ref()
        .unwrap()
        .message
        .is_none());
    assert_eq!(report.diagnostics[0].error_code, Some(ErrorCode::Network));
    assert!(
        !rendered.contains("secret-token"),
        "contract: raw error leaked"
    );
    assert!(
        !rendered.contains("raw payload"),
        "contract: raw payload leaked"
    );
}

#[test]
fn invariant_cache_key_keeps_provider_account_metric_and_window_separate() {
    let cache = SnapshotCache::new();
    let now = instant();
    let codex = percent_snapshot(
        Provider::Codex,
        now,
        Some(20.0),
        Freshness::Live,
        Some("primary"),
    );
    let mut kimi = codex.clone();
    kimi.provider = Provider::Kimi;
    kimi.account_id = "kimi-account".into();
    kimi.used = Some(40.0);
    kimi.remaining = Some(60.0);
    kimi.window_kind = WindowKind::Daily;

    assert!(cache.store_live(codex));
    assert!(cache.store_live(kimi));
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
        Some(20.0)
    );
    assert_eq!(
        cache
            .get(
                &Provider::Kimi,
                "kimi-account",
                MetricKind::Quota,
                WindowKind::Daily,
            )
            .unwrap()
            .used,
        Some(40.0)
    );
}

#[test]
fn invariant_store_live_reports_reject_reason() {
    let cache = SnapshotCache::new();
    let now = instant();
    let live = percent_snapshot(
        Provider::Codex,
        now,
        Some(20.0),
        Freshness::Live,
        Some("primary"),
    );

    assert_eq!(cache.try_store_live(live.clone()), StoreLiveResult::Stored);

    let mut not_live = live.clone();
    not_live.freshness = Freshness::Cached;
    assert_eq!(
        cache.try_store_live(not_live),
        StoreLiveResult::Rejected(StoreLiveReject::NotLive)
    );

    let mut with_error = live.clone();
    with_error.error = Some(AdapterError {
        code: ErrorCode::Timeout,
        message: None,
    });
    assert_eq!(
        cache.try_store_live(with_error),
        StoreLiveResult::Rejected(StoreLiveReject::HasError)
    );

    let mut invalid = live.clone();
    invalid.used = Some(150.0);
    assert_eq!(
        cache.try_store_live(invalid),
        StoreLiveResult::Rejected(StoreLiveReject::InvalidContract)
    );

    let older = percent_snapshot(
        Provider::Codex,
        now - Duration::minutes(1),
        Some(10.0),
        Freshness::Live,
        Some("primary"),
    );
    assert_eq!(
        cache.try_store_live(older),
        StoreLiveResult::Rejected(StoreLiveReject::OlderThanCached)
    );
}

#[test]
fn invariant_one_invalid_window_does_not_drop_valid_siblings() {
    let now = instant();
    let clock = FixedClock::new(now);
    let registry = ProviderRegistry::new();
    let primary = percent_snapshot(
        Provider::Codex,
        now,
        Some(25.0),
        Freshness::Live,
        Some("primary"),
    );
    let mut secondary = percent_snapshot(
        Provider::Codex,
        now,
        Some(101.0),
        Freshness::Live,
        Some("secondary"),
    );
    secondary.window_kind = WindowKind::Daily;
    let (adapter, _) = SequenceAdapter::new(Provider::Codex, vec![Ok(vec![primary, secondary])]);
    registry.register(adapter).unwrap();

    let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();

    assert_eq!(report.snapshots.len(), 2);
    let primary = report
        .snapshots
        .iter()
        .find(|snapshot| snapshot.window_label.as_deref() == Some("primary"))
        .expect("primary window kept");
    let secondary = report
        .snapshots
        .iter()
        .find(|snapshot| snapshot.window_label.as_deref() == Some("secondary"))
        .expect("secondary window present");
    assert_eq!(primary.freshness, Freshness::Live);
    assert_eq!(primary.used, Some(25.0));
    assert_eq!(secondary.freshness, Freshness::Unavailable);
    assert_eq!(
        secondary.error.as_ref().map(|error| error.code.clone()),
        Some(ErrorCode::SchemaDrift)
    );
    assert_eq!(
        report.diagnostics[0].error_code,
        Some(ErrorCode::SchemaDrift)
    );
}

#[test]
fn invariant_cached_with_error_is_schema_drift_not_healed() {
    let now = instant();
    let clock = FixedClock::new(now);
    let registry = ProviderRegistry::new();
    let mut snapshot = percent_snapshot(
        Provider::Codex,
        now,
        Some(40.0),
        Freshness::Cached,
        Some("primary"),
    );
    snapshot.error = Some(AdapterError {
        code: ErrorCode::Network,
        message: Some("authorization=secret-token".into()),
    });
    let (adapter, _) = SequenceAdapter::new(Provider::Codex, vec![Ok(vec![snapshot])]);
    registry.register(adapter).unwrap();

    let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();
    let rendered = format!("{report:?}");

    assert_eq!(report.snapshots[0].freshness, Freshness::Unavailable);
    assert_eq!(
        report.snapshots[0].error.as_ref().map(|error| error.code.clone()),
        Some(ErrorCode::SchemaDrift)
    );
    assert!(report.snapshots[0]
        .error
        .as_ref()
        .unwrap()
        .message
        .is_none());
    assert!(!rendered.contains("secret-token"));
    assert_eq!(
        report.diagnostics[0].error_code,
        Some(ErrorCode::SchemaDrift)
    );
}
