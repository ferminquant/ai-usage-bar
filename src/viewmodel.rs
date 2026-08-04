use crate::model::{Freshness, MetricKind, UsageSnapshot};

#[derive(Debug, Clone, PartialEq)]
pub struct TrayViewModel {
    pub icon_text: String,
    pub tooltip: String,
    pub used_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderCard {
    pub provider: String,
    pub account_id: String,
    pub freshness: Freshness,
    pub metrics: Vec<MetricCard>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricCard {
    pub label: String,
    pub metric_kind: MetricKind,
    pub window_kind: String,
    pub used: Option<String>,
    pub remaining: Option<String>,
    pub limit: Option<String>,
    pub unit: String,
    pub unlimited: bool,
    pub resets_at: Option<String>,
    pub observed_at: String,
    pub source: String,
    pub confidence: String,
    pub error: Option<String>,
}

impl MetricCard {
    pub fn from_snapshot(s: &UsageSnapshot) -> Self {
        let label = s.window_label.clone().unwrap_or_else(|| match s.metric_kind {
            MetricKind::Credits => "credits".into(),
            MetricKind::Health => "health".into(),
            _ => "usage".into(),
        });
        MetricCard {
            label,
            metric_kind: s.metric_kind,
            window_kind: format!("{:?}", s.window_kind).to_lowercase(),
            used: s.used.map(|v| format!("{v:.0}")),
            remaining: s.remaining.map(|v| format!("{v:.0}")),
            limit: s.limit.map(|v| format!("{v:.0}")),
            unit: s.unit.clone(),
            unlimited: s.unlimited,
            resets_at: s.resets_at.map(|t| t.to_rfc3339()),
            observed_at: s.observed_at.to_rfc3339(),
            source: format!("{:?}", s.source).to_lowercase(),
            confidence: format!("{:?}", s.confidence).to_lowercase(),
            error: s.error.as_ref().map(|e| {
                format!(
                    "{}: {}",
                    e.code.as_str(),
                    e.message.as_deref().unwrap_or("")
                )
            }),
        }
    }
}

impl ProviderCard {
    pub fn from_snapshots(snapshots: &[UsageSnapshot]) -> Vec<Self> {
        let mut providers: std::collections::BTreeMap<String, ProviderCard> = Default::default();
        for s in snapshots {
            let key = format!("{}-{}", s.provider, s.account_id);
            let card = providers.entry(key.clone()).or_insert(ProviderCard {
                provider: s.provider.to_string(),
                account_id: s.account_id.clone(),
                freshness: s.freshness,
                metrics: Vec::new(),
            });
            card.metrics.push(MetricCard::from_snapshot(s));
        }
        providers.into_values().collect()
    }
}

fn remaining_percent(metric: &MetricCard) -> Option<f64> {
    metric
        .remaining
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .or_else(|| {
            metric
                .used
                .as_deref()
                .and_then(|value| value.parse::<f64>().ok())
                .map(|used| 100.0 - used)
        })
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}

fn tooltip_metric_value(metric: &MetricCard) -> String {
    if metric.unit == "percent" {
        return remaining_percent(metric)
            .map(|remaining| format!("{remaining:.0}% left"))
            .unwrap_or_else(|| "?% left".to_string());
    }

    let unit_display = metric.unit.as_str();
    let value = metric.used.as_deref().unwrap_or("?");
    format!("{value}{unit_display}")
}

pub fn build_tray_view(snapshots: &[UsageSnapshot]) -> TrayViewModel {
    if snapshots.is_empty() {
        return TrayViewModel {
            icon_text: "—".into(),
            tooltip: "No provider data".into(),
            used_percent: None,
        };
    }

    let error_count = snapshots
        .iter()
        .filter(|s| s.freshness == Freshness::Unavailable)
        .count();

    let primary = snapshots.iter().find(|s| {
        matches!(
            s.freshness,
            Freshness::Live | Freshness::Cached | Freshness::Stale
        )
            && matches!(s.metric_kind, MetricKind::Quota | MetricKind::Credits | MetricKind::Requests)
            && s.unit == "percent"
            && s.window_label.as_deref() == Some("primary")
            && s.used.is_some()
    });

    let (icon_text, used_percent) = match primary {
        Some(s) => {
            // Bound icon percentage even if a caller bypasses validate().
            let pct = s.used.unwrap().clamp(0.0, 100.0);
            let icon = if pct >= 90.0 {
                "\u{1F534}"
            } else if pct >= 70.0 {
                "\u{1F7E1}"
            } else {
                "\u{1F7E2}"
            };
            (icon.to_string(), Some(pct))
        }
        None if error_count > 0 => ("\u{26D4}".to_string(), None),
        None => ("—".to_string(), None),
    };

    let mut lines = Vec::new();
    for card in ProviderCard::from_snapshots(snapshots) {
        let status = match card.freshness {
            Freshness::Live => "",
            Freshness::Cached => " (cached)",
            Freshness::Stale => " (stale)",
            Freshness::Unavailable => " (unavailable)",
            Freshness::NotConfigured => " (not configured)",
            Freshness::NotApplicable => " (n/a)",
        };
        lines.push(format!("{}{}", card.provider, status));
        for m in &card.metrics {
            let unit_display = if m.unit == "percent" { "%" } else { m.unit.as_str() };
            let label = &m.label;
            let win = &m.window_kind;

            let reset_str = m
                .resets_at
                .as_deref()
                .and_then(|r| chrono::DateTime::parse_from_rfc3339(r).ok())
                .map(|dt| {
                    dt.with_timezone(&chrono::Utc)
                        .format("%a %H:%M UTC")
                        .to_string()
                })
                .unwrap_or_else(|| "—".to_string());

            if m.metric_kind == MetricKind::Credits {
                let bal = m.used.as_deref().unwrap_or("?");
                let unlim = if m.unlimited { " (unlimited)" } else { "" };
                lines.push(format!("  {label}: {bal} {unit_display}{unlim}"));
            } else if let Some(error) = &m.error {
                lines.push(format!("  {label}: unavailable ({error})"));
            } else {
                lines.push(format!(
                    "  {label} {win}: {}, resets {reset_str}",
                    tooltip_metric_value(m)
                ));
            }
        }
    }
    let tooltip = if lines.len() > 8 {
        format!("{}\n...", lines[..8].join("\n"))
    } else {
        lines.join("\n")
    };

    TrayViewModel {
        icon_text,
        tooltip,
        used_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use chrono::Utc;

    fn make_snapshot(
        used: Option<f64>,
        freshness: Freshness,
        label: Option<&str>,
    ) -> UsageSnapshot {
        UsageSnapshot {
            provider: Provider::Codex,
            account_id: "codex-test".into(),
            metric_kind: MetricKind::Quota,
            window_kind: WindowKind::Weekly,
            unit: "percent".into(),
            observed_at: Utc::now(),
            source: Source::Cli,
            freshness,
            confidence: Confidence::Exact,
            used,
            remaining: used.map(|u| 100.0 - u),
            limit: Some(100.0),
            unlimited: false,
            resets_at: None,
            window_label: label.map(String::from),
            error: None,
        }
    }

    #[test]
    fn empty_snapshots_show_dash() {
        let vm = build_tray_view(&[]);
        assert_eq!(vm.icon_text, "—");
        assert!(vm.tooltip.contains("No provider"));
    }

    #[test]
    fn green_icon_for_low_usage() {
        let snaps = vec![make_snapshot(Some(40.0), Freshness::Live, Some("primary"))];
        let vm = build_tray_view(&snaps);
        assert_eq!(vm.icon_text, "🟢");
    }

    #[test]
    fn yellow_icon_for_high_usage() {
        let snaps = vec![make_snapshot(Some(75.0), Freshness::Live, Some("primary"))];
        let vm = build_tray_view(&snaps);
        assert_eq!(vm.icon_text, "🟡");
    }

    #[test]
    fn red_icon_for_critical_usage() {
        let snaps = vec![make_snapshot(Some(95.0), Freshness::Live, Some("primary"))];
        let vm = build_tray_view(&snaps);
        assert_eq!(vm.icon_text, "🔴");
    }

    #[test]
    fn error_icon_when_unavailable() {
        let mut snapshot = make_snapshot(None, Freshness::Unavailable, None);
        snapshot.error = Some(AdapterError {
            code: ErrorCode::Timeout,
            message: None,
        });
        let snaps = vec![snapshot];
        let vm = build_tray_view(&snaps);
        assert_eq!(vm.icon_text, "⛔");
        assert!(vm.tooltip.contains("timeout"));
    }

    #[test]
    fn tooltip_contains_provider_and_usage() {
        let snaps = vec![make_snapshot(Some(40.0), Freshness::Live, Some("primary"))];
        let vm = build_tray_view(&snaps);
        assert!(vm.tooltip.contains("codex"));
        assert!(vm.tooltip.contains("60% left"));
        assert!(!vm.tooltip.contains("40%"));
    }

    #[test]
    fn multiple_windows_render_separately() {
        let snaps = vec![
            make_snapshot(Some(65.0), Freshness::Live, Some("primary")),
            make_snapshot(Some(80.0), Freshness::Live, Some("secondary")),
        ];
        let cards = ProviderCard::from_snapshots(&snaps);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].metrics.len(), 2);
        assert_eq!(cards[0].metrics[0].label, "primary");
        assert_eq!(cards[0].metrics[1].label, "secondary");
    }

    #[test]
    fn stale_state_labeled_in_tooltip() {
        let snaps = vec![make_snapshot(Some(40.0), Freshness::Stale, Some("primary"))];
        let vm = build_tray_view(&snaps);
        assert_eq!(vm.icon_text, "🟢");
        assert!(vm.tooltip.contains("(stale)"));
    }

    #[test]
    fn no_aggregate_percentage_across_providers() {
        let mut s1 = make_snapshot(Some(40.0), Freshness::Live, Some("primary"));
        s1.provider = Provider::Codex;
        let mut s2 = make_snapshot(Some(60.0), Freshness::Live, Some("primary"));
        s2.provider = Provider::Kimi;
        s2.account_id = "kimi-test".into();
        let snaps = vec![s1, s2];
        let vm = build_tray_view(&snaps);
        assert!(!vm.tooltip.contains("total"));
        assert!(!vm.tooltip.contains("average"));
    }

    #[test]
    fn local_token_telemetry_is_not_a_quota_percentage() {
        let mut snapshot = make_snapshot(Some(400.0), Freshness::Live, Some("primary"));
        snapshot.provider = Provider::OllamaLocal;
        snapshot.metric_kind = MetricKind::Tokens;
        snapshot.window_kind = WindowKind::Session;
        snapshot.unit = "tokens".into();
        snapshot.limit = None;
        snapshot.remaining = None;

        let vm = build_tray_view(&[snapshot]);

        assert_eq!(vm.icon_text, "—");
        assert_eq!(vm.used_percent, None);
    }
}
