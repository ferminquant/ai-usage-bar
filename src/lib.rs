pub mod codex;
pub mod config;
pub mod daemon;
pub mod grok;
pub mod ollama;
pub mod model;
pub mod viewmodel;

pub use codex::{
    account_id_from_email, error_snapshot, fetch_codex_snapshots, parse_account_response,
    parse_rate_limits_response, CodexAdapter, CodexAdapterError,
};
pub use config::{
    build_registry, default_config_path, load_registry, AppConfig, ConfigError, ProviderSettings,
    CONFIG_DIR_NAME, CONFIG_FILE_NAME, CONFIG_VERSION,
};
pub use grok::{
    account_id_from_identity as grok_account_id_from_identity, error_snapshot as grok_error_snapshot,
    fetch_grok_consumer_snapshots, parse_billing_response, GrokAdapterError, GrokConsumerAdapter,
};
pub use ollama::{
    error_snapshot as ollama_error_snapshot, fetch_ollama_cloud_snapshots, parse_settings_resets,
    parse_usage_response, OllamaAdapterError, OllamaCloudAdapter, ResetTimes,
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
    build_tray_view, build_tray_view_focused, build_tray_view_focused_window, format_reset_label,
    provider_display_name, MetricCard, ProviderCard, TrayViewModel,
};
