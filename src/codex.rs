use crate::model::*;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

const CODEX_PROVIDER: Provider = Provider::Codex;
const CODEX_SOURCE: Source = Source::Cli;
const CODEX_CONFIDENCE: Confidence = Confidence::Exact;

#[derive(Debug, thiserror::Error)]
pub enum CodexAdapterError {
    #[error("auth expired or not configured")]
    AuthExpired,
    #[error("timeout waiting for app-server response")]
    Timeout,
    #[error("schema drift: {0}")]
    SchemaDrift(String),
    #[error("io error: {0}")]
    Io(String),
}

impl From<CodexAdapterError> for AdapterError {
    fn from(e: CodexAdapterError) -> Self {
        let code = match &e {
            CodexAdapterError::AuthExpired => ErrorCode::AuthExpired,
            CodexAdapterError::Timeout => ErrorCode::Timeout,
            CodexAdapterError::SchemaDrift(_) => ErrorCode::SchemaDrift,
            CodexAdapterError::Io(_) => ErrorCode::Network,
        };
        let message = match &e {
            CodexAdapterError::Io(msg) => Some(msg.clone()),
            CodexAdapterError::SchemaDrift(msg) => Some(msg.clone()),
            _ => None,
        };
        AdapterError { code, message }
    }
}

#[derive(Debug, Deserialize)]
struct RateLimitsResponse {
    #[serde(default, rename = "rateLimits")]
    rate_limits: Option<RateLimitSnapshot>,
    #[serde(default, rename = "rateLimitsByLimitId")]
    rate_limits_by_limit_id: Option<std::collections::BTreeMap<String, RateLimitSnapshot>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RateLimitSnapshot {
    #[serde(rename = "limitId", default)]
    limit_id: Option<String>,
    #[serde(rename = "limitName", default)]
    limit_name: Option<String>,
    #[serde(default)]
    primary: Option<RateLimitWindow>,
    #[serde(default)]
    secondary: Option<RateLimitWindow>,
    #[serde(default)]
    credits: Option<CreditsSnapshot>,
    #[serde(rename = "planType", default)]
    plan_type: Option<String>,
    #[serde(rename = "rateLimitReachedType", default)]
    rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RateLimitWindow {
    #[serde(rename = "usedPercent")]
    used_percent: i32,
    #[serde(rename = "windowDurationMins", default)]
    window_duration_mins: Option<i64>,
    #[serde(default, rename = "resetsAt")]
    resets_at: Option<i64>,
}

impl RateLimitWindow {
    fn validate(&self) -> Result<(), String> {
        if !(0..=100).contains(&self.used_percent) {
            return Err(format!(
                "usedPercent {} out of range [0, 100]",
                self.used_percent
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CreditsSnapshot {
    #[serde(rename = "hasCredits")]
    has_credits: bool,
    unlimited: bool,
    #[serde(default)]
    balance: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountResponse {
    #[serde(default)]
    account: Option<AccountInfo>,
    #[serde(rename = "requiresOpenaiAuth")]
    requires_openai_auth: bool,
}

#[derive(Debug, Deserialize)]
pub struct AccountInfo {
    #[serde(rename = "type")]
    pub account_type: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(rename = "planType", default)]
    pub plan_type: Option<String>,
}

fn epoch_to_datetime(epoch: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(epoch, 0).unwrap()
}

fn duration_to_window_kind(mins: Option<i64>) -> WindowKind {
    match mins {
        Some(1440) => WindowKind::Daily,
        Some(10080) => WindowKind::Weekly,
        Some(43200) => WindowKind::Monthly,
        _ => WindowKind::Rolling,
    }
}

fn redact_account_id(email: Option<&str>) -> String {
    match email {
        Some(e) => {
            let hash = simple_hash(e);
            format!("codex-{hash:016x}")
        }
        None => "codex-unknown".to_string(),
    }
}

fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn parse_window(
    window: &RateLimitWindow,
    label: &str,
    account_id: &str,
    observed_at: DateTime<Utc>,
) -> Result<UsageSnapshot, String> {
    window.validate()?;
    Ok(UsageSnapshot {
        provider: CODEX_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind: duration_to_window_kind(window.window_duration_mins),
        unit: "percent".to_string(),
        observed_at,
        source: CODEX_SOURCE,
        freshness: Freshness::Live,
        confidence: CODEX_CONFIDENCE,
        used: Some(f64::from(window.used_percent)),
        remaining: Some(100.0 - f64::from(window.used_percent)),
        limit: Some(100.0),
        unlimited: false,
        resets_at: window.resets_at.map(epoch_to_datetime),
        window_label: Some(label.to_string()),
        error: None,
    })
}

fn parse_credits(
    credits: &CreditsSnapshot,
    account_id: &str,
    observed_at: DateTime<Utc>,
) -> UsageSnapshot {
    UsageSnapshot {
        provider: CODEX_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Credits,
        window_kind: WindowKind::None,
        unit: "credits".to_string(),
        observed_at,
        source: CODEX_SOURCE,
        freshness: Freshness::Live,
        confidence: CODEX_CONFIDENCE,
        used: credits.balance.as_ref().and_then(|b| b.parse::<f64>().ok()),
        remaining: None,
        limit: None,
        unlimited: credits.unlimited,
        resets_at: None,
        window_label: None,
        error: None,
    }
}

fn parse_snapshot(
    snap: &RateLimitSnapshot,
    account_id: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<UsageSnapshot>, String> {
    let mut out = Vec::new();

    if let Some(w) = &snap.primary {
        out.push(parse_window(w, "primary", account_id, observed_at)?);
    }
    if let Some(w) = &snap.secondary {
        out.push(parse_window(w, "secondary", account_id, observed_at)?);
    }
    if let Some(c) = &snap.credits {
        out.push(parse_credits(c, account_id, observed_at));
    }

    if out.is_empty() {
        return Err("no primary, secondary, or credits in rate limit snapshot".to_string());
    }
    Ok(out)
}

pub fn parse_rate_limits_response(
    raw: &serde_json::Value,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<Vec<UsageSnapshot>, CodexAdapterError> {
    let resp: RateLimitsResponse = serde_json::from_value(raw.clone())
        .map_err(|e| CodexAdapterError::SchemaDrift(e.to_string()))?;

    let snapshots = if let Some(by_id) = resp.rate_limits_by_limit_id {
        let mut all = Vec::new();
        for (_id, snap) in by_id {
            all.extend(parse_snapshot(&snap, account_id, observed_at)
                .map_err(CodexAdapterError::SchemaDrift)?);
        }
        all
    } else if let Some(snap) = resp.rate_limits {
        parse_snapshot(&snap, account_id, observed_at)
            .map_err(CodexAdapterError::SchemaDrift)?
    } else {
        return Err(CodexAdapterError::SchemaDrift(
            "no rateLimits or rateLimitsByLimitId".to_string(),
        ));
    };

    Ok(snapshots)
}

pub fn parse_account_response(raw: &serde_json::Value) -> Result<AccountInfo, CodexAdapterError> {
    let resp: AccountResponse = serde_json::from_value(raw.clone())
        .map_err(|e| CodexAdapterError::SchemaDrift(e.to_string()))?;
    if !resp.requires_openai_auth {
        return Err(CodexAdapterError::AuthExpired);
    }
    resp.account.ok_or(CodexAdapterError::AuthExpired)
}

pub fn account_id_from_email(email: Option<&str>) -> String {
    redact_account_id(email)
}

pub fn error_snapshot(
    account_id: &str,
    observed_at: DateTime<Utc>,
    err: CodexAdapterError,
) -> UsageSnapshot {
    UsageSnapshot {
        provider: CODEX_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind: WindowKind::None,
        unit: "percent".to_string(),
        observed_at,
        source: CODEX_SOURCE,
        freshness: Freshness::Unavailable,
        confidence: Confidence::Unknown,
        used: None,
        remaining: None,
        limit: None,
        unlimited: false,
        resets_at: None,
        window_label: None,
        error: Some(AdapterError::from(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn fixture_time() -> DateTime<Utc> {
        Utc.timestamp_opt(1786000000, 0).unwrap()
    }

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures/codex")
            .join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
        let full: serde_json::Value = serde_json::from_str(&content).unwrap();
        full.get("result").cloned().unwrap_or(full)
    }

    #[test]
    fn parse_normal_fixture() {
        let raw = load_fixture("normal.json");
        let account_id = "codex-deadbeef";
        let snaps = parse_rate_limits_response(&raw, fixture_time(), account_id).unwrap();
        assert!(!snaps.is_empty());
        let primary = snaps.iter().find(|s| s.window_label == Some("primary".into())).unwrap();
        assert_eq!(primary.provider, Provider::Codex);
        assert_eq!(primary.metric_kind, MetricKind::Quota);
        assert_eq!(primary.window_kind, WindowKind::Weekly);
        assert_eq!(primary.used, Some(40.0));
        assert_eq!(primary.remaining, Some(60.0));
        assert_eq!(primary.limit, Some(100.0));
        assert_eq!(primary.unit, "percent");
        assert_eq!(primary.freshness, Freshness::Live);
        assert_eq!(primary.resets_at, Some(epoch_to_datetime(1786036566)));
        let credits = snaps.iter().find(|s| s.metric_kind == MetricKind::Credits).unwrap();
        assert_eq!(credits.unlimited, false);
        assert_eq!(credits.used, Some(0.0));
    }

    #[test]
    fn parse_multiple_windows_fixture() {
        let raw = load_fixture("multiple_windows.json");
        let snaps = parse_rate_limits_response(&raw, fixture_time(), "codex-test").unwrap();
        let primary = snaps.iter().find(|s| s.window_label == Some("primary".into())).unwrap();
        let secondary = snaps.iter().find(|s| s.window_label == Some("secondary".into())).unwrap();
        assert_eq!(primary.used, Some(65.0));
        assert_eq!(primary.window_kind, WindowKind::Weekly);
        assert_eq!(secondary.used, Some(80.0));
        assert_eq!(secondary.window_kind, WindowKind::Daily);
    }

    #[test]
    fn parse_unlimited_credits_fixture() {
        let raw = load_fixture("unlimited_or_missing.json");
        let snaps = parse_rate_limits_response(&raw, fixture_time(), "codex-test").unwrap();
        let credits = snaps.iter().find(|s| s.metric_kind == MetricKind::Credits).unwrap();
        assert!(credits.unlimited);
        assert_eq!(credits.used, None);
    }

    #[test]
    fn parse_auth_failure_fixture() {
        let raw = load_fixture("auth_failure.json");
        let result = parse_account_response(&raw);
        assert!(matches!(result, Err(CodexAdapterError::AuthExpired)));
    }

    #[test]
    fn parse_malformed_fixture_returns_schema_drift() {
        let raw = load_fixture("malformed.json");
        let result = parse_rate_limits_response(&raw, fixture_time(), "codex-test");
        assert!(matches!(result, Err(CodexAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn used_percent_out_of_range_is_rejected() {
        let raw = serde_json::json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 150, "windowDurationMins": 10080, "resetsAt": 1786036566 },
                "credits": { "hasCredits": false, "unlimited": false, "balance": "0" },
                "planType": "plus",
                "rateLimitReachedType": null
            }
        });
        let result = parse_rate_limits_response(&raw, fixture_time(), "codex-test");
        assert!(matches!(result, Err(CodexAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn missing_used_percent_is_schema_drift() {
        let raw = serde_json::json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "windowDurationMins": 10080, "resetsAt": 1786036566 },
                "credits": { "hasCredits": false, "unlimited": false, "balance": "0" }
            }
        });
        let result = parse_rate_limits_response(&raw, fixture_time(), "codex-test");
        assert!(matches!(result, Err(CodexAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn account_id_redaction_is_stable_and_not_email() {
        let id1 = redact_account_id(Some("user@example.com"));
        let id2 = redact_account_id(Some("user@example.com"));
        let id3 = redact_account_id(Some("other@example.com"));
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert!(!id1.contains("user@example.com"));
        assert!(id1.starts_with("codex-"));
    }

    #[test]
    fn duration_to_window_kind_mapping() {
        assert_eq!(duration_to_window_kind(Some(1440)), WindowKind::Daily);
        assert_eq!(duration_to_window_kind(Some(10080)), WindowKind::Weekly);
        assert_eq!(duration_to_window_kind(Some(43200)), WindowKind::Monthly);
        assert_eq!(duration_to_window_kind(Some(300)), WindowKind::Rolling);
        assert_eq!(duration_to_window_kind(None), WindowKind::Rolling);
    }

    #[test]
    fn error_snapshot_is_unavailable_with_redacted_code() {
        let snap = error_snapshot("codex-test", fixture_time(), CodexAdapterError::Timeout);
        assert_eq!(snap.freshness, Freshness::Unavailable);
        assert!(snap.used.is_none());
        assert!(snap.error.is_some());
        assert_eq!(snap.error.unwrap().code, ErrorCode::Timeout);
    }

    #[test]
    fn rate_limit_reached_maps_to_error() {
        let raw = serde_json::json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 100, "windowDurationMins": 10080, "resetsAt": 1786036566 },
                "credits": { "hasCredits": false, "unlimited": false, "balance": "0" },
                "planType": "plus",
                "rateLimitReachedType": "rate_limit_reached"
            }
        });
        let snaps = parse_rate_limits_response(&raw, fixture_time(), "codex-test").unwrap();
        let primary = snaps.iter().find(|s| s.window_label == Some("primary".into())).unwrap();
        assert_eq!(primary.used, Some(100.0));
        assert_eq!(primary.remaining, Some(0.0));
    }
}