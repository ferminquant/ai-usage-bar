pub mod codex;
pub mod daemon;
pub mod model;
pub mod viewmodel;

pub use codex::{
    account_id_from_email, error_snapshot, fetch_codex_snapshots, parse_account_response,
    parse_rate_limits_response, CodexAdapter, CodexAdapterError,
};
pub use daemon::{
    Clock, ProviderRegistry, RefreshDiagnostic, RefreshPolicy, RefreshReport, RefreshService,
    RegistryError, SharedAdapter, SnapshotCache, SystemClock,
};
pub use model::{
    AdapterError, Confidence, ErrorCode, Freshness, MetricKind, Provider, ProviderAdapter,
    SnapshotValidationError, Source, UsageSnapshot, WindowKind,
};
pub use viewmodel::{build_tray_view, MetricCard, ProviderCard, TrayViewModel};
