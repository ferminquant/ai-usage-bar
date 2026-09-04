//! z.ai GLM Coding Plan usage adapter.
//!
//! z.ai's coding-plan tooling exposes the quota information used by its
//! subscription UI at the monitor endpoint.  The endpoint is not part of the
//! public OpenAPI catalog, so the parser is deliberately defensive and keeps
//! unknown limit types out of the provider-neutral model.  Both the legacy
//! `TOKENS_LIMIT` response and the current credit-based `CREDIT_LIMIT`
//! response are supported.

use crate::{model::*, security::stable_hash};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::time::Duration;

const ZAI_PROVIDER: Provider = Provider::Zai;
const ZAI_SOURCE: Source = Source::Api;
const ZAI_CONFIDENCE: Confidence = Confidence::Exact;
const DEFAULT_USAGE_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const USAGE_URL_ENV: &str = "ZAI_USAGE_URL";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

const FIVE_HOUR_LABEL: &str = "5-hour";
const WEEKLY_LABEL: &str = "weekly";
const MCP_LABEL: &str = "MCP";

/// Built-in adapter for z.ai GLM Coding Plan quotas.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZaiAdapter;

#[derive(Debug, thiserror::Error)]
pub enum ZaiAdapterError {
    #[error("z.ai API key is missing or expired")]
    AuthExpired,
    #[error("timeout waiting for z.ai usage response")]
    Timeout,
    #[error("z.ai usage response schema drift: {0}")]
    SchemaDrift(String),
    #[error("z.ai usage request failed")]
    Network,
    #[error("z.ai usage request was rate limited")]
    RateLimited,
}

impl From<ZaiAdapterError> for AdapterError {
    fn from(error: ZaiAdapterError) -> Self {
        let code = match error {
            ZaiAdapterError::AuthExpired => ErrorCode::AuthExpired,
            ZaiAdapterError::Timeout => ErrorCode::Timeout,
            ZaiAdapterError::SchemaDrift(_) => ErrorCode::SchemaDrift,
            ZaiAdapterError::Network => ErrorCode::Network,
            ZaiAdapterError::RateLimited => ErrorCode::RateLimited,
        };
        // Do not attach upstream bodies or credential-derived details.
        AdapterError {
            code,
            message: None,
        }
    }
}

impl ProviderAdapter for ZaiAdapter {
    fn provider(&self) -> Provider {
        ZAI_PROVIDER
    }

    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError> {
        fetch_zai_snapshots().map_err(AdapterError::from)
    }
}

/// Whether a z.ai credential is available to the adapter.
///
/// The normal credential is `ZAI_API_KEY`; `GLM_API_KEY` is the documented
/// compatibility name used by several z.ai clients.  Claude-compatible z.ai
/// setups use `ANTHROPIC_AUTH_TOKEN`, but it is accepted only when the paired
/// base URL points at z.ai so an unrelated Anthropic credential is never sent
/// to this provider.
pub fn zai_api_key_available() -> bool {
    load_api_key().is_some()
}

/// Return a stable, non-secret account identifier derived from an API key.
pub fn account_id_from_api_key(api_key: &str) -> String {
    format!("zai-api-{:016x}", stable_hash(api_key.trim().as_bytes()))
}

/// Fetch live z.ai coding-plan usage using the local API-key environment.
pub fn fetch_zai_snapshots() -> Result<Vec<UsageSnapshot>, ZaiAdapterError> {
    let api_key = load_api_key().ok_or(ZaiAdapterError::AuthExpired)?;
    let body = http_get_usage(&api_key)?;
    parse_usage_response(&body, Utc::now(), &account_id_from_api_key(&api_key))
}

/// Parse a z.ai monitor response into provider-neutral snapshots.
///
/// The monitor endpoint has returned both `TOKENS_LIMIT` (legacy plans) and
/// `CREDIT_LIMIT` (current plans) rows.  Quota rows are normalized to a
/// percentage so they can drive the compact usage pill.  `TIME_LIMIT` rows
/// are preserved as count-based request snapshots (currently the monthly MCP
/// allowance) rather than being conflated with model quota.
pub fn parse_usage_response(
    raw: &Value,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<Vec<UsageSnapshot>, ZaiAdapterError> {
    if let Some(error) = response_error(raw) {
        return Err(error);
    }

    // The normal envelope is { data: { limits: [...] } }.  Accepting a direct
    // { limits: [...] } body keeps deterministic fixtures and future proxy
    // responses compatible without changing the external contract.
    let data = raw.get("data").unwrap_or(raw);
    let limits = data
        .get("limits")
        .and_then(Value::as_array)
        .ok_or_else(|| ZaiAdapterError::SchemaDrift("missing data.limits".into()))?;

    let mut snapshots = Vec::new();
    for row in limits {
        let Some(object) = row.as_object() else {
            continue;
        };
        let Some(kind) = object.get("type").and_then(Value::as_str) else {
            continue;
        };

        let parsed = match kind {
            "TOKENS_LIMIT" | "CREDIT_LIMIT" => parse_quota_row(object, observed_at, account_id),
            "TIME_LIMIT" => parse_request_row(object, observed_at, account_id),
            // New server-side limit types should not be guessed at.  Sibling
            // rows remain useful when one unknown type is added upstream.
            _ => Ok(None),
        };
        // A malformed sibling must not hide valid windows from the same
        // refresh. If every supported row is malformed, the empty result
        // below becomes one redacted schema-drift failure for the provider.
        if let Ok(Some(snapshot)) = parsed {
            snapshots.push(snapshot);
        }
    }

    if snapshots.is_empty() {
        return Err(ZaiAdapterError::SchemaDrift(
            "limits contains no supported usage windows".into(),
        ));
    }

    Ok(snapshots)
}

fn parse_quota_row(
    row: &serde_json::Map<String, Value>,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<Option<UsageSnapshot>, ZaiAdapterError> {
    let Some((window_kind, label)) = classify_window(row, false) else {
        return Ok(None);
    };

    let percentage = percentage_used(row)?;
    let snapshot = UsageSnapshot {
        provider: ZAI_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Quota,
        window_kind,
        unit: "percent".to_string(),
        observed_at,
        source: ZAI_SOURCE,
        freshness: Freshness::Live,
        confidence: ZAI_CONFIDENCE,
        used: Some(percentage),
        remaining: Some(100.0 - percentage),
        limit: Some(100.0),
        unlimited: false,
        resets_at: parse_reset_time(row.get("nextResetTime")),
        window_label: Some(label.to_string()),
        error: None,
    };
    snapshot
        .validate()
        .map_err(|error| ZaiAdapterError::SchemaDrift(error.to_string()))?;
    Ok(Some(snapshot))
}

fn parse_request_row(
    row: &serde_json::Map<String, Value>,
    observed_at: DateTime<Utc>,
    account_id: &str,
) -> Result<Option<UsageSnapshot>, ZaiAdapterError> {
    let Some((window_kind, _)) = classify_window(row, true) else {
        return Ok(None);
    };
    let Some(limit) = number_from(row, "usage") else {
        return Ok(None);
    };
    if !limit.is_finite() || limit < 0.0 {
        return Err(ZaiAdapterError::SchemaDrift(
            "TIME_LIMIT total is outside the valid range".into(),
        ));
    }

    let percentage = number_from(row, "percentage");
    if let Some(percentage) = percentage {
        if !percentage.is_finite() || !(0.0..=100.0).contains(&percentage) {
            return Err(ZaiAdapterError::SchemaDrift(
                "TIME_LIMIT percentage is outside [0, 100]".into(),
            ));
        }
    }
    let used =
        number_from(row, "currentValue").or_else(|| percentage.map(|value| limit * value / 100.0));
    let Some(used) = used else {
        return Ok(None);
    };
    let remaining = number_from(row, "remaining").unwrap_or(limit - used);
    if !used.is_finite()
        || used < 0.0
        || !remaining.is_finite()
        || remaining < 0.0
        || used > limit
        || remaining > limit
    {
        return Err(ZaiAdapterError::SchemaDrift(
            "TIME_LIMIT usage is outside the reported total".into(),
        ));
    }

    let snapshot = UsageSnapshot {
        provider: ZAI_PROVIDER,
        account_id: account_id.to_string(),
        metric_kind: MetricKind::Requests,
        window_kind,
        unit: "requests".to_string(),
        observed_at,
        source: ZAI_SOURCE,
        freshness: Freshness::Live,
        confidence: ZAI_CONFIDENCE,
        used: Some(used),
        remaining: Some(remaining),
        limit: Some(limit),
        unlimited: false,
        resets_at: parse_reset_time(row.get("nextResetTime")),
        window_label: Some(MCP_LABEL.to_string()),
        error: None,
    };
    snapshot
        .validate()
        .map_err(|error| ZaiAdapterError::SchemaDrift(error.to_string()))?;
    Ok(Some(snapshot))
}

/// Determine the product window from z.ai's compact unit/number pair.
///
/// `unit=3, number=5` is the rolling five-hour window and `unit=6,
/// number=1|7` is the weekly window.  `TIME_LIMIT` currently uses `unit=5,
/// number=1` for the monthly MCP allowance.
fn classify_window(
    row: &serde_json::Map<String, Value>,
    time_limit: bool,
) -> Option<(WindowKind, &'static str)> {
    let unit = row.get("unit").and_then(Value::as_i64);
    let number = row.get("number").and_then(Value::as_i64);
    match (unit, number) {
        (Some(3), Some(5)) => Some((WindowKind::Rolling, FIVE_HOUR_LABEL)),
        (Some(6), Some(1 | 7)) => Some((WindowKind::Weekly, WEEKLY_LABEL)),
        (Some(5), Some(1)) if time_limit => Some((WindowKind::Monthly, MCP_LABEL)),
        _ => None,
    }
}

fn percentage_used(row: &serde_json::Map<String, Value>) -> Result<f64, ZaiAdapterError> {
    let percentage = if let Some(percentage) = number_from(row, "percentage") {
        percentage
    } else {
        let total = number_from(row, "usage").ok_or_else(|| {
            ZaiAdapterError::SchemaDrift("quota row is missing percentage and usable totals".into())
        })?;
        let used = number_from(row, "currentValue")
            .or_else(|| number_from(row, "remaining").map(|remaining| total - remaining))
            .ok_or_else(|| {
                ZaiAdapterError::SchemaDrift(
                    "quota row is missing percentage and usable totals".into(),
                )
            })?;
        if !total.is_finite() || total < 0.0 || !used.is_finite() || used < 0.0 || used > total {
            return Err(ZaiAdapterError::SchemaDrift(
                "quota totals are outside the valid range".into(),
            ));
        }
        if total == 0.0 {
            if used == 0.0 {
                0.0
            } else {
                return Err(ZaiAdapterError::SchemaDrift(
                    "quota total is zero with non-zero usage".into(),
                ));
            }
        } else {
            used / total * 100.0
        }
    };
    if !percentage.is_finite() || !(0.0..=100.0).contains(&percentage) {
        return Err(ZaiAdapterError::SchemaDrift(format!(
            "quota percentage {percentage} is outside [0, 100]"
        )));
    }
    Ok(percentage)
}

fn number_from(row: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    row.get(key).and_then(Value::as_f64)
}

fn parse_reset_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(Value::as_i64)
        .filter(|millis| *millis > 0)
        .and_then(|millis| Utc.timestamp_millis_opt(millis).single())
}

fn response_error(raw: &Value) -> Option<ZaiAdapterError> {
    let success = raw.get("success").and_then(Value::as_bool);
    let error_code = raw.get("code").and_then(parse_code);
    let explicit_failure = success == Some(false);
    let code_failure = error_code.is_some_and(|value| value != 0 && value != 200);
    if !explicit_failure && !code_failure {
        return None;
    }

    if let Some(text) = raw
        .get("message")
        .or_else(|| raw.get("msg"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
    {
        if text.contains("timeout") {
            return Some(ZaiAdapterError::Timeout);
        }
        if text.contains("auth")
            || text.contains("unauthoriz")
            || text.contains("forbidden")
            || text.contains("expired")
            || text.contains("invalid")
        {
            return Some(ZaiAdapterError::AuthExpired);
        }
        if text.contains("rate") || text.contains("limit") || text.contains("quota") {
            return Some(ZaiAdapterError::RateLimited);
        }
    }

    Some(match error_code {
        Some(401 | 403 | 1000 | 1001 | 1002 | 1003 | 1005 | 1309) => ZaiAdapterError::AuthExpired,
        Some(1113 | 1302 | 1303 | 1304 | 1305 | 1308 | 1310 | 1316..=1321) => {
            ZaiAdapterError::RateLimited
        }
        _ => ZaiAdapterError::SchemaDrift("usage response contains an error".into()),
    })
}

fn parse_code(value: &Value) -> Option<i64> {
    match value {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn load_api_key() -> Option<String> {
    for name in ["ZAI_API_KEY", "GLM_API_KEY"] {
        if let Some(value) = std::env::var(name).ok().and_then(normalize_api_key) {
            return Some(value);
        }
    }

    let base_url = std::env::var("ANTHROPIC_BASE_URL").ok()?;
    if !base_url.to_ascii_lowercase().contains("api.z.ai") {
        return None;
    }
    std::env::var("ANTHROPIC_AUTH_TOKEN")
        .ok()
        .and_then(normalize_api_key)
}

fn normalize_api_key(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

fn http_get_usage(api_key: &str) -> Result<Value, ZaiAdapterError> {
    let url = std::env::var(USAGE_URL_ENV).unwrap_or_else(|_| DEFAULT_USAGE_URL.to_string());
    let response = ureq::get(&url)
        // z.ai's official usage-query plugin sends the raw API key in this
        // header. The monitor service also accepts Bearer keys, but raw is
        // compatible with both legacy and current coding-plan credentials.
        .set("Authorization", api_key)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .set("Accept-Language", "en-US,en")
        .set("User-Agent", "ai-usage-bar")
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(map_ureq_error)?;

    response
        .into_json()
        .map_err(|_| ZaiAdapterError::SchemaDrift("usage body is not JSON".into()))
}

fn map_ureq_error(error: ureq::Error) -> ZaiAdapterError {
    match error {
        ureq::Error::Status(401 | 403, _) => ZaiAdapterError::AuthExpired,
        ureq::Error::Status(429, _) => ZaiAdapterError::RateLimited,
        ureq::Error::Status(408 | 504, _) => ZaiAdapterError::Timeout,
        ureq::Error::Status(_, _) => ZaiAdapterError::Network,
        ureq::Error::Transport(transport) => {
            let message = transport.to_string().to_ascii_lowercase();
            if message.contains("timed out") || message.contains("timeout") {
                ZaiAdapterError::Timeout
            } else {
                ZaiAdapterError::Network
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn fixture_time() -> DateTime<Utc> {
        Utc.timestamp_opt(1_786_000_000, 0).unwrap()
    }

    fn load_fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures/zai")
            .join(name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"));
        serde_json::from_str(&content).unwrap()
    }

    #[test]
    fn adapter_reports_zai_without_fetching() {
        assert_eq!(ZaiAdapter.provider(), Provider::Zai);
    }

    #[test]
    fn parse_credit_fixture_emits_five_hour_and_weekly_percentages() {
        let snapshots =
            parse_usage_response(&load_fixture("normal.json"), fixture_time(), "zai-test").unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].window_kind, WindowKind::Rolling);
        assert_eq!(snapshots[0].window_label.as_deref(), Some(FIVE_HOUR_LABEL));
        assert_eq!(snapshots[0].used, Some(25.0));
        assert_eq!(snapshots[0].remaining, Some(75.0));
        assert_eq!(
            snapshots[0].resets_at,
            Some(fixture_time() + chrono::Duration::hours(2))
        );
        assert_eq!(snapshots[1].window_kind, WindowKind::Weekly);
        assert_eq!(snapshots[1].window_label.as_deref(), Some(WEEKLY_LABEL));
        assert_eq!(snapshots[1].used, Some(20.0));
        assert!(snapshots.iter().all(|snapshot| snapshot.validate().is_ok()));
    }

    #[test]
    fn parse_legacy_token_fixture_and_optional_mcp_requests() {
        let snapshots = parse_usage_response(
            &load_fixture("legacy_tokens.json"),
            fixture_time(),
            "zai-test",
        )
        .unwrap();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].used, Some(12.5));
        assert_eq!(snapshots[1].window_kind, WindowKind::Weekly);
        assert_eq!(snapshots[2].metric_kind, MetricKind::Requests);
        assert_eq!(snapshots[2].unit, "requests");
        assert_eq!(snapshots[2].used, Some(4.0));
        assert_eq!(snapshots[2].remaining, Some(96.0));
        assert_eq!(snapshots[2].window_kind, WindowKind::Monthly);
    }

    #[test]
    fn percentage_can_be_derived_from_current_and_total() {
        let raw = serde_json::json!({
            "data": {"limits": [{
                "type": "CREDIT_LIMIT", "unit": 3, "number": 5,
                "usage": 2000, "currentValue": 500
            }]}
        });
        let snapshots = parse_usage_response(&raw, fixture_time(), "zai-test").unwrap();
        assert_eq!(snapshots[0].used, Some(25.0));
    }

    #[test]
    fn impossible_derived_totals_are_schema_drift() {
        let raw = serde_json::json!({
            "data": {"limits": [{
                "type": "CREDIT_LIMIT", "unit": 3, "number": 5,
                "usage": 0, "currentValue": 1
            }]}
        });
        assert!(matches!(
            parse_usage_response(&raw, fixture_time(), "zai-test"),
            Err(ZaiAdapterError::SchemaDrift(_))
        ));
    }

    #[test]
    fn malformed_sibling_does_not_hide_a_valid_window() {
        let raw = serde_json::json!({
            "data": {"limits": [
                {"type": "CREDIT_LIMIT", "unit": 3, "number": 5, "percentage": 140},
                {"type": "CREDIT_LIMIT", "unit": 6, "number": 1, "percentage": 20}
            ]}
        });
        let snapshots = parse_usage_response(&raw, fixture_time(), "zai-test").unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].window_label.as_deref(), Some(WEEKLY_LABEL));
    }

    #[test]
    fn unknown_limit_type_does_not_become_fake_usage() {
        let raw = serde_json::json!({
            "code": 200,
            "success": true,
            "data": {"limits": [{"type": "NEW_LIMIT", "percentage": 40}]}
        });
        assert!(matches!(
            parse_usage_response(&raw, fixture_time(), "zai-test"),
            Err(ZaiAdapterError::SchemaDrift(_))
        ));
    }

    #[test]
    fn auth_and_rate_limit_codes_are_classified_without_leaking_messages() {
        let auth = serde_json::json!({"code": 401, "success": false, "msg": "secret"});
        assert!(matches!(
            parse_usage_response(&auth, fixture_time(), "zai-test"),
            Err(ZaiAdapterError::AuthExpired)
        ));
        let limited = serde_json::json!({"code": 1310, "success": false});
        assert!(matches!(
            parse_usage_response(&limited, fixture_time(), "zai-test"),
            Err(ZaiAdapterError::RateLimited)
        ));
    }

    #[test]
    fn redacted_state_fixtures_map_to_typed_errors() {
        assert!(matches!(
            parse_usage_response(
                &load_fixture("auth_failure.json"),
                fixture_time(),
                "zai-test"
            ),
            Err(ZaiAdapterError::AuthExpired)
        ));
        assert!(matches!(
            parse_usage_response(&load_fixture("timeout.json"), fixture_time(), "zai-test"),
            Err(ZaiAdapterError::Timeout)
        ));
        assert!(matches!(
            parse_usage_response(
                &load_fixture("unlimited_or_missing.json"),
                fixture_time(),
                "zai-test"
            ),
            Err(ZaiAdapterError::SchemaDrift(_))
        ));
    }

    #[test]
    fn api_key_account_id_is_stable_and_redacted() {
        let id = account_id_from_api_key("secret.key");
        assert!(id.starts_with("zai-api-"));
        assert!(!id.contains("secret"));
        assert_eq!(id, account_id_from_api_key("secret.key"));
    }
}
