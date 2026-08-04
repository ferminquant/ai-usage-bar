use serde::{Deserialize, Serialize};
use std::fmt;

const PROVIDER_CODEX: &str = "codex";
const PROVIDER_KIMI: &str = "kimi";
const PROVIDER_OLLAMA_LOCAL: &str = "ollama_local";
const PROVIDER_OLLAMA_CLOUD: &str = "ollama_cloud";
const PROVIDER_GROK_CONSUMER: &str = "grok_consumer";
const PROVIDER_GROK_API: &str = "grok_api";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Kimi,
    OllamaLocal,
    OllamaCloud,
    GrokConsumer,
    GrokApi,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Codex => PROVIDER_CODEX,
            Self::Kimi => PROVIDER_KIMI,
            Self::OllamaLocal => PROVIDER_OLLAMA_LOCAL,
            Self::OllamaCloud => PROVIDER_OLLAMA_CLOUD,
            Self::GrokConsumer => PROVIDER_GROK_CONSUMER,
            Self::GrokApi => PROVIDER_GROK_API,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
}

pub trait ProviderAdapter {
    fn provider(&self) -> Provider;
    fn fetch(&self) -> Result<Vec<UsageSnapshot>, AdapterError>;
}