use ai_usage_bar::{
    fetch_codex_snapshots, fetch_grok_consumer_snapshots, Freshness, UsageSnapshot,
};

fn main() {
    let mut any_ok = false;

    match fetch_codex_snapshots() {
        Ok(snapshots) => {
            any_ok = true;
            print_provider("Codex", &snapshots);
        }
        Err(e) => eprintln!("Codex: {e}"),
    }

    match fetch_grok_consumer_snapshots() {
        Ok(snapshots) => {
            any_ok = true;
            print_provider("Grok consumer", &snapshots);
        }
        Err(e) => eprintln!("Grok consumer: {e}"),
    }

    if !any_ok {
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
        let label = s.window_label.as_deref().unwrap_or("—");
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
        let unit = &s.unit;

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
                err.message.as_deref().unwrap_or("")
            );
        }
    }
    println!();
}
