mod common;

use ai_usage_bar::{
    AdapterError, Confidence, ErrorCode, Freshness, MetricKind, Provider, SnapshotValidationError,
    Source, UsageSnapshot, WindowKind,
};
use chrono::{Duration, TimeZone, Utc};
use common::{instant, metric_snapshot};

#[test]
fn contract_snapshot_round_trip_preserves_state_and_timestamps() {
    let observed_at = instant();
    let reset_at = observed_at - Duration::hours(1);
    let snapshot = UsageSnapshot {
        provider: Provider::Codex,
        account_id: "redacted-account".into(),
        metric_kind: MetricKind::Quota,
        window_kind: WindowKind::Weekly,
        unit: "percent".into(),
        observed_at,
        source: Source::Fixture,
        freshness: Freshness::Cached,
        confidence: Confidence::ReportedEstimate,
        used: Some(0.0),
        remaining: Some(100.0),
        limit: Some(100.0),
        unlimited: false,
        resets_at: Some(reset_at),
        window_label: Some("primary".into()),
        error: None,
    };

    let encoded = serde_json::to_string(&snapshot).expect("contract serialization should work");
    let decoded: UsageSnapshot =
        serde_json::from_str(&encoded).expect("contract deserialization should work");

    assert_eq!(decoded, snapshot, "contract: cache round trip changed data");
    assert_eq!(decoded.observed_at, observed_at);
    assert_eq!(decoded.resets_at, Some(reset_at));
    assert!(decoded.validate().is_ok());
}

#[test]
fn contract_distinguishes_missing_zero_and_unlimited_values() {
    let missing = metric_snapshot(
        Provider::OllamaLocal,
        instant(),
        MetricKind::Tokens,
        WindowKind::Session,
        "tokens",
        None,
        None,
        None,
        Freshness::Live,
        None,
    );
    let zero = metric_snapshot(
        Provider::OllamaLocal,
        instant(),
        MetricKind::Tokens,
        WindowKind::Session,
        "tokens",
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Freshness::Live,
        None,
    );
    let mut unlimited = metric_snapshot(
        Provider::Codex,
        instant(),
        MetricKind::Credits,
        WindowKind::None,
        "credits",
        Some(12.0),
        None,
        None,
        Freshness::Live,
        None,
    );
    unlimited.unlimited = true;

    assert!(missing.used.is_none(), "contract: missing is not zero");
    assert_eq!(zero.used, Some(0.0), "contract: zero was lost");
    assert!(unlimited.unlimited, "contract: unlimited flag was lost");
    assert!(missing.validate().is_ok());
    assert!(zero.validate().is_ok());
    assert!(unlimited.validate().is_ok());
}

#[test]
fn contract_rejects_impossible_values_with_actionable_errors() {
    let mut percentage = metric_snapshot(
        Provider::Codex,
        instant(),
        MetricKind::Quota,
        WindowKind::Weekly,
        "percent",
        Some(101.0),
        None,
        Some(100.0),
        Freshness::Live,
        Some("primary"),
    );
    assert_eq!(
        percentage.validate(),
        Err(SnapshotValidationError::PercentageOutOfRange("used"))
    );

    percentage.used = Some(f64::NAN);
    assert_eq!(
        percentage.validate(),
        Err(SnapshotValidationError::NonFiniteValue("used"))
    );

    let mut live_error = metric_snapshot(
        Provider::Codex,
        instant(),
        MetricKind::Quota,
        WindowKind::Weekly,
        "percent",
        Some(20.0),
        Some(80.0),
        Some(100.0),
        Freshness::Live,
        Some("primary"),
    );
    live_error.error = Some(AdapterError {
        code: ErrorCode::Timeout,
        message: None,
    });
    assert_eq!(
        live_error.validate(),
        Err(SnapshotValidationError::ErrorStateMismatch)
    );

    let unavailable = UsageSnapshot {
        provider: Provider::Codex,
        account_id: "codex-unavailable".into(),
        metric_kind: MetricKind::Health,
        window_kind: WindowKind::None,
        unit: "status".into(),
        observed_at: Utc.timestamp_opt(1_786_000_000, 0).unwrap(),
        source: Source::System,
        freshness: Freshness::Unavailable,
        confidence: Confidence::Unknown,
        used: None,
        remaining: None,
        limit: None,
        unlimited: false,
        resets_at: None,
        window_label: None,
        error: Some(AdapterError {
            code: ErrorCode::Timeout,
            message: Some("must be sanitized before persistence".into()),
        }),
    };
    assert!(unavailable.validate().is_ok());
}
