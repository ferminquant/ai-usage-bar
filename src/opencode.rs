//! OpenCode Go usage adapter.
//!
//! OpenCode's Go gateway exposes the authoritative rolling/weekly/monthly
//! counters through the same authenticated usage endpoint used by the Go
//! console.  The adapter reads the existing local OpenCode Go API key without
//! persisting or displaying it.  When no key is available, it retains the
//! local SQLite ledger estimator as an explicitly inferred fallback.

use crate::model::*;
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Timelike, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OPENCODE_PROVIDER: Provider = Provider::OpenCodeGo;
const OPENCODE_ACCOUNT_ID: &str = "opencode-local";
const OPENCODE_DATA_DIR_ENV: &str = "OPENCODE_DATA_DIR";
const OPENCODE_DB_ENV: &str = "OPENCODE_DB";
const OPENCODE_AUTH_ENV: &str = "OPENCODE_AUTH_JSON";
const OPENCODE_USAGE_URL_ENV: &str = "OPENCODE_USAGE_URL";
const DEFAULT_USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const FIVE_HOURS: i64 = 5 * 60 * 60;
const SEVEN_DAYS: i64 = 7 * 24 * 60 * 60;
const WEEKLY_LIMIT_USD: f64 = 30.0;
const MONTHLY_LIMIT_USD: f64 = 60.0;
const ROLLING_LIMIT_USD: f64 = 12.0;

/// User-adjustable reset anchors for the inferred estimator.
///
/// Each value is the next known reset instant in UTC.  Once it passes, the
/// estimator advances the same phase by one week or one calendar month.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenCodeResetSettings {
    pub weekly_reset_at: Option<DateTime<Utc>>,
    pub monthly_reset_at: Option<DateTime<Utc>>,
}

/// Built-in local OpenCode Go estimator.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCodeGoAdapter {
    settings: OpenCodeResetSettings,
}

impl OpenCodeGoAdapter {
    pub fn new(settings: OpenCodeResetSettings) -> Self {
        Self { settings }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenCodeAdapterError {
    #[error("OpenCode local database is not configured")]
    NotConfigured,
    #[error("unable to read OpenCode local database")]
    Database,
    #[error("OpenCode Go API key is missing or expired")]
    AuthExpired,
    #[error("timeout waiting for OpenCode Go usage response")]
    Timeout,
    #[error("OpenCode Go usage request failed")]
    Network,
    #[error("OpenCode Go usage request was rate limited")]
    RateLimited,
    #[error("OpenCode local database schema drift: {0}")]
    SchemaDrift(String),
}

impl From<OpenCodeAdapterError> for AdapterError {
    fn from(error: OpenCodeAdapterError) -> Self {
        let code = match error {
            OpenCodeAdapterError::NotConfigured => ErrorCode::Network,
            OpenCodeAdapterError::Database => ErrorCode::Network,
            OpenCodeAdapterError::AuthExpired => ErrorCode::AuthExpired,
            OpenCodeAdapterError::Timeout => ErrorCode::Timeout,
            OpenCodeAdapterError::Network => ErrorCode::Network,
            OpenCodeAdapterError::RateLimited => ErrorCode::RateLimited,
            OpenCodeAdapterError::SchemaDrift(_) => ErrorCode::SchemaDrift,
        };
        AdapterError {
            code,
            message: None,
        }
    }
}

impl ProviderAdapter for OpenCodeGoAdapter {
    fn provider(&self) -> Provider {
        OPENCODE_PROVIDER
    }

    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
        fetch_opencode_snapshots(self.settings).map_err(AdapterError::from)
    }
}

#[derive(Debug, Clone)]
struct UsageEvent {
    observed_at: DateTime<Utc>,
    model_id: String,
    cost_usd: f64,
    weight: f64,
}

#[derive(Debug, Deserialize, Default)]
struct MessageInfo {
    #[serde(default)]
    role: Option<String>,
    #[serde(rename = "providerID", default)]
    provider_id: Option<String>,
    #[serde(rename = "modelID", default)]
    model_id: Option<String>,
    #[serde(default)]
    cost: Option<f64>,
}

/// Whether the OpenCode local session database can be found without opening
/// it, or a local OpenCode Go API key is available. Used by registry bootstrap
/// to distinguish "not configured" from a live adapter that later encounters
/// a read/schema/network failure.
pub fn opencode_data_available() -> bool {
    opencode_api_key().is_some() || opencode_database_path().is_some()
}

/// Resolve OpenCode's SQLite database using the same XDG/Windows conventions
/// as the upstream CLI, with explicit test/developer overrides first.
pub fn opencode_database_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os(OPENCODE_DB_ENV).map(PathBuf::from) {
        return path.is_file().then_some(path);
    }

    opencode_data_directories()
        .into_iter()
        .flat_map(|directory| database_candidates(&directory))
        .filter(|path| path.is_file())
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
}

fn opencode_data_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(data_dir) = env::var_os(OPENCODE_DATA_DIR_ENV) {
        directories.push(PathBuf::from(data_dir));
    }
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        directories.push(PathBuf::from(data_home).join("opencode"));
    }
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        directories.push(PathBuf::from(local_app_data).join("opencode"));
    }
    if let Some(home) = env::var_os("USERPROFILE") {
        directories.push(
            PathBuf::from(&home)
                .join(".local")
                .join("share")
                .join("opencode"),
        );
    }
    if let Some(home) = env::var_os("HOME") {
        directories.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("opencode"),
        );
    }
    directories
}

#[derive(Debug, Deserialize)]
struct OpenCodeAuthEntry {
    #[serde(default)]
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeAuthFile {
    #[serde(rename = "opencode-go", default)]
    opencode_go: Option<OpenCodeAuthEntry>,
}

fn opencode_auth_paths() -> Vec<PathBuf> {
    if let Some(path) = env::var_os(OPENCODE_AUTH_ENV).map(PathBuf::from) {
        return vec![path];
    }
    opencode_data_directories()
        .into_iter()
        .map(|directory| directory.join("auth.json"))
        .collect()
}

fn opencode_api_key() -> Option<String> {
    for name in ["OPENCODE_GO_API_KEY", "OPENCODE_API_KEY"] {
        if let Some(value) = env::var_os(name)
            .map(|value| value.to_string_lossy().into_owned())
            .filter(|value| !value.trim().is_empty())
        {
            return Some(value);
        }
    }

    for path in opencode_auth_paths() {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(auth) = serde_json::from_str::<OpenCodeAuthFile>(&raw) else {
            continue;
        };
        if let Some(key) = auth
            .opencode_go
            .and_then(|entry| entry.key)
            .filter(|key| !key.trim().is_empty())
        {
            return Some(key);
        }
    }
    None
}

fn database_candidates(directory: &Path) -> Vec<PathBuf> {
    let preferred = directory.join("opencode.db");
    let mut candidates = vec![preferred.clone()];
    let Ok(entries) = fs::read_dir(directory) else {
        return candidates;
    };
    let mut channel_databases: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("opencode-") && name.ends_with(".db"))
        })
        .collect();
    channel_databases.sort();
    candidates.extend(channel_databases);
    candidates
}

/// Fetch local inferred snapshots for the three Go windows.
pub fn fetch_opencode_snapshots(
    settings: OpenCodeResetSettings,
) -> Result<Vec<UsageSnapshot>, OpenCodeAdapterError> {
    if let Some(api_key) = opencode_api_key() {
        let body = http_get_usage(&api_key)?;
        return parse_usage_response(&body, Utc::now());
    }

    let observed_at = Utc::now();
    let path = opencode_database_path().ok_or(OpenCodeAdapterError::NotConfigured)?;
    fetch_from_database(&path, settings, observed_at)
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    usage: Option<UsageWindows>,
}

#[derive(Debug, Deserialize)]
struct UsageWindows {
    #[serde(default)]
    rolling: Option<UsageWindow>,
    #[serde(default)]
    weekly: Option<UsageWindow>,
    #[serde(default)]
    monthly: Option<UsageWindow>,
}

#[derive(Debug, Deserialize)]
struct UsageWindow {
    #[serde(default)]
    percent: Option<serde_json::Value>,
    #[serde(rename = "resetsAt", default)]
    resets_at: Option<String>,
}

fn http_get_usage(api_key: &str) -> Result<serde_json::Value, OpenCodeAdapterError> {
    let url = env::var(OPENCODE_USAGE_URL_ENV).unwrap_or_else(|_| DEFAULT_USAGE_URL.to_string());
    let response = ureq::get(&url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Accept", "application/json")
        .set("User-Agent", "ai-usage-bar")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(map_ureq_error)?;
    let status = response.status();
    if status == 401 || status == 403 {
        return Err(OpenCodeAdapterError::AuthExpired);
    }
    if status == 429 {
        return Err(OpenCodeAdapterError::RateLimited);
    }
    if !(200..300).contains(&status) {
        return Err(OpenCodeAdapterError::Network);
    }
    response
        .into_json()
        .map_err(|_| OpenCodeAdapterError::SchemaDrift("usage body is not JSON".into()))
}

fn map_ureq_error(error: ureq::Error) -> OpenCodeAdapterError {
    match error {
        ureq::Error::Status(401 | 403, _) => OpenCodeAdapterError::AuthExpired,
        ureq::Error::Status(429, _) => OpenCodeAdapterError::RateLimited,
        ureq::Error::Status(_, _) => OpenCodeAdapterError::Network,
        ureq::Error::Transport(transport) => {
            let message = transport.to_string().to_lowercase();
            if message.contains("timed out") || message.contains("timeout") {
                OpenCodeAdapterError::Timeout
            } else {
                OpenCodeAdapterError::Network
            }
        }
    }
}

fn parse_usage_response(
    raw: &serde_json::Value,
    observed_at: DateTime<Utc>,
) -> Result<Vec<UsageSnapshot>, OpenCodeAdapterError> {
    let response: UsageResponse = serde_json::from_value(raw.clone())
        .map_err(|error| OpenCodeAdapterError::SchemaDrift(error.to_string()))?;
    let usage = response
        .usage
        .ok_or_else(|| OpenCodeAdapterError::SchemaDrift("missing usage object".into()))?;

    let windows = [
        ("5-hour", WindowKind::Rolling, usage.rolling),
        ("weekly", WindowKind::Weekly, usage.weekly),
        ("monthly", WindowKind::Monthly, usage.monthly),
    ];
    let mut snapshots = Vec::with_capacity(windows.len());
    for (label, window_kind, window) in windows {
        let window = window.ok_or_else(|| {
            OpenCodeAdapterError::SchemaDrift(format!("missing {label} usage window"))
        })?;
        snapshots.push(parse_usage_window(
            label,
            window_kind,
            &window,
            observed_at,
        )?);
    }
    Ok(snapshots)
}

fn parse_usage_window(
    label: &str,
    window_kind: WindowKind,
    window: &UsageWindow,
    observed_at: DateTime<Utc>,
) -> Result<UsageSnapshot, OpenCodeAdapterError> {
    let percent = parse_finite_number(window.percent.as_ref()).ok_or_else(|| {
        OpenCodeAdapterError::SchemaDrift(format!("{label} usage is missing percent"))
    })?;
    if !(0.0..=100.0).contains(&percent) {
        return Err(OpenCodeAdapterError::SchemaDrift(format!(
            "{label} usage percent is outside [0, 100]"
        )));
    }
    let resets_at = window
        .resets_at
        .as_deref()
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|date| date.with_timezone(&Utc))
                .map_err(|_| {
                    OpenCodeAdapterError::SchemaDrift(format!(
                        "{label} reset timestamp is not RFC3339"
                    ))
                })
        })
        .transpose()?;

    let snapshot = UsageSnapshot {
        provider: OPENCODE_PROVIDER,
        account_id: "opencode-go-api".to_string(),
        metric_kind: MetricKind::Quota,
        window_kind,
        unit: "percent".to_string(),
        observed_at,
        source: Source::Api,
        freshness: Freshness::Live,
        confidence: Confidence::Exact,
        used: Some(percent),
        remaining: Some(100.0 - percent),
        limit: Some(100.0),
        unlimited: false,
        resets_at,
        window_label: Some(label.to_string()),
        error: None,
    };
    snapshot
        .validate()
        .map_err(|error| OpenCodeAdapterError::SchemaDrift(error.to_string()))?;
    Ok(snapshot)
}

fn parse_finite_number(value: Option<&serde_json::Value>) -> Option<f64> {
    let parsed = match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }?;
    parsed.is_finite().then_some(parsed)
}

fn fetch_from_database(
    path: &Path,
    settings: OpenCodeResetSettings,
    now: DateTime<Utc>,
) -> Result<Vec<UsageSnapshot>, OpenCodeAdapterError> {
    let mut bounds = WindowBounds::new(now, settings);
    let events = read_usage_events(path, bounds.earliest_start())?;
    bounds.rolling_reset = rolling_reset_from_events(&events, bounds.rolling_start, now);
    let mut confidence = Confidence::Inferred;

    let weighted_cost = |start: DateTime<Utc>, end: DateTime<Utc>| {
        events
            .iter()
            .filter(|event| event.observed_at >= start && event.observed_at < end)
            .map(|event| event.cost_usd * event.weight)
            .sum::<f64>()
    };

    // An unrecognized model is still included at 1x so the bar does not
    // silently report zero. Downgrade the confidence to make that assumption
    // visible in copied details/diagnostics.
    if events.iter().any(|event| {
        event.cost_usd > 0.0 && event.weight == 1.0 && is_unknown_model(&event.model_id)
    }) {
        confidence = Confidence::Unknown;
    }

    let rolling_cost = weighted_cost(bounds.rolling_start, now);
    let weekly_cost = weighted_cost(bounds.weekly_start, now);
    let monthly_cost = weighted_cost(bounds.monthly_start, now);

    Ok(vec![
        make_snapshot(
            "5-hour",
            WindowKind::Rolling,
            rolling_cost,
            ROLLING_LIMIT_USD,
            bounds.rolling_reset,
            now,
            confidence,
        ),
        make_snapshot(
            "weekly",
            WindowKind::Weekly,
            weekly_cost,
            WEEKLY_LIMIT_USD,
            bounds.weekly_reset,
            now,
            confidence,
        ),
        make_snapshot(
            "monthly",
            WindowKind::Monthly,
            monthly_cost,
            MONTHLY_LIMIT_USD,
            bounds.monthly_reset,
            now,
            confidence,
        ),
    ])
}

fn read_usage_events(
    path: &Path,
    earliest_start: DateTime<Utc>,
) -> Result<Vec<UsageEvent>, OpenCodeAdapterError> {
    if let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if let Ok(events) = read_usage_events_from_connection(&connection, earliest_start) {
            return Ok(events);
        }
    }

    // OpenCode keeps a live WAL beside the database. On Windows/WSL mounts,
    // a read-only connection can reject that WAL with a transient disk-I/O
    // error while the desktop process is writing. An immutable read is a safe
    // fallback: it never takes locks or writes, and the next refresh can
    // observe the latest checkpoint.
    let uri = format!(
        "file:{}?immutable=1",
        path.to_string_lossy().replace('\\', "/")
    );
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|_| OpenCodeAdapterError::Database)?;
    read_usage_events_from_connection(&connection, earliest_start)
}

fn read_usage_events_from_connection(
    connection: &Connection,
    earliest_start: DateTime<Utc>,
) -> Result<Vec<UsageEvent>, OpenCodeAdapterError> {
    connection
        .busy_timeout(Duration::from_millis(500))
        .map_err(|_| OpenCodeAdapterError::Database)?;

    let mut statement = connection
        .prepare("SELECT time_created, data FROM message WHERE time_created >= ?1")
        .map_err(|error| OpenCodeAdapterError::SchemaDrift(error.to_string()))?;
    let rows = statement
        .query_map(params![earliest_start.timestamp_millis()], |row| {
            let timestamp: i64 = row.get(0)?;
            let data: String = row.get(1)?;
            Ok((timestamp, data))
        })
        .map_err(|error| OpenCodeAdapterError::SchemaDrift(error.to_string()))?;

    let mut events = Vec::new();
    for row in rows {
        let (timestamp, data) = row.map_err(|_| OpenCodeAdapterError::Database)?;
        let message: serde_json::Value = serde_json::from_str(&data)
            .map_err(|error| OpenCodeAdapterError::SchemaDrift(error.to_string()))?;
        let info_value = message.get("info").unwrap_or(&message);
        let info: MessageInfo = serde_json::from_value(info_value.clone())
            .map_err(|error| OpenCodeAdapterError::SchemaDrift(error.to_string()))?;
        if info.role.as_deref() != Some("assistant")
            || !is_opencode_provider(info.provider_id.as_deref())
        {
            continue;
        }
        let Some(model_id) = info.model_id else {
            continue;
        };
        let Some(cost_usd) = info.cost.filter(|cost| cost.is_finite() && *cost >= 0.0) else {
            continue;
        };
        let Some(observed_at) = Utc.timestamp_millis_opt(timestamp).single() else {
            continue;
        };
        let weight = model_weight(&model_id);
        events.push(UsageEvent {
            observed_at,
            model_id,
            cost_usd,
            weight,
        });
    }
    Ok(events)
}

fn is_opencode_provider(provider_id: Option<&str>) -> bool {
    // `opencode` is the separate free/local provider in the same ledger. Only
    // the hosted Go provider contributes to this subscription estimate.
    provider_id == Some("opencode-go")
}

fn make_snapshot(
    label: &str,
    window_kind: WindowKind,
    weighted_cost: f64,
    limit_usd: f64,
    resets_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    confidence: Confidence,
) -> UsageSnapshot {
    let used = (weighted_cost.max(0.0) / limit_usd * 100.0).clamp(0.0, 100.0);
    let used = if used == 0.0 { 0.0 } else { used };
    UsageSnapshot {
        provider: OPENCODE_PROVIDER,
        account_id: OPENCODE_ACCOUNT_ID.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind,
        unit: "percent".to_string(),
        observed_at,
        source: Source::LocalApi,
        freshness: Freshness::Live,
        confidence,
        used: Some(used),
        remaining: Some(100.0 - used),
        limit: Some(100.0),
        unlimited: false,
        resets_at: Some(resets_at),
        window_label: Some(label.to_string()),
        error: None,
    }
}

#[derive(Debug, Clone, Copy)]
struct WindowBounds {
    rolling_start: DateTime<Utc>,
    weekly_start: DateTime<Utc>,
    monthly_start: DateTime<Utc>,
    rolling_reset: DateTime<Utc>,
    weekly_reset: DateTime<Utc>,
    monthly_reset: DateTime<Utc>,
}

impl WindowBounds {
    fn new(now: DateTime<Utc>, settings: OpenCodeResetSettings) -> Self {
        let rolling_start = now - ChronoDuration::seconds(FIVE_HOURS);
        let rolling_reset = now + ChronoDuration::seconds(FIVE_HOURS);
        let (weekly_start, weekly_reset) = recurring_weekly_bounds(now, settings.weekly_reset_at);
        let (monthly_start, monthly_reset) =
            recurring_monthly_bounds(now, settings.monthly_reset_at);
        Self {
            rolling_start,
            weekly_start,
            monthly_start,
            rolling_reset,
            weekly_reset,
            monthly_reset,
        }
    }

    fn earliest_start(self) -> DateTime<Utc> {
        self.rolling_start
            .min(self.weekly_start)
            .min(self.monthly_start)
    }
}

fn rolling_reset_from_events(
    events: &[UsageEvent],
    rolling_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    // The hosted service's rolling window is refreshed by the latest usage.
    // Use the newest local assistant event as the best available reset
    // estimate; with no recent event, show a full five-hour horizon.
    events
        .iter()
        .filter(|event| event.observed_at >= rolling_start && event.observed_at < now)
        .map(|event| event.observed_at + ChronoDuration::seconds(FIVE_HOURS))
        .max()
        .unwrap_or_else(|| now + ChronoDuration::seconds(FIVE_HOURS))
}

fn recurring_weekly_bounds(
    now: DateTime<Utc>,
    configured_next: Option<DateTime<Utc>>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let mut next = configured_next.unwrap_or_else(|| default_weekly_reset(now));
    while next <= now {
        next += ChronoDuration::seconds(SEVEN_DAYS);
    }
    (next - ChronoDuration::seconds(SEVEN_DAYS), next)
}

fn recurring_monthly_bounds(
    now: DateTime<Utc>,
    configured_next: Option<DateTime<Utc>>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let mut next = configured_next.unwrap_or_else(|| default_monthly_reset(now));
    while next <= now {
        next = shift_month(next, 1);
    }
    (shift_month(next, -1), next)
}

fn default_weekly_reset(now: DateTime<Utc>) -> DateTime<Utc> {
    let days_until_monday = (7 - now.weekday().num_days_from_monday()) % 7;
    let days_until_monday = if days_until_monday == 0 {
        7
    } else {
        days_until_monday
    };
    let date = now.date_naive() + ChronoDuration::days(days_until_monday as i64);
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
}

fn default_monthly_reset(now: DateTime<Utc>) -> DateTime<Utc> {
    let first = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).expect("valid month");
    let first_next = shift_month_date(first, 1);
    Utc.from_utc_datetime(&first_next.and_hms_opt(0, 0, 0).expect("valid midnight"))
}

fn shift_month(value: DateTime<Utc>, delta: i32) -> DateTime<Utc> {
    let date = shift_month_date(value.date_naive(), delta);
    Utc.from_utc_datetime(
        &date
            .and_hms_nano_opt(
                value.hour(),
                value.minute(),
                value.second(),
                value.nanosecond(),
            )
            .expect("valid shifted datetime"),
    )
}

fn shift_month_date(date: NaiveDate, delta: i32) -> NaiveDate {
    let total = date.year() * 12 + date.month0() as i32 + delta;
    let year = total.div_euclid(12);
    let month0 = total.rem_euclid(12) as u32;
    let month = month0 + 1;
    let max_day = NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|first| first.checked_add_months(chrono::Months::new(1)))
        .and_then(|first_next| first_next.pred_opt())
        .map(|last| last.day())
        .unwrap_or(28);
    NaiveDate::from_ymd_opt(year, month, date.day().min(max_day)).expect("valid shifted date")
}

fn is_unknown_model(model: &str) -> bool {
    model_weight_known(model).is_none()
}

fn model_weight(model: &str) -> f64 {
    model_weight_known(model).unwrap_or(1.0)
}

fn model_weight_known(model: &str) -> Option<f64> {
    Some(match model {
        // Models whose published Usage tier is $15 are charged against the
        // $60 Go monthly allowance at 4x their raw model cost.
        "grok-4.5" | "gpt-5.6-luna" | "glm-5.3" | "kimi-k3" | "mimo-v2.5-pro" | "qwen3.8-max"
        | "deepseek-v4-pro" => 4.0,
        // Models whose published Usage tier is $60 use the raw model cost.
        "glm-5.2" | "glm-5.1" | "kimi-k2.7-code" | "kimi-k2.6" | "mimo-v2.5" | "minimax-m3"
        | "minimax-m2.7" | "minimax-m2.5" | "qwen3.7-max" | "qwen3.7-plus" | "qwen3.6-plus"
        | "deepseek-v4-flash" | "hy3" => 1.0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn instant(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn published_usage_tiers_map_to_expected_weights() {
        assert_eq!(model_weight("qwen3.8-max"), 4.0);
        assert_eq!(model_weight("glm-5.3"), 4.0);
        assert_eq!(model_weight("glm-5.2"), 1.0);
        assert_eq!(model_weight("future-model"), 1.0);
        assert!(is_unknown_model("future-model"));
    }

    #[test]
    fn parses_authoritative_go_usage_windows() {
        let observed_at = instant("2026-08-14T13:00:00Z");
        let raw = serde_json::json!({
            "usage": {
                "rolling": {"status": "ok", "percent": 22, "resetsAt": "2026-08-14T17:43:38.318Z"},
                "weekly": {"status": "ok", "percent": 83, "resetsAt": "2026-08-17T00:00:00.318Z"},
                "monthly": {"status": "ok", "percent": 60, "resetsAt": "2026-09-05T14:03:20.318Z"}
            }
        });

        let snapshots = parse_usage_response(&raw, observed_at).unwrap();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].source, Source::Api);
        assert_eq!(snapshots[0].confidence, Confidence::Exact);
        assert_eq!(snapshots[0].used, Some(22.0));
        assert_eq!(snapshots[1].used, Some(83.0));
        assert_eq!(snapshots[2].used, Some(60.0));
        assert_eq!(
            snapshots[0].resets_at,
            Some(instant("2026-08-14T17:43:38.318Z"))
        );
    }

    #[test]
    fn rejects_authoritative_usage_when_a_window_is_missing() {
        let raw = serde_json::json!({
            "usage": {
                "rolling": {"percent": 22, "resetsAt": "2026-08-14T17:43:38.318Z"},
                "weekly": {"percent": 83, "resetsAt": "2026-08-17T00:00:00.318Z"}
            }
        });
        assert!(matches!(
            parse_usage_response(&raw, instant("2026-08-14T13:00:00Z")),
            Err(OpenCodeAdapterError::SchemaDrift(message))
                if message.contains("monthly")
        ));
    }

    #[test]
    fn configured_weekly_anchor_controls_the_window() {
        let now = instant("2026-08-07T18:00:00Z");
        let configured = instant("2026-08-10T15:00:00Z");
        let (start, end) = recurring_weekly_bounds(now, Some(configured));
        assert_eq!(start, instant("2026-08-03T15:00:00Z"));
        assert_eq!(end, configured);
    }

    #[test]
    fn configured_monthly_anchor_clamps_february() {
        let now = instant("2026-02-03T00:00:00Z");
        let configured = instant("2026-02-28T12:00:00Z");
        let (start, end) = recurring_monthly_bounds(now, Some(configured));
        assert_eq!(start, instant("2026-01-28T12:00:00Z"));
        assert_eq!(end, configured);
    }

    #[test]
    fn reads_only_opencode_go_assistant_costs() {
        let temp = tempfile_path();
        let connection = Connection::open(&temp).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE message (time_created INTEGER NOT NULL, data TEXT NOT NULL);",
            )
            .unwrap();
        let when = instant("2026-08-07T12:00:00Z").timestamp_millis();
        let mut insert = connection
            .prepare("INSERT INTO message(time_created, data) VALUES (?1, ?2)")
            .unwrap();
        insert
            .execute(params![
                when,
                serde_json::json!({
                    "info": {"role":"assistant","providerID":"opencode-go","modelID":"qwen3.8-max","cost":1.25}
                })
                .to_string()
            ])
            .unwrap();
        insert
            .execute(params![
                when + 2 * 60 * 60 * 1000,
                serde_json::json!({
                    "role":"assistant", "providerID":"opencode-go", "modelID":"glm-5.2", "cost":0.5
                })
                .to_string()
            ])
            .unwrap();
        insert
            .execute(params![
                when,
                serde_json::json!({
                    "info": {"role":"assistant","providerID":"ollama","modelID":"qwen3.8-max","cost":99.0}
                })
                .to_string()
            ])
            .unwrap();
        drop(insert);

        let snapshots = fetch_from_database(
            &temp,
            OpenCodeResetSettings::default(),
            instant("2026-08-07T18:00:00Z"),
        )
        .unwrap();
        let weekly = snapshots
            .iter()
            .find(|snapshot| snapshot.window_label.as_deref() == Some("weekly"))
            .unwrap();
        assert!((weekly.used.unwrap() - ((1.25 * 4.0 + 0.5) / 30.0 * 100.0)).abs() < 1e-9);
        let rolling = snapshots
            .iter()
            .find(|snapshot| snapshot.window_label.as_deref() == Some("5-hour"))
            .unwrap();
        assert_eq!(rolling.resets_at, Some(instant("2026-08-07T19:00:00Z")));
        let _ = fs::remove_file(temp);
    }

    fn tempfile_path() -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!("ai-usage-bar-opencode-{}.db", std::process::id()));
        let _ = fs::remove_file(&path);
        // Keep the import explicit so the test fails loudly if the temporary
        // path cannot be created on the host.
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(b"").unwrap();
        path
    }
}
