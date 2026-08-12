//! Grok consumer (SuperGrok) weekly usage adapter.
//!
//! Source (verified in `docs/spikes/grok-spike.md`): Grok Build CLI OIDC session
//! in `~/.grok/auth.json` and the official CLI chat-proxy billing endpoint
//! `GET …/billing?format=credits`. Does **not** implement xAI API rate limits
//! or console spend (`grok_api` is deferred).

use crate::model::*;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const GROK_PROVIDER: Provider = Provider::GrokConsumer;
const GROK_SOURCE: Source = Source::Cli;
const GROK_CONFIDENCE: Confidence = Confidence::Exact;

const DEFAULT_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const DEFAULT_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const TOKEN_AUTH_HEADER: &str = "xai-grok-cli";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default)]
pub struct GrokConsumerAdapter;

#[derive(Debug, thiserror::Error)]
pub enum GrokAdapterError {
    #[error("auth expired or not configured")]
    AuthExpired,
    #[error("timeout waiting for billing response")]
    Timeout,
    #[error("schema drift: {0}")]
    SchemaDrift(String),
    #[error("network error")]
    Network,
    #[error("rate limited")]
    RateLimited,
}

impl From<GrokAdapterError> for AdapterError {
    fn from(e: GrokAdapterError) -> Self {
        let code = match &e {
            GrokAdapterError::AuthExpired => ErrorCode::AuthExpired,
            GrokAdapterError::Timeout => ErrorCode::Timeout,
            GrokAdapterError::SchemaDrift(_) => ErrorCode::SchemaDrift,
            GrokAdapterError::Network => ErrorCode::Network,
            GrokAdapterError::RateLimited => ErrorCode::RateLimited,
        };
        // Never attach upstream bodies or tokens — redacted code only.
        AdapterError {
            code,
            message: None,
        }
    }
}

impl ProviderAdapter for GrokConsumerAdapter {
    fn provider(&self) -> Provider {
        GROK_PROVIDER
    }

    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
        fetch_grok_consumer_snapshots().map_err(AdapterError::from)
    }
}

// ---------------------------------------------------------------------------
// Billing response shape (camelCase JSON from cli-chat-proxy)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BillingResponse {
    #[serde(default)]
    config: Option<BillingConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingConfig {
    #[serde(default)]
    credit_usage_percent: Option<f64>,
    #[serde(default)]
    current_period: Option<UsagePeriod>,
    /// Legacy monthly included budget in USD cents.
    #[serde(default)]
    monthly_limit: Option<Cent>,
    /// Legacy used amount in USD cents.
    #[serde(default)]
    used: Option<Cent>,
    #[serde(default)]
    prepaid_balance: Option<Cent>,
    /// On-demand spend cap and usage. These are zero-valued in the live
    /// unified-billing response when the included percentage is omitted by
    /// proto3 JSON encoding.
    #[serde(default)]
    on_demand_cap: Option<Cent>,
    #[serde(default)]
    on_demand_used: Option<Cent>,
    #[serde(default)]
    #[allow(dead_code)]
    billing_period_start: Option<String>,
    #[serde(default)]
    billing_period_end: Option<String>,
    /// Present in live captures; ignored for compact quota (detail only).
    #[serde(default)]
    #[allow(dead_code)]
    product_usage: Option<Vec<ProductUsage>>,
    #[serde(default)]
    #[allow(dead_code)]
    is_unified_billing_user: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsagePeriod {
    #[serde(rename = "type", default)]
    period_type: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Cent {
    #[serde(default)]
    val: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ProductUsage {
    product: Option<String>,
    usage_percent: Option<f64>,
}

// ---------------------------------------------------------------------------
// Auth store (`~/.grok/auth.json`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct GrokAuthSession {
    key: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    oidc_client_id: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
struct LoadedAuth {
    path: PathBuf,
    map_key: String,
    session: GrokAuthSession,
}

fn grok_home() -> PathBuf {
    if let Ok(custom) = std::env::var("GROK_HOME") {
        return PathBuf::from(custom);
    }
    dirs_next_home()
        .map(|h| h.join(".grok"))
        .unwrap_or_else(|| PathBuf::from(".grok"))
}

/// Minimal home-dir resolution without an extra crate.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn auth_json_path() -> PathBuf {
    if let Ok(custom) = std::env::var("GROK_AUTH_JSON") {
        return PathBuf::from(custom);
    }
    grok_home().join("auth.json")
}

fn load_auth_session(path: &Path) -> Result<LoadedAuth, GrokAdapterError> {
    let raw = fs::read_to_string(path).map_err(|_| GrokAdapterError::AuthExpired)?;
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).map_err(|_| GrokAdapterError::AuthExpired)?;
    // Prefer an entry that looks like an xAI OIDC session.
    let (map_key, value) = map
        .iter()
        .find(|(k, _)| k.contains("auth.x.ai"))
        .or_else(|| map.iter().next())
        .ok_or(GrokAdapterError::AuthExpired)?;
    let session: GrokAuthSession =
        serde_json::from_value(value.clone()).map_err(|_| GrokAdapterError::AuthExpired)?;
    if session.key.trim().is_empty() {
        return Err(GrokAdapterError::AuthExpired);
    }
    Ok(LoadedAuth {
        path: path.to_path_buf(),
        map_key: map_key.clone(),
        session,
    })
}

fn session_expired(session: &GrokAuthSession, now: DateTime<Utc>) -> bool {
    let Some(expires) = session.expires_at.as_deref() else {
        return false;
    };
    match DateTime::parse_from_rfc3339(expires) {
        Ok(dt) => dt.with_timezone(&Utc) <= now + chrono::Duration::seconds(60),
        Err(_) => false,
    }
}

fn client_id_from_map_key(map_key: &str) -> Option<String> {
    // Keys look like `https://auth.x.ai::<client_id>`.
    map_key
        .rsplit_once("::")
        .map(|(_, id)| id.to_string())
        .filter(|id| !id.is_empty())
}

fn persist_auth_session(loaded: &LoadedAuth) -> Result<(), GrokAdapterError> {
    let raw = fs::read_to_string(&loaded.path).map_err(|_| GrokAdapterError::Network)?;
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).map_err(|_| GrokAdapterError::Network)?;
    let value = serde_json::to_value(&loaded.session).map_err(|_| GrokAdapterError::Network)?;
    map.insert(loaded.map_key.clone(), value);
    let pretty = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .map_err(|_| GrokAdapterError::Network)?;
    fs::write(&loaded.path, pretty).map_err(|_| GrokAdapterError::Network)
}

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    #[serde(default)]
    error: Option<String>,
}

fn refresh_access_token(loaded: &mut LoadedAuth) -> Result<(), GrokAdapterError> {
    let refresh = loaded
        .session
        .refresh_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or(GrokAdapterError::AuthExpired)?;
    let client_id = loaded
        .session
        .oidc_client_id
        .clone()
        .or_else(|| client_id_from_map_key(&loaded.map_key))
        .ok_or(GrokAdapterError::AuthExpired)?;

    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        urlencoding_minimal(refresh),
        urlencoding_minimal(&client_id)
    );

    let response = ureq::post(DEFAULT_TOKEN_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .send_string(&body)
        .map_err(map_ureq_error)?;

    let status = response.status();
    let parsed: TokenRefreshResponse = response
        .into_json()
        .map_err(|_| GrokAdapterError::AuthExpired)?;
    if !(200..300).contains(&status) || parsed.error.is_some() {
        return Err(GrokAdapterError::AuthExpired);
    }
    let access = parsed
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or(GrokAdapterError::AuthExpired)?;
    loaded.session.key = access;
    if let Some(rt) = parsed.refresh_token.filter(|t| !t.is_empty()) {
        loaded.session.refresh_token = Some(rt);
    }
    if let Some(secs) = parsed.expires_in {
        let exp = Utc::now() + chrono::Duration::seconds(secs.max(0));
        loaded.session.expires_at = Some(exp.to_rfc3339());
    }
    // Best-effort persist; still use the refreshed token if write fails.
    let _ = persist_auth_session(loaded);
    Ok(())
}

fn urlencoding_minimal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn map_ureq_error(err: ureq::Error) -> GrokAdapterError {
    match err {
        ureq::Error::Status(401 | 403, _) => GrokAdapterError::AuthExpired,
        ureq::Error::Status(429, _) => GrokAdapterError::RateLimited,
        ureq::Error::Status(_, _) => GrokAdapterError::Network,
        ureq::Error::Transport(t) => {
            let msg = t.to_string().to_lowercase();
            if msg.contains("timed out") || msg.contains("timeout") {
                GrokAdapterError::Timeout
            } else {
                GrokAdapterError::Network
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Stable redacted account id from user id (preferred) or email.
pub fn account_id_from_identity(user_id: Option<&str>, email: Option<&str>) -> String {
    if let Some(uid) = user_id.filter(|s| !s.trim().is_empty()) {
        return format!("grok-consumer-{:016x}", simple_hash(uid));
    }
    if let Some(mail) = email.filter(|s| !s.trim().is_empty()) {
        return format!("grok-consumer-{:016x}", simple_hash(mail));
    }
    "grok-consumer-unknown".to_string()
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn window_kind_from_period(period_type: Option<&str>) -> WindowKind {
    let Some(t) = period_type.map(|s| s.to_ascii_uppercase()) else {
        return WindowKind::Weekly;
    };
    if t.contains("DAILY") {
        WindowKind::Daily
    } else if t.contains("MONTHLY") {
        WindowKind::Monthly
    } else if t.contains("WEEKLY") {
        WindowKind::Weekly
    } else {
        WindowKind::Rolling
    }
}

fn resolve_usage_percent(config: &BillingConfig) -> Result<f64, GrokAdapterError> {
    if let Some(pct) = config.credit_usage_percent {
        if !pct.is_finite() || !(0.0..=100.0).contains(&pct) {
            return Err(GrokAdapterError::SchemaDrift(format!(
                "creditUsagePercent {pct} out of range [0, 100]"
            )));
        }
        return Ok(pct);
    }
    // Legacy cents: used / monthly_limit → percent.
    if let (Some(limit), Some(used)) = (&config.monthly_limit, &config.used) {
        if limit.val > 0 {
            let pct = (used.val as f64) / (limit.val as f64) * 100.0;
            if !pct.is_finite() || !(0.0..=100.0).contains(&pct) {
                return Err(GrokAdapterError::SchemaDrift(
                    "legacy used/monthlyLimit percent out of range".into(),
                ));
            }
            return Ok(pct);
        }
    }
    // Proto3 JSON omits zero-valued scalar fields. A current unified billing
    // period with explicit zero on-demand/prepaid balances is therefore a
    // valid 0%-used period, not an ambiguous missing quota. Keep this guard
    // narrow so unrelated incomplete payloads remain schema drift.
    let no_prepaid = config
        .prepaid_balance
        .as_ref()
        .is_none_or(|balance| balance.val == 0);
    let no_product_usage = config.product_usage.as_ref().is_none_or(Vec::is_empty);
    let has_valid_period = config.current_period.as_ref().is_some_and(|period| {
        period.period_type.as_deref().is_some_and(|period_type| {
            matches!(
                window_kind_from_period(Some(period_type)),
                WindowKind::Daily | WindowKind::Weekly | WindowKind::Monthly
            )
        }) && period.end.as_deref().and_then(parse_rfc3339).is_some()
    });
    let zero_usage_period = config.credit_usage_percent.is_none()
        && config.monthly_limit.is_none()
        && config.used.is_none()
        && config.is_unified_billing_user == Some(true)
        && has_valid_period
        && config
            .on_demand_cap
            .as_ref()
            .is_some_and(|cap| cap.val == 0)
        && config
            .on_demand_used
            .as_ref()
            .is_some_and(|used| used.val == 0)
        && no_prepaid
        && no_product_usage;
    if zero_usage_period {
        return Ok(0.0);
    }
    Err(GrokAdapterError::SchemaDrift(
        "missing creditUsagePercent and no usable legacy limit".into(),
    ))
}

/// Parse a billing JSON body into provider-neutral snapshots.
///
/// Emits the shared weekly quota as the primary window. Optional prepaid
/// balance becomes a separate credits snapshot when present and non-zero.
/// Product breakdown rows are **not** emitted as independent quotas.
pub fn parse_billing_response(
    raw: &serde_json::Value,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<Vec<UsageSnapshot>, GrokAdapterError> {
    // Reject obvious non-billing shapes early.
    if raw.get("error").is_some() && raw.get("config").is_none() {
        let code = raw
            .pointer("/error/code")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if code == "auth_expired" || code == "timeout" {
            return Err(if code == "timeout" {
                GrokAdapterError::Timeout
            } else {
                GrokAdapterError::AuthExpired
            });
        }
        return Err(GrokAdapterError::SchemaDrift(
            "billing response contains error without config".into(),
        ));
    }
    if raw.get("unexpected").is_some() {
        return Err(GrokAdapterError::SchemaDrift(
            "unexpected billing payload".into(),
        ));
    }

    let resp: BillingResponse = serde_json::from_value(raw.clone())
        .map_err(|e| GrokAdapterError::SchemaDrift(e.to_string()))?;
    let config = resp
        .config
        .ok_or_else(|| GrokAdapterError::SchemaDrift("missing config".into()))?;

    let mut out = Vec::new();

    match resolve_usage_percent(&config) {
        Ok(used_percent) => {
            let period = config.current_period.as_ref();
            let window_kind =
                window_kind_from_period(period.and_then(|p| p.period_type.as_deref()));
            let resets_at = period
                .and_then(|p| p.end.as_deref())
                .or(config.billing_period_end.as_deref())
                .and_then(parse_rfc3339);

            out.push(UsageSnapshot {
                provider: GROK_PROVIDER,
                account_id: account_id.to_string(),
                metric_kind: MetricKind::Quota,
                window_kind,
                unit: "percent".to_string(),
                observed_at,
                source: GROK_SOURCE,
                freshness: Freshness::Live,
                confidence: GROK_CONFIDENCE,
                used: Some(used_percent),
                remaining: Some(100.0 - used_percent),
                limit: Some(100.0),
                unlimited: false,
                resets_at,
                window_label: Some("primary".into()),
                error: None,
            });
        }
        Err(GrokAdapterError::SchemaDrift(_))
            if config.prepaid_balance.as_ref().is_some_and(|c| c.val > 0) =>
        {
            // Missing included % is allowed when prepaid credits are the only signal.
        }
        Err(e) => return Err(e),
    }

    if let Some(prepaid) = &config.prepaid_balance {
        if prepaid.val > 0 {
            out.push(UsageSnapshot {
                provider: GROK_PROVIDER,
                account_id: account_id.to_string(),
                metric_kind: MetricKind::Credits,
                window_kind: WindowKind::None,
                unit: "cents".to_string(),
                observed_at,
                source: GROK_SOURCE,
                freshness: Freshness::Live,
                confidence: GROK_CONFIDENCE,
                used: None,
                remaining: Some(prepaid.val as f64),
                limit: None,
                unlimited: false,
                resets_at: None,
                window_label: Some("prepaid".into()),
                error: None,
            });
        }
    }

    if out.is_empty() {
        return Err(GrokAdapterError::SchemaDrift(
            "no quota percent or prepaid credits in billing config".into(),
        ));
    }

    for snap in &out {
        snap.validate()
            .map_err(|e| GrokAdapterError::SchemaDrift(e.to_string()))?;
    }

    Ok(out)
}

pub fn error_snapshot(
    account_id: &str,
    observed_at: DateTime<Utc>,
    err: GrokAdapterError,
) -> UsageSnapshot {
    UsageSnapshot {
        provider: GROK_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind: WindowKind::None,
        unit: "percent".to_string(),
        observed_at,
        source: GROK_SOURCE,
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

// ---------------------------------------------------------------------------
// HTTP fetch
// ---------------------------------------------------------------------------

fn http_get_billing(
    access_token: &str,
    user_id: &str,
) -> Result<serde_json::Value, GrokAdapterError> {
    let billing_url =
        std::env::var("GROK_BILLING_URL").unwrap_or_else(|_| DEFAULT_BILLING_URL.to_string());
    let version = std::env::var("GROK_CLIENT_VERSION").unwrap_or_else(|_| "0.1.0".into());

    let response = ureq::get(&billing_url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("X-XAI-Token-Auth", TOKEN_AUTH_HEADER)
        .set("x-userid", user_id)
        .set("x-grok-client-version", &version)
        .set("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(map_ureq_error)?;

    let status = response.status();
    if status == 401 || status == 403 {
        return Err(GrokAdapterError::AuthExpired);
    }
    if status == 429 {
        return Err(GrokAdapterError::RateLimited);
    }
    if !(200..300).contains(&status) {
        return Err(GrokAdapterError::Network);
    }

    response
        .into_json()
        .map_err(|_| GrokAdapterError::SchemaDrift("billing body is not JSON".into()))
}

/// Fetch live SuperGrok weekly usage using the local Grok Build CLI session.
pub fn fetch_grok_consumer_snapshots() -> Result<Vec<UsageSnapshot>, GrokAdapterError> {
    let path = auth_json_path();
    if !path.exists() {
        return Err(GrokAdapterError::AuthExpired);
    }
    let mut loaded = load_auth_session(&path)?;
    let now = Utc::now();

    if session_expired(&loaded.session, now) {
        refresh_access_token(&mut loaded)?;
    }

    let user_id = loaded
        .session
        .user_id
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let account_id = account_id_from_identity(
        loaded.session.user_id.as_deref(),
        loaded.session.email.as_deref(),
    );

    let body = match http_get_billing(&loaded.session.key, &user_id) {
        Ok(v) => v,
        Err(GrokAdapterError::AuthExpired) => {
            refresh_access_token(&mut loaded)?;
            http_get_billing(&loaded.session.key, &user_id)?
        }
        Err(e) => return Err(e),
    };

    parse_billing_response(&body, Utc::now(), &account_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn fixture_time() -> DateTime<Utc> {
        Utc.timestamp_opt(1_786_000_000, 0).unwrap()
    }

    fn load_fixture(name: &str) -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures/grok_consumer")
            .join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"));
        serde_json::from_str(&content).unwrap()
    }

    #[test]
    fn adapter_reports_grok_consumer_without_fetching() {
        assert_eq!(GrokConsumerAdapter.provider(), Provider::GrokConsumer);
    }

    #[test]
    fn parse_normal_fixture() {
        let raw = load_fixture("normal.json");
        let snaps = parse_billing_response(&raw, fixture_time(), "grok-consumer-test").unwrap();
        assert_eq!(snaps.len(), 1);
        let primary = &snaps[0];
        assert_eq!(primary.provider, Provider::GrokConsumer);
        assert_eq!(primary.metric_kind, MetricKind::Quota);
        assert_eq!(primary.window_kind, WindowKind::Weekly);
        assert_eq!(primary.unit, "percent");
        assert_eq!(primary.used, Some(8.0));
        assert_eq!(primary.remaining, Some(92.0));
        assert_eq!(primary.limit, Some(100.0));
        assert_eq!(primary.window_label.as_deref(), Some("primary"));
        assert_eq!(primary.freshness, Freshness::Live);
        assert!(primary.resets_at.is_some());
        assert!(primary.validate().is_ok());
    }

    #[test]
    fn parse_zero_usage_with_omitted_proto3_percent() {
        let raw = load_fixture("zero_usage_omitted.json");
        let snaps = parse_billing_response(&raw, fixture_time(), "grok-consumer-test").unwrap();
        assert_eq!(snaps.len(), 1);
        let primary = &snaps[0];
        assert_eq!(primary.window_kind, WindowKind::Weekly);
        assert_eq!(primary.used, Some(0.0));
        assert_eq!(primary.remaining, Some(100.0));
        assert_eq!(primary.limit, Some(100.0));
        assert_eq!(primary.window_label.as_deref(), Some("primary"));
        assert_eq!(
            primary.resets_at,
            parse_rfc3339("2026-08-18T13:28:26.395580+00:00")
        );
        assert!(primary.validate().is_ok());
    }

    #[test]
    fn missing_percent_without_zero_usage_evidence_is_schema_drift() {
        let raw = serde_json::json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "end": "2026-08-18T13:28:26.395580+00:00"
                },
                "isUnifiedBillingUser": true
            }
        });
        let result = parse_billing_response(&raw, fixture_time(), "grok-consumer-test");
        assert!(matches!(result, Err(GrokAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn parse_multiple_products_still_one_primary_quota() {
        let raw = load_fixture("multiple_windows.json");
        let snaps = parse_billing_response(&raw, fixture_time(), "grok-consumer-test").unwrap();
        // Product breakdown must not create extra independent quota icons.
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].used, Some(42.5));
        assert_eq!(snaps[0].window_label.as_deref(), Some("primary"));
    }

    #[test]
    fn parse_prepaid_without_included_percent() {
        let raw = load_fixture("unlimited_or_missing.json");
        let snaps = parse_billing_response(&raw, fixture_time(), "grok-consumer-test").unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].metric_kind, MetricKind::Credits);
        assert_eq!(snaps[0].remaining, Some(1500.0));
        assert_eq!(snaps[0].unit, "cents");
        assert!(snaps[0].used.is_none());
    }

    #[test]
    fn parse_auth_failure_fixture() {
        let raw = load_fixture("auth_failure.json");
        let result = parse_billing_response(&raw, fixture_time(), "grok-consumer-test");
        assert!(matches!(result, Err(GrokAdapterError::AuthExpired)));
    }

    #[test]
    fn parse_timeout_fixture() {
        let raw = load_fixture("timeout.json");
        let result = parse_billing_response(&raw, fixture_time(), "grok-consumer-test");
        assert!(matches!(result, Err(GrokAdapterError::Timeout)));
    }

    #[test]
    fn parse_malformed_fixture() {
        let raw = load_fixture("malformed.json");
        let result = parse_billing_response(&raw, fixture_time(), "grok-consumer-test");
        assert!(matches!(result, Err(GrokAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn percent_out_of_range_is_schema_drift() {
        let raw = serde_json::json!({
            "config": {
                "creditUsagePercent": 150.0,
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "end": "2026-08-11T13:28:26.395580+00:00"
                }
            }
        });
        let result = parse_billing_response(&raw, fixture_time(), "grok-consumer-test");
        assert!(matches!(result, Err(GrokAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn legacy_cents_percent_fallback() {
        let raw = serde_json::json!({
            "config": {
                "monthlyLimit": { "val": 2000 },
                "used": { "val": 500 },
                "billingPeriodEnd": "2026-08-11T13:28:26.395580+00:00"
            }
        });
        let snaps = parse_billing_response(&raw, fixture_time(), "grok-consumer-test").unwrap();
        assert_eq!(snaps[0].used, Some(25.0));
        assert_eq!(snaps[0].remaining, Some(75.0));
    }

    #[test]
    fn account_id_is_stable_and_not_email() {
        let id1 = account_id_from_identity(Some("user-123"), Some("person@example.com"));
        let id2 = account_id_from_identity(Some("user-123"), Some("other@example.com"));
        let id3 = account_id_from_identity(Some("user-999"), None);
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert!(!id1.contains('@'));
        assert!(id1.starts_with("grok-consumer-"));
    }

    #[test]
    fn error_snapshot_is_unavailable_without_message() {
        let snap = error_snapshot(
            "grok-consumer-test",
            fixture_time(),
            GrokAdapterError::Timeout,
        );
        assert_eq!(snap.freshness, Freshness::Unavailable);
        assert!(snap.used.is_none());
        assert_eq!(snap.error.as_ref().unwrap().code, ErrorCode::Timeout);
        assert!(snap.error.as_ref().unwrap().message.is_none());
        assert!(snap.validate().is_ok());
    }

    #[test]
    fn load_auth_reads_oidc_entry() {
        let dir = tempfile_dir();
        let path = dir.join("auth.json");
        fs::write(
            &path,
            r#"{
              "https://auth.x.ai::client-id": {
                "key": "access-token",
                "refresh_token": "refresh-token",
                "expires_at": "2099-01-01T00:00:00Z",
                "user_id": "uid-1",
                "email": "redacted@example.com",
                "oidc_client_id": "client-id"
              }
            }"#,
        )
        .unwrap();
        let loaded = load_auth_session(&path).unwrap();
        assert_eq!(loaded.session.key, "access-token");
        assert_eq!(loaded.session.user_id.as_deref(), Some("uid-1"));
        assert!(!session_expired(&loaded.session, Utc::now()));
    }

    fn tempfile_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ai-usage-bar-grok-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
