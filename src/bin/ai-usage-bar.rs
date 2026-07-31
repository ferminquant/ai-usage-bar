use ai_usage_bar::{fetch_codex_snapshots, Freshness};

fn main() {
    match fetch_codex_snapshots() {
        Ok(snapshots) => {
            println!(
                "Codex usage — {} snapshot(s) at {}\n",
                snapshots.len(),
                snapshots
                    .first()
                    .map(|s| s.observed_at.to_rfc3339())
                    .unwrap_or_default()
            );
            for s in &snapshots {
                let label = s.window_label.as_deref().unwrap_or("—");
                let used = s.used.map(|u| format!("{u:.0}")).unwrap_or("—".into());
                let limit = s.limit.map(|l| format!("{l:.0}")).unwrap_or("—".into());
                let remaining = s
                    .remaining
                    .map(|r| format!("{r:.0}"))
                    .unwrap_or("—".into());
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
                    println!("    error: {} ({})", err.code.as_str(), err.message.as_deref().unwrap_or(""));
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}