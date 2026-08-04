pub mod codex;
pub mod daemon;
pub mod grok;
pub mod model;
pub mod viewmodel;

pub use codex::{
    account_id_from_email, error_snapshot, fetch_codex_snapshots, parse_account_response,
    parse_rate_limits_response, CodexAdapter, CodexAdapterError,
};
pub use grok::{
    account_id_from_identity as grok_account_id_from_identity, error_snapshot as grok_error_snapshot,
    fetch_grok_consumer_snapshots, parse_billing_response, GrokAdapterError, GrokConsumerAdapter,
};
pub use daemon::{
    Clock, ProviderRegistry, RefreshDiagnostic, RefreshPolicy, RefreshReport, RefreshService,
    RegistryError, SharedAdapter, SnapshotCache, StoreLiveReject, StoreLiveResult, SystemClock,
};
pub use model::{
    AdapterError, Confidence, ErrorCode, Freshness, MetricKind, Provider, ProviderAdapter,
    SnapshotValidationError, Source, UsageSnapshot, WindowKind,
};
pub use viewmodel::{
    build_tray_view, build_tray_view_focused, format_reset_label, provider_display_name, MetricCard,
    ProviderCard, TrayViewModel,
};
