use ai_usage_bar::{ProviderCard, UsageSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageBand {
    Neutral,
    Green,
    Yellow,
    Red,
}

pub(crate) fn normalize_used_percent(used_percent: Option<f64>) -> Option<f64> {
    used_percent
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}

pub(crate) fn usage_band(used_percent: Option<f64>) -> UsageBand {
    match normalize_used_percent(used_percent) {
        None => UsageBand::Neutral,
        Some(percent) if percent >= 90.0 => UsageBand::Red,
        Some(percent) if percent >= 70.0 => UsageBand::Yellow,
        Some(_) => UsageBand::Green,
    }
}

pub(crate) fn render_detail_text(snapshots: &[UsageSnapshot]) -> String {
    let cards = ProviderCard::from_snapshots(snapshots);
    if cards.is_empty() {
        return "No provider data".to_string();
    }

    let mut lines = Vec::new();
    for card in cards {
        lines.push(format!("=== {} ({}) ===", card.provider, card.account_id));
        for metric in &card.metrics {
            let unit_display = if metric.unit == "percent" {
                "%"
            } else {
                metric.unit.as_str()
            };
            let resets = metric.resets_at.as_deref().unwrap_or("?");
            let value = if metric.unit == "percent" {
                format!(
                    "{}% left ({}% used)",
                    metric.remaining.as_deref().unwrap_or("?"),
                    metric.used.as_deref().unwrap_or("?")
                )
            } else {
                format!("{}{}", metric.used.as_deref().unwrap_or("?"), unit_display)
            };
            lines.push(format!(
                "  [{}] {:?} {} — {}, resets {}",
                metric.label,
                metric.metric_kind,
                metric.window_kind,
                value,
                resets
            ));
            lines.push(format!("    observed: {}", metric.observed_at));
            lines.push(format!(
                "    source: {}, confidence: {}",
                metric.source, metric.confidence
            ));
            if metric.unlimited {
                lines.push("    unlimited: true".to_string());
            }
            if let Some(error) = &metric.error {
                lines.push(format!("    error: {error}"));
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_usage_bar::{
        Confidence, ErrorCode, Freshness, MetricKind, Provider, Source, UsageSnapshot,
        WindowKind,
    };
    use chrono::Utc;

    fn make_snapshot(
        used: Option<f64>,
        remaining: Option<f64>,
        metric_kind: MetricKind,
        unit: &str,
    ) -> UsageSnapshot {
        UsageSnapshot {
            provider: Provider::Codex,
            account_id: "codex-test".into(),
            metric_kind,
            window_kind: WindowKind::Weekly,
            unit: unit.into(),
            observed_at: Utc::now(),
            source: Source::Cli,
            freshness: Freshness::Live,
            confidence: Confidence::Exact,
            used,
            remaining,
            limit: Some(100.0),
            unlimited: false,
            resets_at: None,
            window_label: Some("primary".into()),
            error: None,
        }
    }

    #[test]
    fn normalizes_percentages_and_rejects_non_finite_values() {
        assert_eq!(normalize_used_percent(Some(-5.0)), Some(0.0));
        assert_eq!(normalize_used_percent(Some(42.5)), Some(42.5));
        assert_eq!(normalize_used_percent(Some(105.0)), Some(100.0));
        assert_eq!(normalize_used_percent(Some(f64::NAN)), None);
        assert_eq!(normalize_used_percent(Some(f64::INFINITY)), None);
        assert_eq!(normalize_used_percent(None), None);
    }

    #[test]
    fn selects_usage_bands_at_widget_thresholds() {
        assert_eq!(usage_band(None), UsageBand::Neutral);
        assert_eq!(usage_band(Some(69.9)), UsageBand::Green);
        assert_eq!(usage_band(Some(70.0)), UsageBand::Yellow);
        assert_eq!(usage_band(Some(89.9)), UsageBand::Yellow);
        assert_eq!(usage_band(Some(90.0)), UsageBand::Red);
        assert_eq!(usage_band(Some(150.0)), UsageBand::Red);
    }

    #[test]
    fn detail_payload_reports_remaining_and_used_percentages() {
        let text = render_detail_text(&[make_snapshot(
            Some(17.0),
            Some(83.0),
            MetricKind::Quota,
            "percent",
        )]);

        assert!(text.contains("83% left (17% used)"));
        assert!(!text.contains("17% left"));
        assert!(text.contains("source: cli, confidence: exact"));
    }

    #[test]
    fn detail_payload_keeps_credits_unlimited_and_error_metadata() {
        let mut snapshot = make_snapshot(Some(12.0), None, MetricKind::Credits, "USD");
        snapshot.unlimited = true;
        snapshot.error = Some(ai_usage_bar::AdapterError {
            code: ErrorCode::Timeout,
            message: Some("provider did not respond".into()),
        });

        let text = render_detail_text(&[snapshot]);

        assert!(text.contains("12USD"));
        assert!(text.contains("unlimited: true"));
        assert!(text.contains("error: timeout: provider did not respond"));
    }

    #[test]
    fn empty_detail_payload_is_explicit() {
        assert_eq!(render_detail_text(&[]), "No provider data");
    }
}
