//! Ollama Pro/cloud usage adapter.
//!
//! The cloud usage endpoint is the primary source for the two account-level
//! totals.  Ollama does not currently include reset timestamps in that JSON,
//! so an authenticated settings-page request may optionally enrich the
//! snapshots with the machine-readable `data-time` values rendered beside
//! each quota.  If that optional request is unavailable, the totals remain
//! live and `resets_at` is deliberately left empty.

use crate::model::*;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use signature::Signer;
use ssh_key::PrivateKey;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const OLLAMA_PROVIDER: Provider = Provider::OllamaCloud;
const OLLAMA_SOURCE: Source = Source::Api;
const OLLAMA_CONFIDENCE: Confidence = Confidence::Exact;
const DEFAULT_USAGE_URL: &str = "https://ollama.com/api/usage";
const DEFAULT_SETTINGS_URL: &str = "https://ollama.com/settings";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_LABEL: &str = "session";
const WEEKLY_LABEL: &str = "weekly";

/// Built-in adapter for Ollama Pro/cloud account quotas.
#[derive(Debug, Clone, Copy, Default)]
pub struct OllamaCloudAdapter;

#[derive(Debug, thiserror::Error)]
pub enum OllamaAdapterError {
    #[error("auth expired or not configured")]
    AuthExpired,
    #[error("timeout waiting for Ollama response")]
    Timeout,
    #[error("schema drift: {0}")]
    SchemaDrift(String),
    #[error("network error")]
    Network,
    #[error("rate limited")]
    RateLimited,
}

impl From<OllamaAdapterError> for AdapterError {
    fn from(error: OllamaAdapterError) -> Self {
        let code = match error {
            OllamaAdapterError::AuthExpired => ErrorCode::AuthExpired,
            OllamaAdapterError::Timeout => ErrorCode::Timeout,
            OllamaAdapterError::SchemaDrift(_) => ErrorCode::SchemaDrift,
            OllamaAdapterError::Network => ErrorCode::Network,
            OllamaAdapterError::RateLimited => ErrorCode::RateLimited,
        };
        // Do not include upstream bodies or credential-derived details.
        AdapterError {
            code,
            message: None,
        }
    }
}

impl ProviderAdapter for OllamaCloudAdapter {
    fn provider(&self) -> Provider {
        OLLAMA_PROVIDER
    }

    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
        fetch_ollama_cloud_snapshots().map_err(AdapterError::from)
    }
}

/// Reset timestamps parsed from the optional settings page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResetTimes {
    pub session: Option<DateTime<Utc>>,
    pub weekly: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    limits: Option<UsageLimits>,
}

#[derive(Debug, Deserialize)]
struct UsageLimits {
    #[serde(default)]
    session: Option<UsageWindow>,
    #[serde(default)]
    weekly: Option<UsageWindow>,
}

#[derive(Debug, Deserialize)]
struct UsageWindow {
    #[serde(default)]
    usage: Option<f64>,
}

enum OllamaAuth {
    ApiKey {
        value: String,
        account_id: String,
    },
    SignedKey {
        private_key: Box<PrivateKey>,
        public_key_blob: Vec<u8>,
        account_id: String,
    },
}

impl OllamaAuth {
    fn account_id(&self) -> &str {
        match self {
            Self::ApiKey { account_id, .. } | Self::SignedKey { account_id, .. } => account_id,
        }
    }

    fn authorization_header(&self, timestamp: i64) -> Result<String, OllamaAdapterError> {
        match self {
            Self::ApiKey { value, .. } => Ok(format!("Bearer {value}")),
            Self::SignedKey {
                private_key,
                public_key_blob,
                ..
            } => {
                let challenge = format!("GET,/api/usage?ts={timestamp}");
                let signature = private_key
                    .key_data()
                    .ed25519()
                    .ok_or(OllamaAdapterError::AuthExpired)?
                    .try_sign(challenge.as_bytes())
                    .map_err(|_| OllamaAdapterError::AuthExpired)?;
                Ok(format!(
                    "{}:{}",
                    BASE64.encode(public_key_blob),
                    BASE64.encode(signature.as_bytes())
                ))
            }
        }
    }
}

/// Fetch the live session and weekly cloud quota snapshots.
pub fn fetch_ollama_cloud_snapshots() -> Result<Vec<UsageSnapshot>, OllamaAdapterError> {
    let auth = load_auth()?;
    let observed_at = Utc::now();
    let body = http_get_usage(&auth, observed_at.timestamp())?;

    // Reset enrichment is deliberately best effort.  A browser cookie is not
    // persisted by this application and is never included in an error.
    let resets = session_cookie()
        .and_then(|cookie| http_get_settings(&cookie).ok())
        .and_then(|html| parse_settings_resets(&html).ok());

    parse_usage_response(&body, observed_at, auth.account_id(), resets)
}

/// Parse the guarded `/api/usage` response into provider-neutral snapshots.
///
/// `usage` is a fraction in the range 0..=1.  Model rows are intentionally
/// ignored; this adapter reports only the account-level totals.
pub fn parse_usage_response(
    raw: &serde_json::Value,
    observed_at: DateTime<Utc>,
    account_id: &str,
    resets: Option<ResetTimes>,
) -> Result<Vec<UsageSnapshot>, OllamaAdapterError> {
    if let Some(error) = raw.get("error") {
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return Err(match code {
            "auth_expired" | "unauthorized" | "forbidden" => OllamaAdapterError::AuthExpired,
            "timeout" => OllamaAdapterError::Timeout,
            _ => OllamaAdapterError::SchemaDrift("usage response contains an error".into()),
        });
    }
    let response: UsageResponse = serde_json::from_value(raw.clone())
        .map_err(|error| OllamaAdapterError::SchemaDrift(error.to_string()))?;
    let limits = response
        .limits
        .ok_or_else(|| OllamaAdapterError::SchemaDrift("missing limits".into()))?;

    let mut snapshots = Vec::new();
    if let Some(session) = limits.session {
        snapshots.push(parse_window(
            session.usage,
            SESSION_LABEL,
            WindowKind::Rolling,
            resets.and_then(|times| times.session),
            observed_at,
            account_id,
        )?);
    }
    if let Some(weekly) = limits.weekly {
        snapshots.push(parse_window(
            weekly.usage,
            WEEKLY_LABEL,
            WindowKind::Weekly,
            resets.and_then(|times| times.weekly),
            observed_at,
            account_id,
        )?);
    }

    if snapshots.is_empty() {
        return Err(OllamaAdapterError::SchemaDrift(
            "limits contains no session or weekly window".into(),
        ));
    }

    Ok(snapshots)
}

fn parse_window(
    usage: Option<f64>,
    label: &str,
    window_kind: WindowKind,
    resets_at: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<UsageSnapshot, OllamaAdapterError> {
    let usage = usage.ok_or_else(|| {
        OllamaAdapterError::SchemaDrift(format!("{label} window is missing usage"))
    })?;
    if !usage.is_finite() || !(0.0..=1.0).contains(&usage) {
        return Err(OllamaAdapterError::SchemaDrift(format!(
            "{label} usage {usage} is outside [0, 1]"
        )));
    }

    let used = usage * 100.0;
    let snapshot = UsageSnapshot {
        provider: OLLAMA_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind,
        unit: "percent".to_string(),
        observed_at,
        source: OLLAMA_SOURCE,
        freshness: Freshness::Live,
        confidence: OLLAMA_CONFIDENCE,
        used: Some(used),
        remaining: Some(100.0 - used),
        limit: Some(100.0),
        unlimited: false,
        resets_at,
        window_label: Some(label.to_string()),
        error: None,
    };
    snapshot
        .validate()
        .map_err(|error| OllamaAdapterError::SchemaDrift(error.to_string()))?;
    Ok(snapshot)
}

/// Parse reset timestamps from the settings page's `data-time` attributes.
///
/// The parser intentionally scopes each search to the corresponding labelled
/// quota block and never uses the rounded countdown text shown to humans.
pub fn parse_settings_resets(html: &str) -> Result<ResetTimes, OllamaAdapterError> {
    let lower = html.to_ascii_lowercase();
    let session_region = labelled_region(&lower, SESSION_LABEL);
    let weekly_region = labelled_region(&lower, WEEKLY_LABEL);
    if session_region.is_none() && weekly_region.is_none() {
        return Err(OllamaAdapterError::SchemaDrift(
            "settings page contains no recognized usage labels".into(),
        ));
    }

    Ok(ResetTimes {
        session: session_region.and_then(parse_data_time),
        weekly: weekly_region.and_then(parse_data_time),
    })
}

fn labelled_region<'a>(lower_html: &'a str, label: &str) -> Option<&'a str> {
    let start = lower_html.find(label)?;
    let after_start = start + label.len();
    let end = [SESSION_LABEL, WEEKLY_LABEL]
        .into_iter()
        .filter(|other| *other != label)
        .filter_map(|other| lower_html[after_start..].find(other))
        .map(|offset| after_start + offset)
        .min()
        .unwrap_or_else(|| (after_start + 8_192).min(lower_html.len()));
    Some(&lower_html[after_start..end])
}

fn parse_data_time(region: &str) -> Option<DateTime<Utc>> {
    let mut search_from = 0;
    while let Some(relative) = region[search_from..].find("data-time") {
        let marker_start = search_from + relative;
        let mut cursor = marker_start + "data-time".len();
        let bytes = region.as_bytes();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            search_from = marker_start + "data-time".len();
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || !matches!(bytes[cursor], b'"' | b'\'') {
            search_from = marker_start + "data-time".len();
            continue;
        }
        let quote = bytes[cursor];
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor > value_start {
            if let Ok(parsed) = DateTime::parse_from_rfc3339(&region[value_start..cursor]) {
                return Some(parsed.with_timezone(&Utc));
            }
        }
        search_from = cursor.saturating_add(1);
    }
    None
}

pub fn error_snapshot(
    account_id: &str,
    observed_at: DateTime<Utc>,
    error: OllamaAdapterError,
) -> UsageSnapshot {
    UsageSnapshot {
        provider: OLLAMA_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind: WindowKind::None,
        unit: "percent".to_string(),
        observed_at,
        source: OLLAMA_SOURCE,
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

fn load_auth() -> Result<OllamaAuth, OllamaAdapterError> {
    if let Some(value) = env_value(&["OLLAMA_API_KEY", "OLLAMA_KEY"]) {
        return Ok(OllamaAuth::ApiKey {
            account_id: format!("ollama-api-{:016x}", simple_hash(value.as_bytes())),
            value,
        });
    }

    let path = private_key_path();
    let raw = fs::read(&path).map_err(|_| OllamaAdapterError::AuthExpired)?;
    let private_key = parse_private_key(&raw)?;
    if private_key.is_encrypted() || !private_key.key_data().is_ed25519() {
        return Err(OllamaAdapterError::AuthExpired);
    }
    let public_key_blob = private_key
        .public_key()
        .to_bytes()
        .map_err(|_| OllamaAdapterError::AuthExpired)?;
    let account_id = format!("ollama-{:016x}", simple_hash(&public_key_blob));
    Ok(OllamaAuth::SignedKey {
        private_key: Box::new(private_key),
        public_key_blob,
        account_id,
    })
}

fn parse_private_key(raw: &[u8]) -> Result<PrivateKey, OllamaAdapterError> {
    if let Ok(private_key) = PrivateKey::from_openssh(raw) {
        return Ok(private_key);
    }

    // Ollama's generated key is a valid OpenSSH key, but some versions of the
    // SSH PEM decoder reject its line wrapping. Decode the PEM payload and
    // pass the canonical binary form to the same format parser as a fallback.
    let encoded: String = std::str::from_utf8(raw)
        .map_err(|_| OllamaAdapterError::AuthExpired)?
        .lines()
        .filter(|line| !line.starts_with('-'))
        .map(str::trim)
        .collect();
    let decoded = BASE64
        .decode(encoded.as_bytes())
        .map_err(|_| OllamaAdapterError::AuthExpired)?;
    PrivateKey::from_bytes(&decoded).map_err(|_| OllamaAdapterError::AuthExpired)
}

fn private_key_path() -> PathBuf {
    if let Some(path) = env_value(&["OLLAMA_ID"]) {
        return PathBuf::from(path);
    }
    if let Some(home) = env_value(&["OLLAMA_HOME"]) {
        return PathBuf::from(home).join("id_ed25519");
    }
    user_home()
        .map(|home| home.join(".ollama").join("id_ed25519"))
        .unwrap_or_else(|| PathBuf::from(".ollama").join("id_ed25519"))
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn env_value(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
}

fn session_cookie() -> Option<String> {
    env_value(&["OLLAMA_SESSION_COOKIE", "OLLAMA_COOKIE"])
        .filter(|cookie| !cookie.chars().any(|ch| matches!(ch, '\r' | '\n')))
}

fn http_get_usage(
    auth: &OllamaAuth,
    timestamp: i64,
) -> Result<serde_json::Value, OllamaAdapterError> {
    let url = format!("{DEFAULT_USAGE_URL}?ts={timestamp}");
    let authorization = auth.authorization_header(timestamp)?;
    let response = ureq::get(&url)
        .set("Authorization", &authorization)
        .set("Accept", "application/json")
        .set("User-Agent", "ai-usage-bar")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(map_ureq_error)?;
    let status = response.status();
    if status == 401 || status == 403 {
        return Err(OllamaAdapterError::AuthExpired);
    }
    if status == 429 {
        return Err(OllamaAdapterError::RateLimited);
    }
    if !(200..300).contains(&status) {
        return Err(OllamaAdapterError::Network);
    }
    response
        .into_json()
        .map_err(|_| OllamaAdapterError::SchemaDrift("usage body is not JSON".into()))
}

fn http_get_settings(cookie: &str) -> Result<String, OllamaAdapterError> {
    let response = ureq::get(DEFAULT_SETTINGS_URL)
        .set("Cookie", cookie)
        .set("Accept", "text/html")
        .set("User-Agent", "ai-usage-bar")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(map_ureq_error)?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(if status == 401 || status == 403 {
            OllamaAdapterError::AuthExpired
        } else {
            OllamaAdapterError::Network
        });
    }
    response
        .into_string()
        .map_err(|_| OllamaAdapterError::SchemaDrift("settings body is not text".into()))
}

fn map_ureq_error(error: ureq::Error) -> OllamaAdapterError {
    match error {
        ureq::Error::Status(401 | 403, _) => OllamaAdapterError::AuthExpired,
        ureq::Error::Status(429, _) => OllamaAdapterError::RateLimited,
        ureq::Error::Status(_, _) => OllamaAdapterError::Network,
        ureq::Error::Transport(transport) => {
            let message = transport.to_string().to_lowercase();
            if message.contains("timed out") || message.contains("timeout") {
                OllamaAdapterError::Timeout
            } else {
                OllamaAdapterError::Network
            }
        }
    }
}

fn simple_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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
            .join("docs/fixtures/ollama_cloud")
            .join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"));
        serde_json::from_str(&content).unwrap()
    }

    fn load_settings_fixture() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures/ollama_cloud/settings.html");
        std::fs::read_to_string(path).expect("failed to read settings fixture")
    }

    #[test]
    fn adapter_reports_ollama_cloud_without_fetching() {
        assert_eq!(OllamaCloudAdapter.provider(), Provider::OllamaCloud);
    }

    #[test]
    fn parse_normal_fixture_emits_session_and_weekly_totals() {
        let raw = load_fixture("normal.json");
        let resets = ResetTimes {
            session: Some(Utc.with_ymd_and_hms(2030, 1, 30, 18, 0, 0).unwrap()),
            weekly: Some(Utc.with_ymd_and_hms(2030, 2, 2, 0, 0, 0).unwrap()),
        };
        let snapshots =
            parse_usage_response(&raw, fixture_time(), "ollama-test", Some(resets)).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].window_kind, WindowKind::Rolling);
        assert_eq!(snapshots[0].window_label.as_deref(), Some(SESSION_LABEL));
        assert_eq!(snapshots[0].used, Some(37.0));
        assert_eq!(snapshots[0].remaining, Some(63.0));
        assert_eq!(snapshots[0].resets_at, resets.session);
        assert_eq!(snapshots[1].window_kind, WindowKind::Weekly);
        assert_eq!(snapshots[1].used, Some(18.4));
        assert_eq!(snapshots[1].remaining, Some(81.6));
        assert_eq!(snapshots[1].resets_at, resets.weekly);
        assert!(snapshots.iter().all(|snapshot| snapshot.validate().is_ok()));
    }

    #[test]
    fn totals_survive_missing_reset_enrichment() {
        let raw = load_fixture("normal.json");
        let snapshots = parse_usage_response(&raw, fixture_time(), "ollama-test", None).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots
            .iter()
            .all(|snapshot| snapshot.resets_at.is_none()));
    }

    #[test]
    fn parse_settings_data_time_attributes() {
        let html = load_settings_fixture();
        let resets = parse_settings_resets(&html).unwrap();
        assert_eq!(
            resets.session,
            Some(Utc.with_ymd_and_hms(2030, 1, 30, 18, 0, 0).unwrap())
        );
        assert_eq!(
            resets.weekly,
            Some(Utc.with_ymd_and_hms(2030, 2, 2, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn settings_parser_does_not_use_unrelated_data_time() {
        let html = r#"
          <span data-time="2030-01-01T00:00:00Z">header</span>
          <h2>Session usage</h2><span>No timestamp available</span>
          <h2>Weekly usage</h2><span>No timestamp available</span>
        "#;
        let resets = parse_settings_resets(html).unwrap();
        assert_eq!(resets, ResetTimes::default());
    }

    #[test]
    fn settings_parser_reports_schema_drift_without_usage_labels() {
        let error = parse_settings_resets("<html><body>signed out</body></html>").unwrap_err();
        assert!(matches!(error, OllamaAdapterError::SchemaDrift(_)));
    }

    #[test]
    fn malformed_usage_is_schema_drift() {
        let raw = load_fixture("malformed.json");
        let error = parse_usage_response(&raw, fixture_time(), "ollama-test", None).unwrap_err();
        assert!(matches!(error, OllamaAdapterError::SchemaDrift(_)));
    }

    #[test]
    fn auth_failure_fixture_maps_to_auth_expired() {
        let raw = load_fixture("auth_failure.json");
        let error = parse_usage_response(&raw, fixture_time(), "ollama-test", None).unwrap_err();
        assert!(matches!(error, OllamaAdapterError::AuthExpired));
    }

    #[test]
    fn missing_limits_is_schema_drift() {
        let error = parse_usage_response(
            &serde_json::json!({"activity": {}}),
            fixture_time(),
            "ollama-test",
            None,
        )
        .unwrap_err();
        assert!(matches!(error, OllamaAdapterError::SchemaDrift(_)));
    }

    #[test]
    fn signed_out_settings_fixture_keeps_resets_optional() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures/ollama_cloud/settings_signed_out.html");
        let html = std::fs::read_to_string(path).unwrap();
        assert!(matches!(
            parse_settings_resets(&html),
            Err(OllamaAdapterError::SchemaDrift(_))
        ));
    }

    #[test]
    fn api_key_account_id_is_redacted() {
        let auth = OllamaAuth::ApiKey {
            value: "secret-value".into(),
            account_id: format!("ollama-api-{:016x}", simple_hash(b"secret-value")),
        };
        assert!(auth.account_id().starts_with("ollama-api-"));
        assert!(!auth.account_id().contains("secret-value"));
    }
}
