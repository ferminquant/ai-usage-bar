//! Kimi Code membership usage adapter.
//!
//! Source (verified in `docs/spikes/kimi-spike.md`): the managed endpoint
//! behind the official Kimi Code CLI `/usage` command,
//! `GET {base}/coding/v1/usages`, reusing the OAuth session the user already
//! created with `kimi login` (`~/.kimi-code/credentials/kimi-code.json`).
//!
//! Reported: the weekly plan window (`window_kind=weekly`), one snapshot per
//! reported `limits[]` row (e.g. the rolling 5-hour window), and the Extra
//! Usage wallet as a `metric_kind=credits` snapshot when present. Not
//! reported: the shared monthly membership pool cap, the monthly spending
//! cap/used amounts, the plan tier, the wallet currency, or the Kimi Open
//! Platform (API-key billing) balance.
//!
//! Missing or corrupt credential files are treated as "not configured" by
//! [`session_available`]; `build_registry` registers the adapter only when a
//! session exists, otherwise the provider is registered as not configured.
//! The adapter itself never initiates the device-code flow.

use crate::model::*;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const KIMI_PROVIDER: Provider = Provider::Kimi;
const KIMI_SOURCE: Source = Source::Cli;
const KIMI_CONFIDENCE: Confidence = Confidence::Exact;

const DEFAULT_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const USAGES_PATH: &str = "/usages";
const DEFAULT_TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);
const FIXED_POINT_CENTS: f64 = 1_000_000.0;
const WEEKLY_LABEL: &str = "primary";
const CREDITS_LABEL: &str = "extra usage";

#[derive(Debug, Clone, Default)]
pub struct KimiAdapter;

#[derive(Debug, thiserror::Error)]
pub enum KimiAdapterError {
    #[error("auth expired or not configured")]
    AuthExpired,
    #[error("timeout waiting for usage response")]
    Timeout,
    #[error("schema drift: {0}")]
    SchemaDrift(String),
    #[error("network error")]
    Network,
    #[error("rate limited")]
    RateLimited,
}

impl From<KimiAdapterError> for AdapterError {
    fn from(error: KimiAdapterError) -> Self {
        let code = match error {
            KimiAdapterError::AuthExpired => ErrorCode::AuthExpired,
            KimiAdapterError::Timeout => ErrorCode::Timeout,
            KimiAdapterError::SchemaDrift(_) => ErrorCode::SchemaDrift,
            KimiAdapterError::Network => ErrorCode::Network,
            KimiAdapterError::RateLimited => ErrorCode::RateLimited,
        };
        // Never attach upstream bodies or tokens — redacted code only.
        AdapterError { code, message: None }
    }
}

impl ProviderAdapter for KimiAdapter {
    fn provider(&self) -> Provider {
        KIMI_PROVIDER
    }

    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
        fetch_kimi_snapshots().map_err(AdapterError::from)
    }
}

// ---------------------------------------------------------------------------
// Credential store (`~/.kimi-code/credentials/kimi-code.json`)
// ---------------------------------------------------------------------------

/// On-disk OAuth token bundle, snake_case to match the CLI wire format.
/// `expires_at` is Unix seconds; the adapter tolerates string or number.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct KimiCredentialFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_in: Option<i64>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

fn credential_path() -> PathBuf {
    if let Ok(custom) = std::env::var("KIMI_CREDENTIALS_JSON") {
        return PathBuf::from(custom);
    }
    let home = if let Ok(custom) = std::env::var("KIMI_CODE_HOME") {
        PathBuf::from(custom)
    } else {
        user_home()
            .map(|home| home.join(".kimi-code"))
            .unwrap_or_else(|| PathBuf::from(".kimi-code"))
    };
    home.join("credentials").join("kimi-code.json")
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Whether a usable local CLI session exists (file present, parseable, and
/// carrying an access token). Used by the registry bootstrap to distinguish
/// `not_configured` from `auth_expired`; performs no network access.
pub fn session_available() -> bool {
    let path = credential_path();
    path.exists() && load_credential_file(&path).is_ok()
}

fn load_credential_file(path: &Path) -> Result<KimiCredentialFile, KimiAdapterError> {
    let raw = fs::read_to_string(path).map_err(|_| KimiAdapterError::AuthExpired)?;
    let session: KimiCredentialFile =
        serde_json::from_str(&raw).map_err(|_| KimiAdapterError::AuthExpired)?;
    if session
        .access_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
    {
        return Err(KimiAdapterError::AuthExpired);
    }
    Ok(session)
}

fn session_expired(session: &KimiCredentialFile, now: DateTime<Utc>) -> bool {
    let Some(expires_at) = session.expires_at.as_ref() else {
        return false;
    };
    let Some(expiry) = parse_unix_seconds(expires_at) else {
        return false;
    };
    expiry <= now + chrono::Duration::seconds(60)
}

fn parse_unix_seconds(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    let raw = match value {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }?;
    let seconds = if raw.abs() >= 100_000_000_000 {
        raw / 1000
    } else {
        raw
    };
    Utc.timestamp_opt(seconds, 0).single()
}

fn persist_credential_file(
    path: &Path,
    session: &KimiCredentialFile,
) -> Result<(), KimiAdapterError> {
    let document = serde_json::to_string_pretty(session).map_err(|_| KimiAdapterError::Network)?;
    fs::write(path, format!("{document}\n")).map_err(|_| KimiAdapterError::Network)
}

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    #[serde(default)]
    error: Option<String>,
}

fn refresh_access_token(
    session: &mut KimiCredentialFile,
    path: &Path,
) -> Result<(), KimiAdapterError> {
    let refresh = session
        .refresh_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .ok_or(KimiAdapterError::AuthExpired)?;
    let token_url =
        std::env::var("KIMI_TOKEN_URL").unwrap_or_else(|_| DEFAULT_TOKEN_URL.to_string());
    let body = format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        KIMI_CLIENT_ID,
        urlencoding_minimal(refresh)
    );

    // The CLI treats 401/403/`invalid_grant` as unrecoverable and retries
    // 429/5xx and transport failures; the daemon bounds total time anyway.
    for attempt in 0..2 {
        match refresh_once(&token_url, &body) {
            Ok(mut parsed) => {
                session.access_token = parsed.access_token.take();
                if let Some(refresh_token) = parsed
                    .refresh_token
                    .as_deref()
                    .filter(|token| !token.is_empty())
                {
                    session.refresh_token = Some(refresh_token.to_string());
                }
                if let Some(seconds) = parsed.expires_in {
                    let expiry = Utc::now().timestamp() + seconds.max(0);
                    session.expires_at = Some(serde_json::Value::from(expiry));
                }
                // Best-effort persist; still use the refreshed token if the
                // write fails so the CLI session stays healthy.
                let _ = persist_credential_file(path, session);
                return Ok(());
            }
            Err(KimiAdapterError::AuthExpired) => return Err(KimiAdapterError::AuthExpired),
            Err(_) if attempt == 0 => {
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(final_error) => return Err(final_error),
        }
    }
    Err(KimiAdapterError::Network)
}

fn refresh_once(
    token_url: &str,
    body: &str,
) -> Result<TokenRefreshResponse, KimiAdapterError> {
    let response = ureq::post(token_url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .timeout(REFRESH_TIMEOUT)
        .send_string(body)
        .map_err(map_ureq_error)?;

    let status = response.status();
    let parsed: TokenRefreshResponse = response
        .into_json()
        .map_err(|_| KimiAdapterError::AuthExpired)?;
    if status == 401
        || status == 403
        || parsed.error.as_deref() == Some("invalid_grant")
        || parsed
            .access_token
            .as_deref()
            .is_none_or(|token| token.trim().is_empty())
    {
        return Err(KimiAdapterError::AuthExpired);
    }
    if !(200..300).contains(&status) {
        return Err(if status == 429 {
            KimiAdapterError::RateLimited
        } else {
            KimiAdapterError::Network
        });
    }
    if parsed.expires_in.is_none() {
        return Err(KimiAdapterError::AuthExpired);
    }
    Ok(parsed)
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

fn map_ureq_error(error: ureq::Error) -> KimiAdapterError {
    match error {
        ureq::Error::Status(401 | 403, _) => KimiAdapterError::AuthExpired,
        ureq::Error::Status(429, _) => KimiAdapterError::RateLimited,
        ureq::Error::Status(_, _) => KimiAdapterError::Network,
        ureq::Error::Transport(transport) => {
            let message = transport.to_string().to_lowercase();
            if message.contains("timed out") || message.contains("timeout") {
                KimiAdapterError::Timeout
            } else {
                KimiAdapterError::Network
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Usage response shape (camelCase JSON from the managed /usages endpoint)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsagesResponse {
    #[serde(default)]
    usage: Option<UsageSummary>,
    #[serde(default)]
    limits: Option<Vec<LimitRow>>,
    #[serde(default)]
    booster_wallet: Option<BoosterWallet>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummary {
    #[serde(default)]
    used: Option<serde_json::Value>,
    #[serde(default)]
    limit: Option<serde_json::Value>,
    #[serde(default)]
    remaining: Option<serde_json::Value>,
    #[serde(default)]
    reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct LimitRow {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    window: Option<LimitWindow>,
    #[serde(default)]
    detail: Option<LimitDetail>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitWindow {
    #[serde(default)]
    duration: Option<serde_json::Value>,
    #[serde(default)]
    time_unit: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitDetail {
    #[serde(default)]
    used: Option<serde_json::Value>,
    #[serde(default)]
    limit: Option<serde_json::Value>,
    #[serde(default)]
    remaining: Option<serde_json::Value>,
    #[serde(default)]
    reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoosterWallet {
    #[serde(default)]
    balance: Option<WalletBalance>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WalletBalance {
    #[serde(default)]
    amount: Option<serde_json::Value>,
    #[serde(default)]
    amount_left: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn simple_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Stable redacted account id derived from the local credential path; never
/// exposes the username or path. Stable across token refreshes.
pub fn account_id_from_credential_path(path: &Path) -> String {
    format!("kimi-{:016x}", simple_hash(path.to_string_lossy().as_bytes()))
}

fn parse_decimal(value: Option<&serde_json::Value>) -> Option<f64> {
    let parsed = match value? {
        serde_json::Value::String(raw) => raw.parse::<f64>().ok(),
        serde_json::Value::Number(n) => n.as_f64(),
        _ => None,
    }?;
    parsed.is_finite().then_some(parsed)
}

fn parse_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(raw) => raw.parse::<i64>().ok(),
        _ => None,
    }
}

fn parse_iso8601(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?.as_str()?)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Map a `limits[]` window to the contract window kind. The evidenced case is
/// 300 TIME_UNIT_MINUTE (rolling 5 hours); exact daily/weekly/monthly
/// durations map like the Codex adapter, and unknown units fall back to
/// rolling so a schema surprise never drops a usable row.
fn window_kind_from_window(window: Option<&LimitWindow>) -> WindowKind {
    let Some(window) = window else {
        return WindowKind::Rolling;
    };
    let Some(duration) = parse_i64(window.duration.as_ref()) else {
        return WindowKind::Rolling;
    };
    match window.time_unit.as_deref() {
        Some("TIME_UNIT_MINUTE") => match duration {
            1440 => WindowKind::Daily,
            10080 => WindowKind::Weekly,
            43200 => WindowKind::Monthly,
            _ => WindowKind::Rolling,
        },
        Some("TIME_UNIT_HOUR") => match duration {
            24 => WindowKind::Daily,
            168 => WindowKind::Weekly,
            720 => WindowKind::Monthly,
            _ => WindowKind::Rolling,
        },
        Some("TIME_UNIT_DAY") => match duration {
            7 => WindowKind::Weekly,
            30 | 31 => WindowKind::Monthly,
            _ => WindowKind::Rolling,
        },
        Some("TIME_UNIT_WEEK") => match duration {
            1 => WindowKind::Weekly,
            4 => WindowKind::Monthly,
            _ => WindowKind::Rolling,
        },
        _ => WindowKind::Rolling,
    }
}

/// The plan summary is normalized to percent (`limit=100`); the CLI renders
/// the same numbers as "% left". A limit that is not 100 scales the row so
/// the snapshot stays contract-safe.
#[allow(clippy::too_many_arguments)]
fn percent_window_snapshot(
    used: f64,
    limit: f64,
    remaining: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
    label: Option<String>,
    window_kind: WindowKind,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Option<UsageSnapshot> {
    if !used.is_finite() || !limit.is_finite() || limit <= 0.0 {
        return None;
    }
    let (used, remaining, limit) = if limit == 100.0 {
        (used, remaining, limit)
    } else {
        let scale = 100.0 / limit;
        (used * scale, remaining.map(|value| value * scale), 100.0)
    };
    Some(UsageSnapshot {
        provider: KIMI_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind,
        unit: "percent".to_string(),
        observed_at,
        source: KIMI_SOURCE,
        freshness: Freshness::Live,
        confidence: KIMI_CONFIDENCE,
        used: Some(used),
        remaining,
        limit: Some(limit),
        unlimited: false,
        resets_at,
        window_label: label,
        error: None,
    })
}

fn parse_usage_summary(
    usage: &UsageSummary,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Option<UsageSnapshot> {
    let used = parse_decimal(usage.used.as_ref())?;
    let limit = parse_decimal(usage.limit.as_ref())?;
    percent_window_snapshot(
        used,
        limit,
        parse_decimal(usage.remaining.as_ref()),
        parse_iso8601(usage.reset_time.as_ref()),
        Some(WEEKLY_LABEL.to_string()),
        WindowKind::Weekly,
        observed_at,
        account_id,
    )
}

fn parse_limit_row(
    row: &LimitRow,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Option<UsageSnapshot> {
    let detail = row.detail.as_ref()?;
    let used = parse_decimal(detail.used.as_ref())?;
    let limit = parse_decimal(detail.limit.as_ref())?;
    percent_window_snapshot(
        used,
        limit,
        parse_decimal(detail.remaining.as_ref()),
        parse_iso8601(detail.reset_time.as_ref()),
        row.name.clone(),
        window_kind_from_window(row.window.as_ref()),
        observed_at,
        account_id,
    )
}

fn fixed_point_to_cents(value: i64) -> i64 {
    let cents = value as f64 / FIXED_POINT_CENTS;
    if cents > 0.0 && cents < 1.0 {
        1
    } else {
        cents.round() as i64
    }
}

/// Extra Usage wallet, fixed-point cents. Absent or non-positive wallet means
/// no credits snapshot (never zero, never unlimited).
fn parse_wallet(
    wallet: &BoosterWallet,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Option<UsageSnapshot> {
    let balance = wallet.balance.as_ref()?;
    let total = parse_i64(balance.amount.as_ref())?;
    let left = parse_i64(balance.amount_left.as_ref())?;
    if total <= 0 || left < 0 || left > total {
        return None;
    }
    Some(UsageSnapshot {
        provider: KIMI_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Credits,
        window_kind: WindowKind::None,
        unit: "cents".to_string(),
        observed_at,
        source: KIMI_SOURCE,
        freshness: Freshness::Live,
        confidence: KIMI_CONFIDENCE,
        used: Some(fixed_point_to_cents(left) as f64),
        remaining: None,
        limit: Some(fixed_point_to_cents(total) as f64),
        unlimited: false,
        resets_at: None,
        window_label: Some(CREDITS_LABEL.to_string()),
        error: None,
    })
}

/// Parse a `/usages` JSON body into provider-neutral snapshots.
///
/// Emits the weekly plan summary as the primary quota, one quota snapshot
/// per parseable `limits[]` row, and the Extra Usage wallet as a credits
/// snapshot when present. Rows with non-numeric values are skipped; a
/// response with no parseable row is schema drift. Error-shaped bodies map
/// to `auth_expired` / `timeout` / `schema_drift`.
pub fn parse_usages_response(
    raw: &serde_json::Value,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<Vec<UsageSnapshot>, KimiAdapterError> {
    let error_field = raw.get("error").or_else(|| raw.get("_error"));
    let has_usage_shape = raw.get("usage").is_some()
        || raw.get("limits").is_some()
        || raw.get("boosterWallet").is_some();
    if let Some(error) = error_field {
        if !has_usage_shape {
            let code = error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            return Err(match code {
                "auth_expired" | "unauthorized" | "forbidden" | "invalid_grant" => {
                    KimiAdapterError::AuthExpired
                }
                "timeout" => KimiAdapterError::Timeout,
                _ => KimiAdapterError::SchemaDrift("usage response contains an error".into()),
            });
        }
    }

    let response: UsagesResponse = serde_json::from_value(raw.clone())
        .map_err(|error| KimiAdapterError::SchemaDrift(error.to_string()))?;

    let mut snapshots = Vec::new();
    if let Some(usage) = &response.usage {
        if let Some(snapshot) = parse_usage_summary(usage, observed_at, account_id) {
            snapshots.push(snapshot);
        }
    }
    if let Some(rows) = &response.limits {
        for row in rows {
            if let Some(snapshot) = parse_limit_row(row, observed_at, account_id) {
                snapshots.push(snapshot);
            }
        }
    }
    if let Some(wallet) = &response.booster_wallet {
        if let Some(snapshot) = parse_wallet(wallet, observed_at, account_id) {
            snapshots.push(snapshot);
        }
    }

    snapshots.retain(|snapshot| snapshot.validate().is_ok());
    if snapshots.is_empty() {
        return Err(KimiAdapterError::SchemaDrift(
            "no parseable usage, limit row, or wallet in response".into(),
        ));
    }

    Ok(snapshots)
}

pub fn error_snapshot(
    account_id: &str,
    observed_at: DateTime<Utc>,
    error: KimiAdapterError,
) -> UsageSnapshot {
    UsageSnapshot {
        provider: KIMI_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind: WindowKind::None,
        unit: "percent".to_string(),
        observed_at,
        source: KIMI_SOURCE,
        freshness: Freshness::Unavailable,
        confidence: Confidence::Unknown,
        used: None,
        remaining: None,
        limit: None,
        unlimited: false,
        resets_at: None,
        window_label: None,
        error: Some(AdapterError::from(error)),
    }
}

// ---------------------------------------------------------------------------
// HTTP fetch
// ---------------------------------------------------------------------------

fn http_get_usages(access_token: &str) -> Result<serde_json::Value, KimiAdapterError> {
    let base = std::env::var("KIMI_CODE_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string();
    let url = format!("{base}{USAGES_PATH}");

    let response = ureq::get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Accept", "application/json")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(map_ureq_error)?;

    let status = response.status();
    if status == 401 || status == 403 {
        return Err(KimiAdapterError::AuthExpired);
    }
    if status == 429 {
        return Err(KimiAdapterError::RateLimited);
    }
    if !(200..300).contains(&status) {
        return Err(KimiAdapterError::Network);
    }
    response
        .into_json()
        .map_err(|_| KimiAdapterError::SchemaDrift("usage body is not JSON".into()))
}

/// Fetch live Kimi Code snapshots using the local CLI OAuth session.
pub fn fetch_kimi_snapshots() -> Result<Vec<UsageSnapshot>, KimiAdapterError> {
    let path = credential_path();
    if !path.exists() {
        return Err(KimiAdapterError::AuthExpired);
    }
    let mut session = load_credential_file(&path)?;
    let now = Utc::now();

    if session_expired(&session, now) {
        refresh_access_token(&mut session, &path)?;
    }

    let account_id = account_id_from_credential_path(&path);
    let token = session
        .access_token
        .as_deref()
        .unwrap_or_default()
        .to_string();

    let body = match http_get_usages(&token) {
        Ok(body) => body,
        Err(KimiAdapterError::AuthExpired) => {
            refresh_access_token(&mut session, &path)?;
            http_get_usages(
                session
                    .access_token
                    .as_deref()
                    .ok_or(KimiAdapterError::AuthExpired)?,
            )?
        }
        Err(error) => return Err(error),
    };

    parse_usages_response(&body, Utc::now(), &account_id)
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
            .join("docs/fixtures/kimi")
            .join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"));
        serde_json::from_str(&content).unwrap()
    }

    #[test]
    fn adapter_reports_kimi_without_fetching() {
        assert_eq!(KimiAdapter.provider(), Provider::Kimi);
    }

    #[test]
    fn parse_normal_fixture() {
        let raw = load_fixture("normal.json");
        let snapshots = parse_usages_response(&raw, fixture_time(), "kimi-test").unwrap();
        assert_eq!(snapshots.len(), 3);

        let weekly = snapshots
            .iter()
            .find(|snapshot| snapshot.window_label.as_deref() == Some(WEEKLY_LABEL))
            .unwrap();
        assert_eq!(weekly.provider, Provider::Kimi);
        assert_eq!(weekly.metric_kind, MetricKind::Quota);
        assert_eq!(weekly.window_kind, WindowKind::Weekly);
        assert_eq!(weekly.unit, "percent");
        assert_eq!(weekly.used, Some(33.0));
        assert_eq!(weekly.remaining, Some(67.0));
        assert_eq!(weekly.limit, Some(100.0));
        assert_eq!(weekly.source, Source::Cli);
        assert!(weekly.resets_at.is_some());

        let rolling = snapshots
            .iter()
            .find(|snapshot| snapshot.window_kind == WindowKind::Rolling)
            .unwrap();
        assert_eq!(rolling.used, Some(2.0));
        assert_eq!(rolling.remaining, Some(98.0));

        let credits = snapshots
            .iter()
            .find(|snapshot| snapshot.metric_kind == MetricKind::Credits)
            .unwrap();
        assert_eq!(credits.used, Some(1250.0));
        assert_eq!(credits.remaining, None);
        assert_eq!(credits.limit, Some(1500.0));
        assert_eq!(credits.unit, "cents");
        assert_eq!(credits.window_label.as_deref(), Some(CREDITS_LABEL));
        assert!(!credits.unlimited);

        assert!(snapshots.iter().all(|snapshot| snapshot.validate().is_ok()));
    }

    #[test]
    fn parse_multiple_windows_fixture() {
        let raw = load_fixture("multiple_windows.json");
        let snapshots = parse_usages_response(&raw, fixture_time(), "kimi-test").unwrap();
        assert_eq!(snapshots.len(), 3);

        let weekly = snapshots
            .iter()
            .find(|snapshot| snapshot.window_label.as_deref() == Some(WEEKLY_LABEL))
            .unwrap();
        assert_eq!(weekly.used, Some(40.0));

        let highspeed = snapshots
            .iter()
            .find(|snapshot| snapshot.window_label.as_deref() == Some("highspeed"))
            .unwrap();
        assert_eq!(highspeed.used, Some(0.0));
        assert_eq!(highspeed.window_kind, WindowKind::Rolling);

        let unnamed = snapshots
            .iter()
            .filter(|snapshot| snapshot.window_label.is_none())
            .collect::<Vec<_>>();
        assert_eq!(unnamed.len(), 1);
        assert_eq!(unnamed[0].used, Some(1.0));
        assert_eq!(unnamed[0].window_kind, WindowKind::Rolling);
    }

    #[test]
    fn parse_unlimited_or_missing_fixture() {
        let raw = load_fixture("unlimited_or_missing.json");
        let snapshots = parse_usages_response(&raw, fixture_time(), "kimi-test").unwrap();
        assert_eq!(snapshots.len(), 1);
        let weekly = &snapshots[0];
        assert_eq!(weekly.used, Some(0.0));
        assert_eq!(weekly.remaining, Some(100.0));
        assert_eq!(weekly.limit, Some(100.0));
        assert!(weekly.resets_at.is_none());
        assert!(!weekly.unlimited, "absent fields are not unlimited");
    }

    #[test]
    fn parse_auth_failure_fixture() {
        let raw = load_fixture("auth_failure.json");
        let result = parse_usages_response(&raw, fixture_time(), "kimi-test");
        assert!(matches!(result, Err(KimiAdapterError::AuthExpired)));
    }

    #[test]
    fn parse_timeout_fixture() {
        let raw = load_fixture("timeout.json");
        let result = parse_usages_response(&raw, fixture_time(), "kimi-test");
        assert!(matches!(result, Err(KimiAdapterError::Timeout)));
    }

    #[test]
    fn parse_malformed_fixture_is_schema_drift() {
        let raw = load_fixture("malformed.json");
        let result = parse_usages_response(&raw, fixture_time(), "kimi-test");
        assert!(matches!(result, Err(KimiAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn non_numeric_sibling_row_is_skipped_but_weekly_survives() {
        let raw = serde_json::json!({
            "usage": { "used": "33", "limit": "100", "remaining": "67", "resetTime": "2026-08-10T09:20:45Z" },
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "used": "not-a-number", "limit": "100" }
                }
            ]
        });
        let snapshots = parse_usages_response(&raw, fixture_time(), "kimi-test").unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].window_label.as_deref(), Some(WEEKLY_LABEL));
    }

    #[test]
    fn non_percent_limit_is_normalized() {
        let raw = serde_json::json!({
            "usage": { "used": "40", "limit": "1000", "remaining": "960" }
        });
        let snapshots = parse_usages_response(&raw, fixture_time(), "kimi-test").unwrap();
        let weekly = &snapshots[0];
        assert_eq!(weekly.used, Some(4.0));
        assert_eq!(weekly.remaining, Some(96.0));
        assert_eq!(weekly.limit, Some(100.0));
    }

    #[test]
    fn out_of_range_usage_is_rejected_as_schema_drift() {
        let raw = serde_json::json!({
            "usage": { "used": "120", "limit": "100", "remaining": "-20" }
        });
        let result = parse_usages_response(&raw, fixture_time(), "kimi-test");
        assert!(matches!(result, Err(KimiAdapterError::SchemaDrift(_))));
    }

    #[test]
    fn wallet_absent_means_no_credits_snapshot() {
        let raw = load_fixture("multiple_windows.json");
        let snapshots = parse_usages_response(&raw, fixture_time(), "kimi-test").unwrap();
        assert!(
            snapshots
                .iter()
                .all(|snapshot| snapshot.metric_kind != MetricKind::Credits)
        );
    }

    #[test]
    fn account_id_is_stable_and_does_not_leak_path() {
        let path = Path::new("C:\\Users\\somebody\\.kimi-code\\credentials\\kimi-code.json");
        let first = account_id_from_credential_path(path);
        let second = account_id_from_credential_path(path);
        let other = Path::new("C:\\Users\\else\\.kimi-code\\credentials\\kimi-code.json");
        let other_id = account_id_from_credential_path(other);
        assert_eq!(first, second);
        assert_ne!(first, other_id);
        assert!(first.starts_with("kimi-"));
        assert!(!first.contains("somebody"));
        assert!(!first.contains("kimi-code.json"));
    }

    #[test]
    fn error_snapshot_is_unavailable_without_message() {
        let snapshot = error_snapshot("kimi-test", fixture_time(), KimiAdapterError::Timeout);
        assert_eq!(snapshot.freshness, Freshness::Unavailable);
        assert!(snapshot.used.is_none());
        assert_eq!(snapshot.error.as_ref().unwrap().code, ErrorCode::Timeout);
        assert!(snapshot.error.as_ref().unwrap().message.is_none());
        assert!(snapshot.validate().is_ok());
    }

    #[test]
    fn session_expiry_reads_unix_seconds() {
        let now = fixture_time();
        let valid = KimiCredentialFile {
            access_token: Some("token".into()),
            refresh_token: None,
            expires_at: Some(serde_json::json!(now.timestamp() + 3600)),
            scope: None,
            token_type: None,
            expires_in: None,
            extra: Default::default(),
        };
        assert!(!session_expired(&valid, now));

        let expired = KimiCredentialFile {
            expires_at: Some(serde_json::json!(now.timestamp() - 1)),
            ..valid.clone()
        };
        assert!(session_expired(&expired, now));
    }

    #[test]
    fn refresh_persists_unix_seconds_expiry() {
        let mut session = KimiCredentialFile {
            access_token: Some("old".into()),
            refresh_token: Some("refresh-token".into()),
            expires_at: None,
            scope: None,
            token_type: None,
            expires_in: None,
            extra: Default::default(),
        };
        session.access_token = Some("refreshed".into());
        session.expires_at = Some(serde_json::Value::from(Utc::now().timestamp() + 3600));
        let document = serde_json::to_string_pretty(&session).unwrap();
        assert!(document.contains("\"access_token\": \"refreshed\""));
        assert!(document.contains("\"expires_at\":"));
        let reloaded: KimiCredentialFile = serde_json::from_str(&document).unwrap();
        assert_eq!(reloaded.access_token.as_deref(), Some("refreshed"));
        assert!(parse_unix_seconds(reloaded.expires_at.as_ref().unwrap()).is_some());
    }
}
