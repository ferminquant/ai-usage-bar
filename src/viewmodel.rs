use crate::model::{Freshness, MetricKind, Provider, UsageSnapshot};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct TrayViewModel {
    pub icon_text: String,
    pub tooltip: String,
    pub used_percent: Option<f64>,
    /// Short label for the pill, e.g. "Codex" / "Grok".
    pub status_label: String,
    /// Provider currently driving the compact pill (if any).
    pub focus_provider: Option<Provider>,
    /// Providers that can be selected for the compact pill.
    pub switchable_providers: Vec<Provider>,
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

/// Human-readable reset with remaining days/hours, e.g. `3d 5h left · Tue 13:28 UTC`.
pub fn format_reset_label(resets_at: Option<&str>, now: DateTime<Utc>) -> String {
    let Some(dt) = resets_at
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
    else {
        return "—".to_string();
    };

    let when = dt.format("%a %H:%M UTC");
    let remaining = dt.signed_duration_since(now);
    if remaining.num_seconds() <= 0 {
        return format!("{when} (reset due)");
    }

    let days = remaining.num_days();
    let hours = remaining.num_hours() % 24;
    let minutes = remaining.num_minutes() % 60;
    let countdown = if days > 0 {
        format!("{days}d {hours}h left")
    } else if remaining.num_hours() > 0 {
        format!("{}h {minutes}m left", remaining.num_hours())
    } else {
        format!("{}m left", remaining.num_minutes().max(1))
    };
    format!("{countdown} · {when}")
}

/// Short UI name for the compact pill label.
pub fn provider_display_name(provider: &Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::Kimi => "Kimi",
        Provider::OllamaLocal => "Ollama",
        Provider::OllamaCloud => "Ollama cloud",
        Provider::GrokConsumer => "Grok",
        Provider::GrokApi => "Grok API",
    }
}

fn is_compact_candidate(s: &UsageSnapshot) -> bool {
    matches!(
        s.freshness,
        Freshness::Live | Freshness::Cached | Freshness::Stale
    ) && matches!(
        s.metric_kind,
        MetricKind::Quota | MetricKind::Credits | MetricKind::Requests
    ) && s.unit == "percent"
        && s.window_label.as_deref() == Some("primary")
        && s.used.is_some()
}

/// Compact tray view using the first eligible primary percentage window.
pub fn build_tray_view(snapshots: &[UsageSnapshot]) -> TrayViewModel {
    build_tray_view_focused(snapshots, None, Utc::now())
}

/// Compact tray view with an optional focused provider for the main pill.
pub fn build_tray_view_focused(
    snapshots: &[UsageSnapshot],
    focus: Option<&Provider>,
    now: DateTime<Utc>,
) -> TrayViewModel {
    if snapshots.is_empty() {
        return TrayViewModel {
            icon_text: "—".into(),
            tooltip: "No provider data".into(),
            used_percent: None,
            status_label: "No data".into(),
            focus_provider: None,
            switchable_providers: Vec::new(),
        };
    }

    let error_count = snapshots
        .iter()
        .filter(|s| s.freshness == Freshness::Unavailable)
        .count();

    let mut switchable = Vec::new();
    for snapshot in snapshots.iter().filter(|s| is_compact_candidate(s)) {
        if !switchable.contains(&snapshot.provider) {
            switchable.push(snapshot.provider.clone());
        }
    }

    let primary = focus
        .and_then(|wanted| {
            snapshots
                .iter()
                .find(|s| is_compact_candidate(s) && &s.provider == wanted)
        })
        .or_else(|| snapshots.iter().find(|s| is_compact_candidate(s)));

    let (icon_text, used_percent, status_label, focus_provider) = match primary {
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
            let label = provider_display_name(&s.provider).to_string();
            (icon.to_string(), Some(pct), label, Some(s.provider.clone()))
        }
        None if error_count > 0 => ("\u{26D4}".to_string(), None, "—".into(), None),
        None => ("—".to_string(), None, "—".into(), None),
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
            let unit_display = if m.unit == "percent" {
                "%"
            } else {
                m.unit.as_str()
            };
            let label = &m.label;
            let win = &m.window_kind;
            let reset_str = format_reset_label(m.resets_at.as_deref(), now);

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
        status_label,
        focus_provider,
        switchable_providers: switchable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use chrono::{Duration, TimeZone};

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
        assert_eq!(vm.status_label, "Codex");
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

    #[test]
    fn focus_provider_selects_grok_for_compact_pill() {
        let mut codex = make_snapshot(Some(40.0), Freshness::Live, Some("primary"));
        codex.provider = Provider::Codex;
        let mut grok = make_snapshot(Some(11.0), Freshness::Live, Some("primary"));
        grok.provider = Provider::GrokConsumer;
        grok.account_id = "grok-test".into();
        let snaps = vec![codex, grok];

        let auto = build_tray_view_focused(&snaps, None, Utc::now());
        assert_eq!(auto.used_percent, Some(40.0));
        assert_eq!(auto.status_label, "Codex");

        let focused = build_tray_view_focused(&snaps, Some(&Provider::GrokConsumer), Utc::now());
        assert_eq!(focused.used_percent, Some(11.0));
        assert_eq!(focused.status_label, "Grok");
        assert_eq!(focused.focus_provider, Some(Provider::GrokConsumer));
        assert_eq!(focused.switchable_providers.len(), 2);
    }

    #[test]
    fn reset_label_includes_days_remaining() {
        let now = Utc.with_ymd_and_hms(2026, 8, 4, 12, 0, 0).unwrap();
        let reset = (now + Duration::days(3) + Duration::hours(5)).to_rfc3339();
        let label = format_reset_label(Some(&reset), now);
        assert!(label.contains("3d 5h left"), "{label}");
        assert!(label.contains("UTC"), "{label}");
    }
}
