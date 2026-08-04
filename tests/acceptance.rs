mod common;

use ai_usage_bar::{
    build_tray_view, AdapterError, ErrorCode, Freshness, Provider, ProviderRegistry, RefreshPolicy,
};
use chrono::Duration;
use common::{instant, percent_snapshot, service, FixedClock, SequenceAdapter};

#[test]
fn scenario_stale_provider_value_remains_visible_after_refresh_failure() {
    // Scenario: A stale provider value is visible as stale
    // Given the last successful snapshot is older than the freshness policy
    let observed_at = instant();
    let clock = FixedClock::new(observed_at);
    let registry = ProviderRegistry::new();
    let reset_at = observed_at + Duration::days(7);
    let (adapter, _) = SequenceAdapter::new(
        Provider::Codex,
        vec![
            Ok(vec![percent_snapshot(
                Provider::Codex,
                observed_at,
                Some(25.0),
                Freshness::Live,
                Some("primary"),
            )]),
            Err(AdapterError {
                code: ErrorCode::Timeout,
                message: Some("provider token must not escape".into()),
            }),
        ],
    );
    registry.register(adapter).unwrap();
    let service = service(registry, &clock, RefreshPolicy::default());
    service.refresh_all();

    // When the desktop bar refreshes after the provider fails
    clock.set(observed_at + Duration::minutes(2));
    let report = service.refresh_all_with_report();
    let view = build_tray_view(&report.snapshots);

    // Then the provider card shows the last value with a stale label
    assert_eq!(report.snapshots[0].freshness, Freshness::Stale);
    assert_eq!(report.snapshots[0].used, Some(25.0));
    assert_eq!(report.snapshots[0].observed_at, observed_at);
    assert_eq!(report.snapshots[0].resets_at, Some(reset_at));
    assert!(view.tooltip.contains("(stale)"));
    assert!(view.tooltip.contains("75% left"));
}

#[test]
fn scenario_failed_provider_without_cache_is_explicitly_unavailable() {
    // Scenario: A configured provider cannot be reached on the first refresh
    // Given there is no previous snapshot
    let now = instant();
    let clock = FixedClock::new(now);
    let registry = ProviderRegistry::new();
    let (adapter, _) = SequenceAdapter::new(
        Provider::Codex,
        vec![Err(AdapterError {
            code: ErrorCode::AuthExpired,
            message: Some("cookie=secret should be redacted".into()),
        })],
    );
    registry.register(adapter).unwrap();

    // When the desktop bar refreshes
    let report = service(registry, &clock, RefreshPolicy::default()).refresh_all_with_report();
    let view = build_tray_view(&report.snapshots);

    // Then it reports unavailable, not zero usage
    assert_eq!(report.snapshots[0].freshness, Freshness::Unavailable);
    assert_eq!(report.snapshots[0].used, None);
    assert_eq!(
        report.snapshots[0].error.as_ref().unwrap().code,
        ErrorCode::AuthExpired
    );
    assert!(view.tooltip.contains("auth_expired"));
    assert!(!view.tooltip.contains("0%"));
}

#[test]
fn scenario_disabled_provider_is_not_scheduled_or_shown() {
    // Scenario: A user disables a provider
    // Given a fake provider is registered but disabled
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

    // When the desktop bar refreshes
    let snapshots = service(registry, &clock, RefreshPolicy::default()).refresh_all();
    let view = build_tray_view(&snapshots);

    // Then it neither calls nor renders that provider
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(snapshots.is_empty());
    assert_eq!(view.tooltip, "No provider data");
}
