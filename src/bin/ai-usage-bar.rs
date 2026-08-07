use ai_usage_bar::{
    load_registry, provider_display_name, redact_sensitive_text, Freshness, Provider,
    RefreshPolicy, RefreshService, UsageSnapshot,
};

fn main() {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!(
                "failed to load provider configuration: {}",
                redact_sensitive_text(&error.to_string())
            );
            std::process::exit(1);
        }
    };
    let report = RefreshService::new(registry, RefreshPolicy::default()).refresh_all_with_report();

    let mut grouped: Vec<(Provider, Vec<UsageSnapshot>)> = Vec::new();
    for snapshot in report.snapshots {
        if let Some((_, snapshots)) = grouped
            .iter_mut()
            .find(|(provider, _)| provider == &snapshot.provider)
        {
            snapshots.push(snapshot);
        } else {
            grouped.push((snapshot.provider.clone(), vec![snapshot]));
        }
    }

    for (provider, snapshots) in &grouped {
        print_provider(provider_display_name(provider), snapshots);
    }

    let any_usable = report.diagnostics.iter().any(|diagnostic| {
        !matches!(
            diagnostic.freshness,
            Freshness::Unavailable | Freshness::NotConfigured
        )
    });
    if !any_usable {
        std::process::exit(1);
    }
}

fn print_provider(name: &str, snapshots: &[UsageSnapshot]) {
    println!(
        "{name} — {} snapshot(s) at {}\n",
        snapshots.len(),
        snapshots
            .first()
            .map(|s| s.observed_at.to_rfc3339())
            .unwrap_or_default()
    );
    for s in snapshots {
        let label = s
            .window_label
            .as_deref()
            .map(redact_sensitive_text)
            .unwrap_or_else(|| "—".to_string());
        let used = s.used.map(|u| format!("{u:.0}")).unwrap_or_else(|| "—".into());
        let limit = s
            .limit
            .map(|l| format!("{l:.0}"))
            .unwrap_or_else(|| "—".into());
        let remaining = s
            .remaining
            .map(|r| format!("{r:.0}"))
            .unwrap_or_else(|| "—".into());
        let resets = s
            .resets_at
            .map(|r| r.to_rfc3339())
            .unwrap_or_else(|| "—".to_string());
        let unit = redact_sensitive_text(&s.unit);

        println!(
            "  [{label}] {kind:?} {window:?} — used {used}/{limit} {unit}, remaining {remaining}, resets {resets}",
            kind = s.metric_kind,
            window = s.window_kind,
        );
        if s.freshness != Freshness::Live {
            println!("    freshness: {:?}", s.freshness);
        }
        if let Some(err) = &s.error {
            println!(
                "    error: {} ({})",
                err.code.as_str(),
                err.message
                    .as_deref()
                    .map(redact_sensitive_text)
                    .unwrap_or_default()
            );
        }
    }
    println!();
}
