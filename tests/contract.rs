mod common;

use ai_usage_bar::{
    parse_usage_response, parse_usages_response, AdapterError, Confidence, ErrorCode, Freshness,
    KimiAdapterError, MetricKind, Provider, SnapshotValidationError, Source, UsageSnapshot,
    WindowKind,
};
use chrono::{Duration, TimeZone, Utc};
use common::{instant, metric_snapshot};

#[test]
fn contract_account_id_cache_keys_require_nonempty_noncontrol_values() {
    let mut snapshot = metric_snapshot(
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

    assert!(snapshot.has_safe_account_id());
    for invalid in ["", "   ", "bad\nid", "bad\tid"] {
        snapshot.account_id = invalid.into();
        assert!(
            !snapshot.has_safe_account_id(),
            "account id should not be cache-safe: {invalid:?}"
        );
    }
}

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
fn contract_ollama_fixture_normalizes_both_hosted_windows() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/fixtures/ollama_cloud/normal.json");
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path).expect("Ollama fixture should be readable"),
    )
    .expect("Ollama fixture should be JSON");

    let snapshots = parse_usage_response(&raw, instant(), "ollama-contract")
        .expect("Ollama totals should satisfy the adapter contract");
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].provider, Provider::OllamaCloud);
    assert_eq!(snapshots[0].window_label.as_deref(), Some("session"));
    assert_eq!(snapshots[1].window_label.as_deref(), Some("weekly"));
    assert!(snapshots.iter().all(|snapshot| {
        snapshot.source == Source::Api
            && snapshot.metric_kind == MetricKind::Quota
            && snapshot.validate().is_ok()
    }));
}

#[test]
fn contract_kimi_fixture_normalizes_weekly_rolling_and_credits() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/kimi/normal.json");
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path).expect("Kimi fixture should be readable"),
    )
    .expect("Kimi fixture should be JSON");

    let snapshots = parse_usages_response(&raw, instant(), "kimi-contract")
        .expect("Kimi usages should satisfy the adapter contract");
    assert_eq!(snapshots.len(), 3);

    let weekly = snapshots
        .iter()
        .find(|snapshot| snapshot.window_label.as_deref() == Some("primary"))
        .expect("weekly primary window present");
    assert_eq!(weekly.provider, Provider::Kimi);
    assert_eq!(weekly.metric_kind, MetricKind::Quota);
    assert_eq!(weekly.window_kind, WindowKind::Weekly);
    assert_eq!(weekly.unit, "percent");
    assert_eq!(weekly.used, Some(33.0));
    assert_eq!(weekly.remaining, Some(67.0));
    assert_eq!(weekly.limit, Some(100.0));
    assert_eq!(weekly.source, Source::Cli);

    let rolling = snapshots
        .iter()
        .find(|snapshot| snapshot.window_kind == WindowKind::Rolling)
        .expect("rolling 5-hour window present");
    assert_eq!(rolling.used, Some(2.0));

    let credits = snapshots
        .iter()
        .find(|snapshot| snapshot.metric_kind == MetricKind::Credits)
        .expect("extra usage credits snapshot present");
    assert_eq!(credits.used, Some(1250.0));
    assert_eq!(credits.remaining, None);
    assert_eq!(credits.limit, Some(1500.0));
    assert_eq!(credits.unit, "cents");

    assert!(snapshots.iter().all(|snapshot| snapshot.validate().is_ok()));
}

#[test]
fn contract_kimi_fixture_failures_map_to_redacted_codes() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |name: &str| {
        let path = manifest.join("docs/fixtures/kimi").join(name);
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).expect("Kimi fixture should be readable"),
        )
        .expect("Kimi fixture should be JSON");
        raw
    };

    let auth = parse_usages_response(&read("auth_failure.json"), instant(), "kimi-contract");
    assert!(matches!(auth, Err(KimiAdapterError::AuthExpired)));

    let timeout = parse_usages_response(&read("timeout.json"), instant(), "kimi-contract");
    assert!(matches!(timeout, Err(KimiAdapterError::Timeout)));

    let malformed = parse_usages_response(&read("malformed.json"), instant(), "kimi-contract");
    assert!(matches!(malformed, Err(KimiAdapterError::SchemaDrift(_))));
}

#[test]
fn contract_kimi_optional_total_quota_stays_a_monthly_window() {
    let raw = serde_json::json!({
        "usage": { "used": "33", "limit": "100", "remaining": "67" },
        "totalQuota": {
            "used": "12",
            "limit": "100",
            "remaining": "88",
            "resetTime": "2026-09-01T00:00:00Z"
        }
    });
    let snapshots = parse_usages_response(&raw, instant(), "kimi-contract")
        .expect("optional Kimi total should satisfy the adapter contract");
    let total = snapshots
        .iter()
        .find(|snapshot| snapshot.window_label.as_deref() == Some("total"))
        .expect("monthly total window present");
    assert_eq!(total.window_kind, WindowKind::Monthly);
    assert_eq!(total.metric_kind, MetricKind::Quota);
    assert_eq!(total.used, Some(12.0));
    assert!(total.validate().is_ok());
}

#[test]
fn contract_distinguishes_missing_zero_and_unlimited_values() {
    let missing = metric_snapshot(
        Provider::Codex,
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
        Provider::Codex,
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
    let base = metric_snapshot(
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

    let cases: Vec<(&str, UsageSnapshot, SnapshotValidationError)> = vec![
        {
            let mut snapshot = base.clone();
            snapshot.account_id = "   ".into();
            (
                "empty account_id",
                snapshot,
                SnapshotValidationError::InvalidAccountId,
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.account_id = "bad\nid".into();
            (
                "control char account_id",
                snapshot,
                SnapshotValidationError::InvalidAccountId,
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.unit = "  ".into();
            ("empty unit", snapshot, SnapshotValidationError::EmptyUnit)
        },
        {
            let mut snapshot = base.clone();
            snapshot.used = Some(f64::NAN);
            (
                "non-finite used",
                snapshot,
                SnapshotValidationError::NonFiniteValue("used"),
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.remaining = Some(f64::INFINITY);
            (
                "non-finite remaining",
                snapshot,
                SnapshotValidationError::NonFiniteValue("remaining"),
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.limit = Some(f64::NEG_INFINITY);
            (
                "non-finite limit",
                snapshot,
                SnapshotValidationError::NonFiniteValue("limit"),
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.used = Some(-0.01);
            (
                "negative used",
                snapshot,
                SnapshotValidationError::NegativeValue("used"),
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.remaining = Some(-1.0);
            (
                "negative remaining",
                snapshot,
                SnapshotValidationError::NegativeValue("remaining"),
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.used = Some(101.0);
            (
                "percentage out of range",
                snapshot,
                SnapshotValidationError::PercentageOutOfRange("used"),
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.used = Some(120.0);
            snapshot.limit = Some(100.0);
            snapshot.unit = "tokens".into();
            (
                "used exceeds limit",
                snapshot,
                SnapshotValidationError::UsedExceedsLimit,
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.freshness = Freshness::Unavailable;
            snapshot.error = Some(AdapterError {
                code: ErrorCode::Timeout,
                message: None,
            });
            snapshot.used = Some(10.0);
            snapshot.remaining = None;
            snapshot.limit = None;
            (
                "unavailable with values",
                snapshot,
                SnapshotValidationError::UnavailableHasValues,
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.error = Some(AdapterError {
                code: ErrorCode::Timeout,
                message: None,
            });
            (
                "live with error",
                snapshot,
                SnapshotValidationError::ErrorStateMismatch,
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.freshness = Freshness::Cached;
            snapshot.error = Some(AdapterError {
                code: ErrorCode::Network,
                message: Some("should not heal on sanitize".into()),
            });
            (
                "cached with error",
                snapshot,
                SnapshotValidationError::ErrorStateMismatch,
            )
        },
        {
            let mut snapshot = base.clone();
            snapshot.freshness = Freshness::Unavailable;
            snapshot.used = None;
            snapshot.remaining = None;
            snapshot.limit = None;
            snapshot.error = None;
            (
                "unavailable without error",
                snapshot,
                SnapshotValidationError::ErrorStateMismatch,
            )
        },
    ];

    for (label, snapshot, expected) in cases {
        assert_eq!(
            snapshot.validate(),
            Err(expected),
            "contract: expected {label} to fail validation"
        );
    }

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

    let mut unlimited_over_limit = base;
    unlimited_over_limit.used = Some(150.0);
    unlimited_over_limit.limit = Some(100.0);
    unlimited_over_limit.unit = "credits".into();
    unlimited_over_limit.unlimited = true;
    assert!(
        unlimited_over_limit.validate().is_ok(),
        "contract: unlimited may exceed a reported limit field"
    );
}

#[test]
fn contract_opencode_zen_fixtures_are_unreleased_deferred_and_redacted() {
    // Issue #86 spike evidence gate: every Zen balance fixture is a synthetic
    // artifact for a source that is not released, records the defer decision,
    // and contains no credential-shaped content. The fixture set and each
    // file's failure meaning are locked so the evidence cannot drift silently.
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/opencode_zen");
    let mut entries: Vec<_> = std::fs::read_dir(&directory)
        .expect("opencode_zen fixture directory should exist")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort();

    let expected_fixtures = [
        "auth_failure.json",
        "malformed.json",
        "missing_fields.json",
        "positive_balance.json",
        "rate_limited.json",
        "timeout.json",
        "unavailable.json",
        "zero_balance.json",
    ];
    let actual: Vec<String> = entries
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        actual, expected_fixtures,
        "contract: the opencode_zen fixture set must stay exactly these eight files"
    );

    let secret_patterns = [
        "Bearer ", "sk-", "oc_sk_", "eyJ", // JWT header fragment
        "@",   // email-shaped identifiers
    ];

    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("Zen fixture should be readable"),
        )
        .unwrap_or_else(|error| panic!("{name} should be valid JSON: {error}"));
        assert_eq!(
            raw.get("_status").and_then(|value| value.as_str()),
            Some("unreleased"),
            "{name} must be labeled unreleased"
        );
        assert_eq!(
            raw.get("_decision").and_then(|value| value.as_str()),
            Some("defer"),
            "{name} must record the spike defer decision"
        );
        let expected = raw
            .get("_expected")
            .unwrap_or_else(|| panic!("{name} must document its failure meaning under _expected"));
        let error_code = expected.get("error_code").and_then(|value| value.as_str());
        match name.as_str() {
            "unavailable.json" => {
                assert_eq!(
                    raw.get("_httpStatus").and_then(|value| value.as_u64()),
                    Some(404),
                    "{name}: the live 404 capture must stay an HTTP 404"
                );
                assert_eq!(
                    error_code,
                    Some("endpoint_unavailable"),
                    "{name}: a 404 route is endpoint_unavailable, not a balance"
                );
            }
            "malformed.json" => assert_eq!(
                error_code,
                Some("schema_drift"),
                "{name}: a wrong-typed success-shaped body is schema drift"
            ),
            "auth_failure.json" => {
                assert_eq!(
                    expected
                        .get("no_local_key")
                        .and_then(|value| value.as_str()),
                    Some("not_configured"),
                    "{name}: no local Zen key is not_configured, never zero"
                );
                assert_eq!(
                    expected
                        .get("key_rejected")
                        .and_then(|value| value.as_str()),
                    Some("auth_expired"),
                    "{name}: a rejected key is auth_expired"
                );
            }
            "timeout.json" => assert_eq!(
                error_code,
                Some("timeout"),
                "{name}: a network deadline is timeout"
            ),
            "rate_limited.json" => {
                assert_eq!(
                    raw.get("_httpStatus").and_then(|value| value.as_u64()),
                    Some(429),
                    "{name}: rate limiting must stay an HTTP 429"
                );
                assert_eq!(
                    error_code,
                    Some("rate_limited"),
                    "{name}: an HTTP 429 is rate_limited"
                );
            }
            "missing_fields.json" => {
                assert!(
                    raw.get("balance").is_none()
                        && raw.get("currency").is_none()
                        && raw.get("auto_reload").is_none(),
                    "{name}: absent fields must be missing keys, not JSON nulls"
                );
            }
            "positive_balance.json" | "zero_balance.json" => {
                // Balance/zero mapping distinctions are locked by
                // contract_opencode_zen_balance_mapping_preserves_zero_missing_positive.
            }
            other => panic!("unexpected fixture in the locked set: {other}"),
        }
        let serialized = raw.to_string();
        for pattern in secret_patterns {
            assert!(
                !serialized.contains(pattern),
                "{name} must not contain {pattern:?}"
            );
        }
    }
}

#[test]
fn contract_opencode_zen_balance_mapping_preserves_zero_missing_positive() {
    // Issue #86 admission mapping: the hypothetical Zen balance source would
    // map to metric_kind=credits with window_kind=none, remaining=balance,
    // and no used/limit/resets_at, percentage, or quota semantics. Missing
    // balance is absent; zero balance is a real 0.0.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read_fixture = |name: &str| {
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(manifest.join("docs/fixtures/opencode_zen").join(name))
                .expect("Zen fixture should be readable"),
        )
        .expect("Zen fixture should be JSON");
        raw
    };

    let positive = read_fixture("positive_balance.json");
    let zero = read_fixture("zero_balance.json");
    let missing = read_fixture("missing_fields.json");

    for (name, fixture) in [
        ("positive_balance.json", &positive),
        ("zero_balance.json", &zero),
        ("missing_fields.json", &missing),
    ] {
        let expected = fixture.get("_expected").expect("fixture has _expected");
        assert_eq!(
            expected.get("metric_kind").and_then(|value| value.as_str()),
            Some("credits"),
            "{name}: Zen balance is credits, never quota"
        );
        assert_eq!(
            expected.get("window_kind").and_then(|value| value.as_str()),
            Some("none"),
            "{name}: a balance is not a quota window"
        );
        assert!(
            expected.get("used").is_none_or(serde_json::Value::is_null),
            "{name}: spent is not invented when the provider does not report it"
        );
        assert!(
            expected.get("limit").is_none_or(serde_json::Value::is_null),
            "{name}: a balance is not a credit limit"
        );
        assert!(
            expected
                .get("resets_at")
                .is_none_or(serde_json::Value::is_null),
            "{name}: a balance does not reset"
        );
    }

    // Provider-reported balance is preserved as remaining.
    assert_eq!(positive["_expected"]["remaining"], serde_json::json!(42.5));
    assert_eq!(positive["balance"], serde_json::json!(42.5));
    assert_eq!(
        positive["_expected"]["unit"],
        serde_json::json!("usd"),
        "positive: reported currency becomes the unit"
    );

    // Zero balance is a real zero, distinct from a missing balance.
    assert_eq!(zero["_expected"]["remaining"], serde_json::json!(0.0));
    assert_eq!(zero["balance"], serde_json::json!(0));

    // Missing balance stays absent (missing keys, never JSON nulls) and is
    // never converted to zero.
    assert!(missing.get("balance").is_none());
    assert!(missing.get("currency").is_none());
    assert!(missing.get("auto_reload").is_none());
    assert!(missing["_expected"]["remaining"].is_null());
    assert!(missing["_expected"]["unit"].is_null());
    assert_ne!(
        zero["_expected"]["remaining"],
        missing["_expected"]["remaining"]
    );
}
