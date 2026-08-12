use serde::{Deserialize, Serialize};
use std::fmt;

const PROVIDER_CODEX: &str = "codex";
const PROVIDER_KIMI: &str = "kimi";
const PROVIDER_OLLAMA_CLOUD: &str = "ollama_cloud";
const PROVIDER_GROK_CONSUMER: &str = "grok_consumer";
const PROVIDER_GROK_API: &str = "grok_api";
const PROVIDER_OPENCODE_GO: &str = "opencode_go";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Kimi,
    OllamaCloud,
    GrokConsumer,
    GrokApi,
    OpenCodeGo,
}

impl Provider {
    /// Parse a persisted provider identifier. Unknown identifiers are rejected
    /// by callers rather than being converted into a placeholder provider.
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            PROVIDER_CODEX => Some(Self::Codex),
            PROVIDER_KIMI => Some(Self::Kimi),
            PROVIDER_OLLAMA_CLOUD => Some(Self::OllamaCloud),
            PROVIDER_GROK_CONSUMER => Some(Self::GrokConsumer),
            PROVIDER_GROK_API => Some(Self::GrokApi),
            PROVIDER_OPENCODE_GO => Some(Self::OpenCodeGo),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Codex => PROVIDER_CODEX,
            Self::Kimi => PROVIDER_KIMI,
            Self::OllamaCloud => PROVIDER_OLLAMA_CLOUD,
            Self::GrokConsumer => PROVIDER_GROK_CONSUMER,
            Self::GrokApi => PROVIDER_GROK_API,
            Self::OpenCodeGo => PROVIDER_OPENCODE_GO,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Provider {
    /// Canonical quota-window identifiers understood for this provider.
    ///
    /// These are the persisted, menu-selectable window vocabulary used by the
    /// `view` configuration section. Provider-native aliases are normalized to
    /// canonical keys: Ollama exposes `session`/`weekly`, Kimi
    /// `5-hour`/`weekly`/`total`, OpenCode `5-hour`/`weekly`/`monthly`, and the
    /// remaining providers expose their primary weekly quota as `primary`.
    ///
    /// Window identifiers outside this set are unknown for the provider and
    /// are ignored safely when resolving display preferences.
    pub fn canonical_window_keys(&self) -> &'static [&'static str] {
        match self {
            Self::Codex | Self::GrokConsumer | Self::GrokApi => &["primary"],
            Self::Kimi => &["5-hour", "weekly", "total"],
            Self::OllamaCloud => &["session", "weekly"],
            Self::OpenCodeGo => &["5-hour", "weekly", "monthly"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    Quota,
    Credits,
    Spend,
    Tokens,
    Requests,
    Health,
}

impl MetricKind {
    /// Parse the stable snake-case identifier used by persisted preferences.
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            "quota" => Some(Self::Quota),
            "credits" => Some(Self::Credits),
            "spend" => Some(Self::Spend),
            "tokens" => Some(Self::Tokens),
            "requests" => Some(Self::Requests),
            "health" => Some(Self::Health),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Credits => "credits",
            Self::Spend => "spend",
            Self::Tokens => "tokens",
            Self::Requests => "requests",
            Self::Health => "health",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Rolling,
    Daily,
    Weekly,
    Monthly,
    Session,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Api,
    Cli,
    LocalApi,
    Browser,
    Fixture,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Live,
    Cached,
    Stale,
    Unavailable,
    NotConfigured,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Exact,
    ReportedEstimate,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AuthExpired,
    Timeout,
    SchemaDrift,
    RateLimited,
    Network,
    Unknown,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AuthExpired => "auth_expired",
            Self::Timeout => "timeout",
            Self::SchemaDrift => "schema_drift",
            Self::RateLimited => "rate_limited",
            Self::Network => "network",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterError {
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotValidationError {
    #[error("contract: account_id must be a non-empty safe identifier")]
    InvalidAccountId,
    #[error("contract: unit must be non-empty")]
    EmptyUnit,
    #[error("contract: {0} must be finite")]
    NonFiniteValue(&'static str),
    #[error("contract: {0} must not be negative")]
    NegativeValue(&'static str),
    #[error("contract: {0} percentage must be between 0 and 100")]
    PercentageOutOfRange(&'static str),
    #[error("contract: used must not exceed limit when unlimited is false")]
    UsedExceedsLimit,
    #[error("contract: unavailable snapshots must not carry used/remaining/limit")]
    UnavailableHasValues,
    #[error("contract: error is only valid for unavailable snapshots")]
    ErrorStateMismatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub provider: Provider,
    pub account_id: String,
    pub metric_kind: MetricKind,
    pub window_kind: WindowKind,
    pub unit: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub source: Source,
    pub freshness: Freshness,
    pub confidence: Confidence,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AdapterError>,
}

impl UsageSnapshot {
    pub fn is_percentage(&self) -> bool {
        self.unit == "percent"
    }

    /// Validate the provider-neutral contract at the adapter/cache boundary.
    ///
    /// Provider responses are untrusted input. Keeping this check on the
    /// normalized model prevents an adapter from introducing impossible
    /// percentages, non-finite values, or contradictory freshness states.
    ///
    /// Call this on the adapter's raw snapshot *before* message redaction so
    /// contradictory freshness/error pairs are rejected rather than rewritten.
    pub fn validate(&self) -> Result<(), SnapshotValidationError> {
        if self.account_id.trim().is_empty() || self.account_id.chars().any(char::is_control) {
            return Err(SnapshotValidationError::InvalidAccountId);
        }
        if self.unit.trim().is_empty() {
            return Err(SnapshotValidationError::EmptyUnit);
        }
        for (name, value) in [
            ("used", self.used),
            ("remaining", self.remaining),
            ("limit", self.limit),
        ] {
            let Some(value) = value else {
                continue;
            };
            if !value.is_finite() {
                return Err(SnapshotValidationError::NonFiniteValue(name));
            }
            if value < 0.0 {
                return Err(SnapshotValidationError::NegativeValue(name));
            }
            if self.is_percentage() && value > 100.0 {
                return Err(SnapshotValidationError::PercentageOutOfRange(name));
            }
        }

        if !self.unlimited {
            if let (Some(used), Some(limit)) = (self.used, self.limit) {
                if used > limit {
                    return Err(SnapshotValidationError::UsedExceedsLimit);
                }
            }
        }

        if self.freshness == Freshness::Unavailable
            && (self.used.is_some() || self.remaining.is_some() || self.limit.is_some())
        {
            return Err(SnapshotValidationError::UnavailableHasValues);
        }

        match (self.freshness, self.error.is_some()) {
            (Freshness::Unavailable, false)
            | (Freshness::Live | Freshness::Cached | Freshness::Stale, true)
            | (Freshness::NotConfigured | Freshness::NotApplicable, true) => {
                Err(SnapshotValidationError::ErrorStateMismatch)
            }
            _ => Ok(()),
        }
    }

    /// Whether `account_id` is safe enough to use as part of a cache key.
    pub fn has_safe_account_id(&self) -> bool {
        !self.account_id.trim().is_empty() && !self.account_id.chars().any(char::is_control)
    }
}

pub trait ProviderAdapter {
    fn provider(&self) -> Provider;
    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_identifiers_cover_all_supported_view_values() {
        let providers = [
            ("codex", Provider::Codex, &["primary"][..]),
            ("kimi", Provider::Kimi, &["5-hour", "weekly", "total"][..]),
            (
                "ollama_cloud",
                Provider::OllamaCloud,
                &["session", "weekly"][..],
            ),
            ("grok_consumer", Provider::GrokConsumer, &["primary"][..]),
            ("grok_api", Provider::GrokApi, &["primary"][..]),
            (
                "opencode_go",
                Provider::OpenCodeGo,
                &["5-hour", "weekly", "monthly"][..],
            ),
        ];
        for (identifier, provider, windows) in providers {
            assert_eq!(
                Provider::from_identifier(identifier),
                Some(provider.clone())
            );
            assert_eq!(provider.as_str(), identifier);
            assert_eq!(provider.canonical_window_keys(), windows);
        }
        assert_eq!(Provider::from_identifier("unknown"), None);

        let metrics = [
            ("quota", MetricKind::Quota),
            ("credits", MetricKind::Credits),
            ("spend", MetricKind::Spend),
            ("tokens", MetricKind::Tokens),
            ("requests", MetricKind::Requests),
            ("health", MetricKind::Health),
        ];
        for (identifier, metric) in metrics {
            assert_eq!(MetricKind::from_identifier(identifier), Some(metric));
            assert_eq!(metric.as_str(), identifier);
        }
        assert_eq!(MetricKind::from_identifier("unknown"), None);
    }

    #[test]
    fn unavailable_snapshots_reject_each_value_field_individually() {
        let cases = [
            (Some(10.0), None, None),
            (None, Some(90.0), None),
            (None, None, Some(100.0)),
        ];

        for (used, remaining, limit) in cases {
            let snapshot = UsageSnapshot {
                provider: Provider::Codex,
                account_id: "test-account".into(),
                metric_kind: MetricKind::Quota,
                window_kind: WindowKind::Weekly,
                unit: "percent".into(),
                observed_at: chrono::Utc::now(),
                source: Source::Fixture,
                freshness: Freshness::Unavailable,
                confidence: Confidence::Unknown,
                used,
                remaining,
                limit,
                unlimited: false,
                resets_at: None,
                window_label: Some("primary".into()),
                error: Some(AdapterError {
                    code: ErrorCode::Timeout,
                    message: None,
                }),
            };

            assert_eq!(
                snapshot.validate(),
                Err(SnapshotValidationError::UnavailableHasValues)
            );
        }
    }
}
