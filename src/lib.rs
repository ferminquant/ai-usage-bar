pub mod browser;
pub mod codex;
pub mod config;
pub mod daemon;
pub mod grok;
pub mod kimi;
pub mod model;
pub mod ollama;
pub mod opencode;
pub mod security;
pub mod startup;
pub mod viewmodel;
pub mod zai;

pub use browser::{is_allowed_browser_url, KIMI_CONSOLE_URL, OLLAMA_USAGE_URL};

pub use codex::{
    account_id_from_email, error_snapshot, fetch_codex_snapshots, parse_account_response,
    parse_rate_limits_response, CodexAdapter, CodexAdapterError,
};
pub use config::{
    build_registry, default_config_path, load_registry, AppConfig, ConfigError, ProviderSettings,
    ProviderViewSettings, ResolvedView, ViewSettings, CONFIG_DIR_NAME, CONFIG_FILE_NAME,
    CONFIG_VERSION, VIEW_VERSION,
};
pub use daemon::{
    Clock, ProviderRegistry, RefreshDiagnostic, RefreshPolicy, RefreshReport, RefreshService,
    RegistryError, SharedAdapter, SnapshotCache, StoreLiveReject, StoreLiveResult, SystemClock,
};
pub use grok::{
    account_id_from_identity as grok_account_id_from_identity,
    error_snapshot as grok_error_snapshot, fetch_grok_consumer_snapshots, parse_billing_response,
    GrokAdapterError, GrokConsumerAdapter,
};
pub use kimi::{
    account_id_from_credential_path, error_snapshot as kimi_error_snapshot, fetch_kimi_snapshots,
    parse_usages_response, session_available, KimiAdapter, KimiAdapterError,
};
pub use model::{
    AdapterError, Confidence, ErrorCode, Freshness, MetricKind, Provider, ProviderAdapter,
    SnapshotValidationError, Source, UsageSnapshot, WindowKind,
};
pub use ollama::{
    error_snapshot as ollama_error_snapshot, fetch_ollama_cloud_snapshots, parse_usage_response,
    OllamaAdapterError, OllamaCloudAdapter,
};
pub use opencode::{opencode_data_available, OpenCodeGoAdapter, OpenCodeResetSettings};
pub use security::{redact_sensitive_text, safe_identifier};
pub use viewmodel::{
    build_tray_view, build_tray_view_focused, build_tray_view_focused_window,
    filter_snapshots_for_view, forced_window_for_provider, format_reset_label,
    provider_display_name, providers_for_snapshots, switchable_providers_for_snapshots,
    window_display_name, window_is_selectable, MetricCard, ProviderCard, TrayViewModel,
};
pub use zai::{
    account_id_from_api_key as zai_account_id_from_api_key, fetch_zai_snapshots,
    parse_usage_response as parse_zai_usage_response, zai_api_key_available, ZaiAdapter,
    ZaiAdapterError,
};
