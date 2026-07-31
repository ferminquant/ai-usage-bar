pub mod codex;
pub mod model;

pub use codex::{account_id_from_email, parse_account_response, parse_rate_limits_response, error_snapshot, CodexAdapterError};
pub use model::{AdapterError, Confidence, ErrorCode, Freshness, MetricKind, Provider, ProviderAdapter, Source, UsageSnapshot, WindowKind};