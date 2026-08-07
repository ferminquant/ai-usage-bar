use crate::model::{Freshness, MetricKind, Provider, UsageSnapshot};
use chrono::{DateTime, Utc};
use chrono_tz::America::Toronto;

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
        let label = s
            .window_label
            .clone()
            .unwrap_or_else(|| match s.metric_kind {
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
                provider: provider_display_name(&s.provider).to_string(),
                account_id: s.account_id.clone(),
                freshness: s.freshness,
                metrics: Vec::new(),
            });
            card.metrics.push(MetricCard::from_snapshot(s));
        }
        for card in providers.values_mut() {
            let provider = card.provider.clone();
            card.metrics
                .sort_by_key(|metric| metric_sort_priority(&provider, metric));
        }
        providers.into_values().collect()
    }
}

fn metric_sort_priority(provider: &str, metric: &MetricCard) -> u8 {
    let label = metric.label.to_ascii_lowercase();
    match provider {
        "Ollama" if label == "session" => 0,
        "Kimi" if label == "5-hour" => 0,
        "Kimi" if matches!(label.as_str(), "total" | "monthly") => 1,
        "OpenCode" if label == "5-hour" => 0,
        "OpenCode" if label == "weekly" => 1,
        "OpenCode" if matches!(label.as_str(), "monthly" | "total") => 2,
        _ if matches!(label.as_str(), "primary" | "weekly") => 2,
        _ if metric.metric_kind == MetricKind::Credits => 10,
        _ => 5,
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

fn tooltip_metric_name(provider: &str, metric: &MetricCard) -> String {
    let label = metric.label.trim();
    let window_kind = metric.window_kind.as_str();

    // Keep provider-specific aliases out of the UI. In particular, Ollama's
    // `session` and Kimi's unnamed rolling row both mean the same 5-hour
    // window, while `primary` is the provider-neutral weekly summary used by
    // Codex/Grok/Kimi.
    if label.eq_ignore_ascii_case("session") || label.eq_ignore_ascii_case("5-hour") {
        return "5-hour".to_string();
    }
    if label.eq_ignore_ascii_case("total") {
        return "Total".to_string();
    }
    if label.eq_ignore_ascii_case("weekly") || label.eq_ignore_ascii_case("primary") {
        return "Weekly".to_string();
    }
    if label.eq_ignore_ascii_case("monthly") {
        return if provider.eq_ignore_ascii_case("OpenCode") {
            "Monthly".to_string()
        } else {
            "Total".to_string()
        };
    }

    // A provider-native label can already include the window name. Avoid
    // rendering redundant text such as "weekly weekly" in that case.
    if label.eq_ignore_ascii_case(window_kind) {
        return match window_kind {
            "weekly" => "Weekly".to_string(),
            "monthly" if provider.eq_ignore_ascii_case("OpenCode") => "Monthly".to_string(),
            "monthly" => "Total".to_string(),
            "rolling" => "Rolling".to_string(),
            "daily" => "Daily".to_string(),
            "session" => "5-hour".to_string(),
            other => other.to_string(),
        };
    }

    format!("{label} {window_kind}")
}

/// Canonical label used by the Windows context menu for provider windows.
/// This intentionally accepts the stored/provider-native aliases so cached
/// snapshots from an older build remain selectable.
pub fn window_display_name(provider: &Provider, window: &str) -> String {
    match (provider, window.to_ascii_lowercase().as_str()) {
        (Provider::OllamaCloud, "session") | (Provider::Kimi, "5-hour") => "5-hour".into(),
        (Provider::OpenCodeGo, "5-hour") => "5-hour".into(),
        (Provider::OpenCodeGo, "monthly") => "Monthly".into(),
        (_, "weekly") | (_, "primary") => "Weekly".into(),
        (_, "total") | (_, "monthly") => "Total".into(),
        (_, "rolling") => "Rolling".into(),
        (_, "daily") => "Daily".into(),
        _ => window.to_string(),
    }
}

/// Human-readable reset with remaining days/hours in Eastern local time,
/// e.g. `3d 5h left · Tue 13:28 EDT`.
pub fn format_reset_label(resets_at: Option<&str>, now: DateTime<Utc>) -> String {
    let Some(dt) = resets_at
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
    else {
        return "—".to_string();
    };

    let when = dt.with_timezone(&Toronto).format("%a %H:%M %Z");
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
        Provider::OllamaCloud => "Ollama",
        Provider::GrokConsumer => "Grok",
        Provider::GrokApi => "Grok API",
        Provider::OpenCodeGo => "OpenCode",
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
        && match s.provider {
            Provider::OllamaCloud => matches!(s.window_label.as_deref(), Some("session" | "weekly")),
            Provider::Kimi => matches!(
                s.window_label.as_deref(),
                Some("5-hour" | "weekly" | "primary")
            ),
            Provider::OpenCodeGo => matches!(
                s.window_label.as_deref(),
                Some("5-hour" | "weekly" | "monthly")
            ),
            _ => s.window_label.as_deref() == Some("primary"),
        }
        && s.used.is_some()
}

fn is_window_candidate(s: &UsageSnapshot, window: &str) -> bool {
    matches!(
        s.freshness,
        Freshness::Live | Freshness::Cached | Freshness::Stale
    ) && matches!(
        s.metric_kind,
        MetricKind::Quota | MetricKind::Credits | MetricKind::Requests
    ) && s.unit == "percent"
        && s.used.is_some()
        && window_matches_snapshot(s, window)
}

fn window_matches_snapshot(snapshot: &UsageSnapshot, window: &str) -> bool {
    match snapshot.provider {
        Provider::OllamaCloud => {
            matches!(window, "session" | "weekly")
                && snapshot.window_label.as_deref() == Some(window)
        }
        Provider::Kimi => match window {
            "5-hour" => snapshot.window_label.as_deref() == Some("5-hour"),
            "weekly" => matches!(snapshot.window_label.as_deref(), Some("weekly" | "primary")),
            "total" => matches!(snapshot.window_label.as_deref(), Some("total" | "monthly")),
            _ => false,
        },
        Provider::OpenCodeGo => match window {
            "5-hour" => snapshot.window_label.as_deref() == Some("5-hour"),
            "weekly" => snapshot.window_label.as_deref() == Some("weekly"),
            "monthly" | "total" => {
                matches!(snapshot.window_label.as_deref(), Some("monthly" | "total"))
            }
            _ => false,
        },
        _ => window == "primary" && snapshot.window_label.as_deref() == Some("primary"),
    }
}

fn compact_priority(snapshot: &UsageSnapshot) -> u8 {
    match snapshot.provider {
        Provider::OllamaCloud if snapshot.window_label.as_deref() == Some("session") => 0,
        Provider::Kimi if snapshot.window_label.as_deref() == Some("5-hour") => 0,
        Provider::OpenCodeGo if snapshot.window_label.as_deref() == Some("5-hour") => 0,
        _ => 1,
    }
}

fn first_compact_candidate<'a>(
    snapshots: &'a [UsageSnapshot],
    provider: Option<&Provider>,
) -> Option<&'a UsageSnapshot> {
    let mut candidates = snapshots
        .iter()
        .filter(|snapshot| provider.is_none_or(|wanted| &snapshot.provider == wanted))
        .filter(|snapshot| is_compact_candidate(snapshot));
    if provider.is_some() {
        candidates.min_by_key(|snapshot| compact_priority(snapshot))
    } else {
        // Preserve registry/adapter order for the global auto-selected pill.
        // Provider-specific defaults (Kimi/Ollama 5-hour) apply only after a
        // user focuses that provider.
        candidates.next()
    }
}

/// Compact tray view using the first eligible default percentage window.
pub fn build_tray_view(snapshots: &[UsageSnapshot]) -> TrayViewModel {
    build_tray_view_focused(snapshots, None, Utc::now())
}

/// Compact tray view with an optional focused provider for the main pill.
pub fn build_tray_view_focused(
    snapshots: &[UsageSnapshot],
    focus: Option<&Provider>,
    now: DateTime<Utc>,
) -> TrayViewModel {
    build_tray_view_focused_window(snapshots, focus, None, now)
}

/// Compact tray view with an optional focused provider and quota window.
///
/// Ollama's 5-hour session and Kimi's 5-hour rolling window are the default
/// candidates; callers can pass a canonical window label such as `weekly` or
/// `total` to select another reported quota without changing the
/// provider-neutral snapshot model.
pub fn build_tray_view_focused_window(
    snapshots: &[UsageSnapshot],
    focus: Option<&Provider>,
    focus_window: Option<&str>,
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
            focus_window.and_then(|window| {
                snapshots.iter().find(|s| {
                    is_window_candidate(s, window) && &s.provider == wanted
                })
            })
        })
        .or_else(|| first_compact_candidate(snapshots, focus))
        .or_else(|| first_compact_candidate(snapshots, None));

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
            let metric_name = tooltip_metric_name(&card.provider, m);
            let reset_str = format_reset_label(m.resets_at.as_deref(), now);

            if m.metric_kind == MetricKind::Credits {
                let bal = m.used.as_deref().unwrap_or("?");
                let unlim = if m.unlimited { " (unlimited)" } else { "" };
                lines.push(format!("  {metric_name}: {bal} {unit_display}{unlim}"));
            } else if let Some(error) = &m.error {
                lines.push(format!("  {metric_name}: unavailable ({error})"));
            } else {
                lines.push(format!(
                    "  {metric_name}: {}, resets {reset_str}",
                    tooltip_metric_value(m)
                ));
            }
        }
    }
    // Keep all currently supported providers (and several future additions)
    // visible in the hover text. A bounded fallback still protects the tray
    // from an unexpectedly huge provider/metric response.
    const MAX_TOOLTIP_LINES: usize = 32;
    let tooltip = if lines.len() > MAX_TOOLTIP_LINES {
        format!("{}\n...", lines[..MAX_TOOLTIP_LINES].join("\n"))
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
        assert!(vm.tooltip.contains("Codex"));
        assert!(vm.tooltip.contains("Weekly:"));
        assert!(!vm.tooltip.contains("primary weekly"));
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
    fn ollama_defaults_to_session_and_can_focus_weekly() {
        let mut session = make_snapshot(Some(37.0), Freshness::Live, Some("session"));
        session.provider = Provider::OllamaCloud;
        session.account_id = "ollama-test".into();
        let mut weekly = session.clone();
        weekly.used = Some(18.4);
        weekly.remaining = Some(81.6);
        weekly.window_kind = WindowKind::Weekly;
        weekly.window_label = Some("weekly".into());

        let default = build_tray_view(&[session.clone(), weekly.clone()]);
        assert_eq!(default.used_percent, Some(37.0));
        assert_eq!(default.status_label, "Ollama");
        assert_eq!(default.switchable_providers, vec![Provider::OllamaCloud]);

        let weekly_view = build_tray_view_focused_window(
            &[session, weekly],
            Some(&Provider::OllamaCloud),
            Some("weekly"),
            Utc::now(),
        );
        assert_eq!(weekly_view.used_percent, Some(18.4));
        assert_eq!(weekly_view.focus_provider, Some(Provider::OllamaCloud));
    }

    #[test]
    fn provider_cards_and_ollama_tooltip_use_short_labels() {
        let mut session = make_snapshot(Some(72.0), Freshness::Live, Some("session"));
        session.provider = Provider::OllamaCloud;
        session.account_id = "ollama-test".into();
        session.window_kind = WindowKind::Rolling;
        let mut weekly = session.clone();
        weekly.used = Some(31.0);
        weekly.remaining = Some(69.0);
        weekly.window_kind = WindowKind::Weekly;
        weekly.window_label = Some("weekly".into());

        let view = build_tray_view(&[session, weekly]);
        assert!(view.tooltip.contains("Ollama"));
        assert!(!view.tooltip.contains("Ollama cloud"));
        assert!(view.tooltip.contains("5-hour: 28% left"));
        assert!(view.tooltip.contains("Weekly: 69% left"));
        assert!(!view.tooltip.contains("session rolling"));
        assert!(!view.tooltip.contains("weekly weekly"));
    }

    #[test]
    fn kimi_defaults_to_five_hour_and_can_focus_total() {
        let mut five_hour = make_snapshot(Some(58.0), Freshness::Live, Some("5-hour"));
        five_hour.provider = Provider::Kimi;
        five_hour.account_id = "kimi-test".into();
        five_hour.window_kind = WindowKind::Rolling;
        let mut weekly = five_hour.clone();
        weekly.used = Some(40.0);
        weekly.remaining = Some(60.0);
        weekly.window_kind = WindowKind::Weekly;
        weekly.window_label = Some("primary".into());
        let mut total = five_hour.clone();
        total.used = Some(12.0);
        total.remaining = Some(88.0);
        total.window_kind = WindowKind::Monthly;
        total.window_label = Some("total".into());

        let snapshots = vec![five_hour.clone(), total.clone(), weekly.clone()];
        let default_view = build_tray_view(&snapshots);
        assert_eq!(default_view.used_percent, Some(58.0));
        let five_hour_at = default_view.tooltip.find("5-hour:").unwrap();
        let total_at = default_view.tooltip.find("Total:").unwrap();
        let weekly_at = default_view.tooltip.find("Weekly:").unwrap();
        assert!(five_hour_at < total_at && total_at < weekly_at);
        assert!(default_view.tooltip.contains("5-hour: 42% left"));
        assert!(default_view.tooltip.contains("Weekly: 60% left"));
        assert!(default_view.tooltip.contains("Total: 88% left"));

        let total_view = build_tray_view_focused_window(
            &snapshots,
            Some(&Provider::Kimi),
            Some("total"),
            Utc::now(),
        );
        assert_eq!(total_view.used_percent, Some(12.0));
    }

    #[test]
    fn opencode_defaults_to_five_hour_and_can_focus_monthly() {
        let mut five_hour = make_snapshot(Some(8.0), Freshness::Live, Some("5-hour"));
        five_hour.provider = Provider::OpenCodeGo;
        five_hour.account_id = "opencode-local".into();
        five_hour.window_kind = WindowKind::Rolling;
        let mut weekly = five_hour.clone();
        weekly.used = Some(34.0);
        weekly.remaining = Some(66.0);
        weekly.window_kind = WindowKind::Weekly;
        weekly.window_label = Some("weekly".into());
        let mut monthly = five_hour.clone();
        monthly.used = Some(17.0);
        monthly.remaining = Some(83.0);
        monthly.window_kind = WindowKind::Monthly;
        monthly.window_label = Some("monthly".into());

        let snapshots = vec![five_hour, weekly, monthly];
        let default_view = build_tray_view(&snapshots);
        assert_eq!(default_view.used_percent, Some(8.0));
        assert_eq!(default_view.status_label, "OpenCode");
        assert!(default_view.tooltip.contains("Monthly: 83% left"));

        let monthly_view = build_tray_view_focused_window(
            &snapshots,
            Some(&Provider::OpenCodeGo),
            Some("monthly"),
            Utc::now(),
        );
        assert_eq!(monthly_view.used_percent, Some(17.0));
    }

    #[test]
    fn tooltip_keeps_later_provider_cards_visible() {
        let codex = make_snapshot(Some(20.0), Freshness::Live, Some("primary"));
        let mut codex_secondary = codex.clone();
        codex_secondary.window_label = Some("secondary".into());

        let mut grok = codex.clone();
        grok.provider = Provider::GrokConsumer;
        grok.account_id = "grok-test".into();
        let mut grok_secondary = grok.clone();
        grok_secondary.window_label = Some("secondary".into());

        let mut kimi = codex.clone();
        kimi.provider = Provider::Kimi;
        kimi.account_id = "kimi-test".into();
        let mut kimi_secondary = kimi.clone();
        kimi_secondary.window_label = Some("secondary".into());

        let mut ollama = codex.clone();
        ollama.provider = Provider::OllamaCloud;
        ollama.account_id = "ollama-test".into();
        ollama.window_label = Some("session".into());

        let view = build_tray_view(&[
            codex,
            codex_secondary,
            grok,
            grok_secondary,
            kimi,
            kimi_secondary,
            ollama,
        ]);
        assert!(view.tooltip.contains("Ollama"));
        assert!(!view.tooltip.ends_with("\n..."));
    }

    #[test]
    fn reset_label_includes_days_remaining() {
        let now = Utc.with_ymd_and_hms(2026, 8, 4, 12, 0, 0).unwrap();
        let reset = (now + Duration::days(3) + Duration::hours(5)).to_rfc3339();
        let label = format_reset_label(Some(&reset), now);
        assert!(label.contains("3d 5h left"), "{label}");
        assert!(label.contains("EDT"), "{label}");

        let winter_now = Utc.with_ymd_and_hms(2026, 1, 4, 12, 0, 0).unwrap();
        let winter_reset = (winter_now + Duration::days(3)).to_rfc3339();
        let winter_label = format_reset_label(Some(&winter_reset), winter_now);
        assert!(winter_label.contains("EST"), "{winter_label}");
    }
}
