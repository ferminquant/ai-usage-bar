//! User configuration and the shared built-in provider bootstrap.
//!
//! Configuration controls which compiled hosted providers are scheduled. It
//! deliberately contains no credentials: each hosted adapter discovers its
//! own local login/session surface.

use crate::zai::zai_api_key_available;
use crate::{
    opencode_data_available, session_available, CodexAdapter, GrokConsumerAdapter, KimiAdapter,
    MetricKind, OllamaCloudAdapter, OpenCodeGoAdapter, OpenCodeResetSettings, Provider,
    ProviderRegistry, RegistryError, ZaiAdapter,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: u32 = 1;
pub const VIEW_VERSION: u32 = 1;
pub const CONFIG_FILE_NAME: &str = "config.json";
pub const CONFIG_DIR_NAME: &str = "AI Usage Bar";

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

fn default_view_version() -> u32 {
    VIEW_VERSION
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_provider_enabled() -> bool {
    true
}

/// Per-provider settings persisted in the user configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Whether this provider should be scheduled for refresh. The shell keeps
    /// this in sync with the provider-level hide control in [`AppConfig::view`].
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,
    /// Optional next weekly reset anchor for OpenCode Go, in UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_reset_at: Option<DateTime<Utc>>,
    /// Optional next monthly reset anchor for OpenCode Go, in UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_reset_at: Option<DateTime<Utc>>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            weekly_reset_at: None,
            monthly_reset_at: None,
        }
    }
}

/// Per-provider display-only preferences within [`ViewSettings`].
///
/// `None` means "use the provider's display defaults"; an explicit empty list
/// is a deliberate choice to hide every optional row of that kind. Core quota
/// and health rows remain visible regardless of this preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderViewSettings {
    /// Quota windows to show for this provider, as canonical window keys
    /// (e.g. `5-hour`, `weekly`, `total`, `monthly`, `session`, `primary`).
    /// Absent = all reported windows; an explicit empty list hides every
    /// window row for the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_windows: Option<Vec<String>>,
    /// Metric kinds to show for this provider (e.g. `quota`, `credits`,
    /// `spend`, `tokens`, `requests`, `health`). Absent = all reported kinds;
    /// an explicit empty list hides every metric row for the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_kinds: Option<Vec<String>>,
}

/// Versioned view and provider-control preferences persisted in
/// [`AppConfig::view`]. Provider-level visibility is paired with enablement by
/// the shell so a hidden or disabled provider is not refreshed in the
/// background.
///
/// Unknown provider/window/metric identifiers are ignored during resolution,
/// and an unsupported `version` falls back to defaults instead of failing the
/// whole configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSettings {
    #[serde(default = "default_view_version")]
    pub version: u32,
    /// Provider display order for the expanded view. Known providers keep
    /// their listed relative order and come first; providers not listed keep
    /// the caller's existing order after them. Unknown providers are dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_order: Vec<String>,
    /// Providers hidden from the expanded view. The shell also disables these
    /// providers so hiding a provider stops its refresh work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_providers: Vec<String>,
    /// Whether disabled providers should be shown in the expanded view. The
    /// default keeps the panel focused on providers that are enabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub show_disabled_providers: bool,
    /// Per-provider window/metric visibility, keyed by provider identifier.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderViewSettings>,
    /// Default focused provider for the expanded view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    /// Default quota window for the expanded view, valid only together with a
    /// resolvable `default_provider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_window: Option<String>,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            version: VIEW_VERSION,
            provider_order: Vec::new(),
            hidden_providers: Vec::new(),
            show_disabled_providers: false,
            providers: BTreeMap::new(),
            default_provider: None,
            default_window: None,
        }
    }
}

/// Display and provider-control preferences resolved against the compiled
/// provider model.
///
/// Resolution is deterministic: unknown identifiers are dropped, duplicates
/// are collapsed, and contradictions fall back (a hidden or unknown default
/// provider becomes auto; a window without a valid default provider or
/// excluded by that provider's own visibility filter is dropped). Absent
/// preferences resolve to `None`/empty values so callers keep their existing
/// behavior exactly when no `view` section is configured.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedView {
    /// Known providers listed by the user, in their configured order. The
    /// caller appends any available providers not listed here.
    pub provider_order: Option<Vec<Provider>>,
    /// Known providers hidden from display. The shell treats these as
    /// disabled for scheduling as well.
    pub hidden_providers: Vec<Provider>,
    /// Whether disabled providers should be included in the expanded view.
    pub show_disabled_providers: bool,
    /// Per-provider visible quota-window filters. `None` means all windows;
    /// an empty list is an explicit hide-all choice.
    pub visible_windows: BTreeMap<Provider, Vec<String>>,
    /// Per-provider visible metric-kind filters. `None` means the provider's
    /// defaults; an empty list is an explicit hide-all choice for optional
    /// metrics.
    pub visible_metrics: BTreeMap<Provider, Vec<MetricKind>>,
    /// The configured default provider when it is known and not hidden.
    pub default_provider: Option<Provider>,
    /// The configured default window when it is valid for the default
    /// provider and is not excluded by that provider's window filter.
    pub default_window: Option<String>,
}

impl ResolvedView {
    fn resolve(view: &ViewSettings, available: &[Provider]) -> Self {
        let known = |provider: &Provider| available.iter().any(|candidate| candidate == provider);

        let provider_order = if view.provider_order.is_empty() {
            None
        } else {
            let mut order = Vec::new();
            for raw in &view.provider_order {
                let Some(provider) = Provider::from_identifier(raw) else {
                    continue;
                };
                if known(&provider) && !order.contains(&provider) {
                    order.push(provider);
                }
            }
            (!order.is_empty()).then_some(order)
        };

        let mut hidden_providers = Vec::new();
        for raw in &view.hidden_providers {
            let Some(provider) = Provider::from_identifier(raw) else {
                continue;
            };
            if known(&provider) && !hidden_providers.contains(&provider) {
                hidden_providers.push(provider);
            }
        }

        let mut visible_windows = BTreeMap::new();
        let mut visible_metrics = BTreeMap::new();
        for (raw_provider, settings) in &view.providers {
            let Some(provider) = Provider::from_identifier(raw_provider) else {
                continue;
            };
            if !known(&provider) {
                continue;
            }

            if let Some(raw_windows) = &settings.visible_windows {
                let windows = if raw_windows.is_empty() {
                    Some(Vec::new())
                } else {
                    let recognized = raw_windows
                        .iter()
                        .filter(|window| {
                            provider.canonical_window_keys().contains(&window.as_str())
                        })
                        .cloned()
                        .fold(Vec::new(), |mut values, window| {
                            if !values.contains(&window) {
                                values.push(window);
                            }
                            values
                        });
                    // A non-empty list containing only unknown identifiers is
                    // treated as an invalid preference and falls back to all
                    // windows instead of hiding the provider by accident.
                    (!recognized.is_empty()).then_some(recognized)
                };
                if let Some(windows) = windows {
                    visible_windows.insert(provider.clone(), windows);
                }
            }

            if let Some(raw_metrics) = &settings.metric_kinds {
                let metrics = if raw_metrics.is_empty() {
                    Some(Vec::new())
                } else {
                    let recognized = raw_metrics
                        .iter()
                        .filter_map(|metric| MetricKind::from_identifier(metric))
                        .fold(Vec::new(), |mut values, metric| {
                            if !values.contains(&metric) {
                                values.push(metric);
                            }
                            values
                        });
                    (!recognized.is_empty()).then_some(recognized)
                };
                if let Some(metrics) = metrics {
                    visible_metrics.insert(provider, metrics);
                }
            }
        }

        let default_provider = view
            .default_provider
            .as_deref()
            .and_then(Provider::from_identifier)
            .filter(|provider| known(provider) && !hidden_providers.contains(provider));
        let default_window = match (default_provider.as_ref(), view.default_window.as_deref()) {
            (Some(provider), Some(window))
                if provider.canonical_window_keys().contains(&window)
                    && visible_windows
                        .get(provider)
                        .map(|windows| windows.iter().any(|candidate| candidate == window))
                        .unwrap_or(true) =>
            {
                Some(window.to_string())
            }
            _ => None,
        };

        Self {
            provider_order,
            hidden_providers,
            show_disabled_providers: view.show_disabled_providers,
            visible_windows,
            visible_metrics,
            default_provider,
            default_window,
        }
    }

    pub fn is_provider_hidden(&self, provider: &Provider) -> bool {
        self.hidden_providers.contains(provider)
    }

    pub fn windows_for(&self, provider: &Provider) -> Option<&[String]> {
        self.visible_windows.get(provider).map(Vec::as_slice)
    }

    pub fn metrics_for(&self, provider: &Provider) -> Option<&[MetricKind]> {
        self.visible_metrics.get(provider).map(Vec::as_slice)
    }

    /// Whether a metric row should be displayed for a provider.
    ///
    /// Quota and health rows are core diagnostics and are never hidden by a
    /// display preference. Optional metrics remain visible by default, except
    /// for Codex credits, which are opt-in because most accounts do not use
    /// that balance. An explicit provider preference overrides that default.
    pub fn is_metric_visible(&self, provider: &Provider, metric: MetricKind) -> bool {
        if matches!(metric, MetricKind::Quota | MetricKind::Health) {
            return true;
        }
        self.metrics_for(provider)
            .map(|metrics| metrics.contains(&metric))
            .unwrap_or_else(|| {
                !(matches!(provider, Provider::Codex) && metric == MetricKind::Credits)
            })
    }

    /// Whether an optional metric is visible when no explicit preference has
    /// been saved for the provider.
    pub fn default_metric_visible(provider: &Provider, metric: MetricKind) -> bool {
        !(matches!(provider, Provider::Codex) && metric == MetricKind::Credits)
    }
}

/// Versioned, non-secret application configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderSettings>,
    /// View and provider-control preferences. Absent keeps the current
    /// display behavior; never stored when unset so existing config files stay
    /// compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewSettings>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            providers: BTreeMap::new(),
            view: None,
        }
    }
}

impl AppConfig {
    /// Parse and validate a JSON configuration document.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_json::from_str(raw).map_err(ConfigError::Parse)?;
        config.validate_version()?;
        Ok(config)
    }

    /// Load configuration from a path. A missing file means defaults.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(raw) => Self::parse(&raw),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Load the per-user configuration from the platform default path.
    pub fn load_default() -> Result<Self, ConfigError> {
        Self::load(default_config_path())
    }

    /// Persist this configuration, creating its parent directory if needed.
    ///
    /// The write is atomic: content is staged in a same-directory temp file
    /// and renamed over the target, so a crash or concurrent reader never
    /// observes a truncated or partially written configuration.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        self.validate_version()?;
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        }
        let document = serde_json::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        write_atomically(path, format!("{document}\n").as_bytes())
    }

    /// Whether a provider should be enabled by the registry bootstrap.
    pub fn provider_enabled(&self, provider: &Provider) -> bool {
        self.providers
            .get(provider.as_str())
            .map(|settings| settings.enabled)
            .unwrap_or_else(|| default_enabled_provider(provider))
    }

    /// Update one provider's persisted enablement setting.
    pub fn set_provider_enabled(&mut self, provider: &Provider, enabled: bool) {
        self.providers
            .entry(provider.as_str().to_string())
            .or_default()
            .enabled = enabled;
    }

    /// Return the user-configured OpenCode Go reset anchors.
    pub fn opencode_reset_settings(&self) -> OpenCodeResetSettings {
        self.providers
            .get(Provider::OpenCodeGo.as_str())
            .map(|settings| OpenCodeResetSettings {
                weekly_reset_at: settings.weekly_reset_at,
                monthly_reset_at: settings.monthly_reset_at,
            })
            .unwrap_or_default()
    }

    /// Persist OpenCode Go reset anchors without changing provider enablement.
    pub fn set_opencode_reset_settings(&mut self, settings: OpenCodeResetSettings) {
        let provider = self
            .providers
            .entry(Provider::OpenCodeGo.as_str().to_string())
            .or_default();
        provider.weekly_reset_at = settings.weekly_reset_at;
        provider.monthly_reset_at = settings.monthly_reset_at;
    }

    /// The raw persisted display preferences, if a `view` section is present.
    pub fn view_settings(&self) -> Option<&ViewSettings> {
        self.view.as_ref()
    }

    /// Display preferences resolved against the compiled provider model.
    ///
    /// Unknown identifiers are ignored, duplicates collapsed, and
    /// contradictions resolved deterministically. Returns the default
    /// (all-empty) resolution when there is no `view` section or its version
    /// is unsupported, preserving current display behavior. The caller
    /// supplies the providers currently available in the compiled registry so
    /// unknown identifiers can be ignored safely.
    pub fn resolved_view(&self, available: &[Provider]) -> ResolvedView {
        let Some(view) = self
            .view
            .as_ref()
            .filter(|view| view.version == VIEW_VERSION)
        else {
            return ResolvedView::default();
        };
        ResolvedView::resolve(view, available)
    }

    /// Replace the persisted provider display order for the expanded view.
    ///
    /// Providers not listed keep the caller's existing order after the listed
    /// ones. Pass an empty slice to clear the preference.
    pub fn set_view_provider_order(&mut self, order: &[Provider]) {
        self.ensure_view().provider_order = order
            .iter()
            .map(|provider| provider.as_str().to_string())
            .collect();
    }

    /// Replace the persisted set of providers hidden from display.
    ///
    /// The shell pairs this with provider enablement when the user clicks a
    /// provider control. Pass an empty slice to show every provider again.
    pub fn set_view_hidden_providers(&mut self, hidden: &[Provider]) {
        self.ensure_view().hidden_providers = hidden
            .iter()
            .map(|provider| provider.as_str().to_string())
            .collect();
    }

    /// Persist whether disabled providers should be shown in the expanded
    /// provider panel.
    pub fn set_view_show_disabled_providers(&mut self, show: bool) {
        self.ensure_view().show_disabled_providers = show;
    }

    /// Set the quota windows shown for one provider.
    ///
    /// `None` clears the preference (all reported windows shown); `Some([])`
    /// explicitly hides every window row; `Some(windows)` restricts to the
    /// given canonical window keys.
    pub fn set_view_visible_windows(&mut self, provider: &Provider, windows: Option<&[&str]>) {
        self.mutate_provider_view(provider, |entry| {
            entry.visible_windows =
                windows.map(|list| list.iter().map(|w| (*w).to_string()).collect());
        });
    }

    /// Set the metric kinds shown for one provider.
    ///
    /// `None` clears the preference (provider defaults); `Some([])` explicitly
    /// hides every optional metric row; `Some(kinds)` restricts optional rows
    /// to the given metric kinds. Core quota and health rows remain visible.
    pub fn set_view_visible_metrics(
        &mut self,
        provider: &Provider,
        metrics: Option<&[MetricKind]>,
    ) {
        self.mutate_provider_view(provider, |entry| {
            entry.metric_kinds =
                metrics.map(|list| list.iter().map(|kind| kind.as_str().to_string()).collect());
        });
    }

    /// Set the default provider and default quota window for the expanded
    /// view. `None` clears each preference independently.
    pub fn set_view_defaults(
        &mut self,
        default_provider: Option<&Provider>,
        default_window: Option<&str>,
    ) {
        let view = self.ensure_view();
        view.default_provider = default_provider.map(|provider| provider.as_str().to_string());
        view.default_window = default_window.map(str::to_string);
    }

    /// Remove the persisted `view` section entirely, restoring current
    /// (unconfigured) display behavior.
    pub fn reset_view(&mut self) {
        self.view = None;
    }

    fn ensure_view(&mut self) -> &mut ViewSettings {
        // A `view` section from a future version cannot be meaningfully
        // edited, so the first mutation upgrades it to the current version
        // rather than writing fields we do not understand.
        if !matches!(self.view.as_ref(), Some(view) if view.version == VIEW_VERSION) {
            self.view = Some(ViewSettings::default());
        }
        self.view.as_mut().expect("view initialized above")
    }

    fn mutate_provider_view(
        &mut self,
        provider: &Provider,
        mutate: impl FnOnce(&mut ProviderViewSettings),
    ) {
        let view = self.ensure_view();
        let key = provider.as_str().to_string();
        let entry = view.providers.entry(key).or_default();
        mutate(entry);
        if entry.visible_windows.is_none() && entry.metric_kinds.is_none() {
            view.providers.remove(provider.as_str());
        }
    }

    fn validate_version(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        Ok(())
    }
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), ConfigError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");

    for attempt in 0..32u32 {
        let temporary = parent.join(format!(".{file_name}.tmp-{}-{attempt}", std::process::id()));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(ConfigError::Write {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };

        let result = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            drop(file);
            replace_atomically(&temporary, path)
        })();

        if result.is_ok() {
            return Ok(());
        }

        let _ = fs::remove_file(&temporary);
        let source = result.expect_err("result checked above");
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source,
        });
    }

    Err(ConfigError::Write {
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "unable to allocate temporary config path",
        ),
    })
}

#[cfg(not(windows))]
fn replace_atomically(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_atomically(temporary: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::ReplaceFileW;

    if !target.exists() {
        return fs::rename(temporary, target);
    }

    let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let temporary_wide: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        ReplaceFileW(
            PCWSTR(target_wide.as_ptr()),
            PCWSTR(temporary_wide.as_ptr()),
            PCWSTR::null(),
            windows::Win32::Storage::FileSystem::REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
        .map_err(|error| io::Error::from_raw_os_error(error.code().0))
    }
}

fn default_enabled_provider(provider: &Provider) -> bool {
    matches!(
        provider,
        Provider::Codex | Provider::GrokConsumer | Provider::OpenCodeGo
    )
}

/// Return the platform-specific per-user configuration path.
pub fn default_config_path() -> PathBuf {
    #[cfg(windows)]
    if let Some(appdata) = env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join(CONFIG_DIR_NAME)
            .join(CONFIG_FILE_NAME);
    }

    #[cfg(not(windows))]
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(config_home)
            .join("ai-usage-bar")
            .join(CONFIG_FILE_NAME);
    }

    #[cfg(not(windows))]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("ai-usage-bar")
            .join(CONFIG_FILE_NAME);
    }

    #[cfg(windows)]
    if let Some(profile) = env::var_os("USERPROFILE") {
        return PathBuf::from(profile)
            .join(CONFIG_DIR_NAME)
            .join(CONFIG_FILE_NAME);
    }

    PathBuf::from(CONFIG_FILE_NAME)
}

/// Build the registry for the currently compiled, hosted adapter set.
///
/// The shell and CLI call this function instead of registering providers
/// themselves. Future hosted adapters add one factory entry here; no shell UI
/// registration changes are required.
pub fn build_registry(config: &AppConfig) -> Result<ProviderRegistry, RegistryError> {
    let registry = ProviderRegistry::new();
    registry.register(CodexAdapter)?;
    registry.register(GrokConsumerAdapter)?;
    registry.register(OllamaCloudAdapter)?;
    if opencode_data_available() {
        registry.register(OpenCodeGoAdapter::new(config.opencode_reset_settings()))?;
    } else {
        registry.register_not_configured(Provider::OpenCodeGo)?;
    }
    // Kimi reuses the CLI OAuth session: with a session it is a live adapter;
    // without one it registers as not configured (never zero usage).
    if session_available() {
        registry.register(KimiAdapter)?;
    } else {
        registry.register_not_configured(Provider::Kimi)?;
    }
    // z.ai uses an API key supplied by the provider-owned CLI/tool setup. Do
    // not call the monitor endpoint when no key is available; the registry
    // still exposes a not-configured card so the user can enable it later.
    if zai_api_key_available() {
        registry.register(ZaiAdapter)?;
    } else {
        registry.register_not_configured(Provider::Zai)?;
    }

    let providers = [
        Provider::Codex,
        Provider::GrokConsumer,
        Provider::OllamaCloud,
        Provider::Kimi,
        Provider::OpenCodeGo,
        Provider::Zai,
    ];
    // A persisted hidden provider may come from an older build where hiding
    // was display-only. Treat it as disabled at bootstrap so the new control
    // never leaves a hidden adapter refreshing in the background.
    let resolved_view = config.resolved_view(&providers);
    for provider in &providers {
        let enabled =
            config.provider_enabled(provider) && !resolved_view.is_provider_hidden(provider);
        registry.set_enabled(provider, enabled)?;
    }

    Ok(registry)
}

/// Load user configuration and build the shared hosted-provider registry.
pub fn load_registry() -> Result<ProviderRegistry, ConfigError> {
    let config = AppConfig::load_default()?;
    build_registry(&config).map_err(ConfigError::Registry)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("unable to read provider config {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("provider config is invalid JSON: {0}")]
    Parse(serde_json::Error),
    #[error("unsupported provider config version {0}; expected {CONFIG_VERSION}")]
    UnsupportedVersion(u32),
    #[error("unable to serialize provider config: {0}")]
    Serialize(serde_json::Error),
    #[error("provider registry setup failed: {0}")]
    Registry(RegistryError),
    #[error("unable to write provider config {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_only_verified_hosted_providers() {
        let config = AppConfig::default();

        assert!(config.provider_enabled(&Provider::Codex));
        assert!(config.provider_enabled(&Provider::GrokConsumer));
        assert!(config.provider_enabled(&Provider::OpenCodeGo));
        assert!(!config.provider_enabled(&Provider::Kimi));
        assert!(!config.provider_enabled(&Provider::OllamaCloud));
        assert!(!config.provider_enabled(&Provider::Zai));
    }

    #[test]
    fn parses_provider_overrides() {
        let config = AppConfig::parse(
            r#"{
                "version": 1,
                "providers": {
                    "codex": {"enabled": false},
                    "grok_consumer": {"enabled": true},
                    "ollama_cloud": {"enabled": true}
                }
            }"#,
        )
        .unwrap();

        assert!(!config.provider_enabled(&Provider::Codex));
        assert!(config.provider_enabled(&Provider::GrokConsumer));
        assert!(config.provider_enabled(&Provider::OllamaCloud));
    }

    #[test]
    fn legacy_config_without_view_keeps_unconfigured_display_defaults() {
        let config =
            AppConfig::parse(r#"{"version":1,"providers":{"codex":{"enabled":false}}}"#).unwrap();

        assert!(config.view_settings().is_none());
        let resolved = config.resolved_view(&[Provider::Codex, Provider::GrokConsumer]);
        assert_eq!(resolved, ResolvedView::default());
    }

    #[test]
    fn view_preferences_round_trip_and_resolve_against_available_providers() {
        let path = std::env::temp_dir().join(format!(
            "ai-usage-bar-view-roundtrip-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        let mut config = AppConfig::default();
        config.set_view_provider_order(&[Provider::Kimi, Provider::Codex]);
        config.set_view_hidden_providers(&[Provider::GrokConsumer]);
        config.set_view_show_disabled_providers(true);
        config.set_view_visible_windows(&Provider::Kimi, Some(&["5-hour", "weekly"]));
        config.set_view_visible_metrics(
            &Provider::Codex,
            Some(&[MetricKind::Quota, MetricKind::Credits]),
        );
        config.set_view_defaults(Some(&Provider::Kimi), Some("5-hour"));
        config.save(&path).unwrap();

        let loaded = AppConfig::load(&path).unwrap();
        let available = [Provider::Codex, Provider::Kimi, Provider::GrokConsumer];
        let resolved = loaded.resolved_view(&available);
        assert_eq!(
            resolved.provider_order,
            Some(vec![Provider::Kimi, Provider::Codex])
        );
        assert_eq!(resolved.hidden_providers, vec![Provider::GrokConsumer]);
        assert!(resolved.show_disabled_providers);
        assert_eq!(
            resolved.windows_for(&Provider::Kimi),
            Some(["5-hour".to_string(), "weekly".to_string()].as_slice())
        );
        assert_eq!(
            resolved.metrics_for(&Provider::Codex),
            Some([MetricKind::Quota, MetricKind::Credits].as_slice())
        );
        assert_eq!(resolved.default_provider, Some(Provider::Kimi));
        assert_eq!(resolved.default_window.as_deref(), Some("5-hour"));

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"view\""));
        assert!(!raw.contains("token"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn view_visibility_preferences_can_be_cleared_back_to_all_rows() {
        let available = [Provider::OllamaCloud, Provider::Kimi];
        let mut config = AppConfig::default();

        config.set_view_hidden_providers(&[Provider::Kimi]);
        config.set_view_visible_windows(&Provider::OllamaCloud, Some(&["weekly"]));
        config.set_view_visible_metrics(&Provider::OllamaCloud, Some(&[MetricKind::Quota]));
        let filtered = config.resolved_view(&available);
        assert_eq!(filtered.hidden_providers, vec![Provider::Kimi]);
        assert_eq!(
            filtered.windows_for(&Provider::OllamaCloud),
            Some(["weekly".to_string()].as_slice())
        );
        assert_eq!(
            filtered.metrics_for(&Provider::OllamaCloud),
            Some([MetricKind::Quota].as_slice())
        );

        config.set_view_hidden_providers(&[]);
        config.set_view_show_disabled_providers(false);
        config.set_view_visible_windows(&Provider::OllamaCloud, None);
        config.set_view_visible_metrics(&Provider::OllamaCloud, None);
        let restored = config.resolved_view(&available);
        assert!(restored.hidden_providers.is_empty());
        assert!(!restored.show_disabled_providers);
        assert!(restored.windows_for(&Provider::OllamaCloud).is_none());
        assert!(restored.metrics_for(&Provider::OllamaCloud).is_none());
    }

    #[test]
    fn unknown_view_identifiers_are_ignored_and_contradictions_fall_back() {
        let config = AppConfig::parse(
            r#"{
                "version": 1,
                "view": {
                    "version": 1,
                    "provider_order": ["missing", "codex", "codex"],
                    "hidden_providers": ["missing", "kimi"],
                    "providers": {
                        "codex": {
                            "visible_windows": ["not-a-window"],
                            "metric_kinds": ["not-a-metric"]
                        },
                        "kimi": {"visible_windows": []}
                    },
                    "default_provider": "kimi",
                    "default_window": "5-hour"
                }
            }"#,
        )
        .unwrap();

        let resolved = config.resolved_view(&[Provider::Codex, Provider::Kimi]);
        assert_eq!(resolved.provider_order, Some(vec![Provider::Codex]));
        assert_eq!(resolved.hidden_providers, vec![Provider::Kimi]);
        assert!(resolved.windows_for(&Provider::Codex).is_none());
        assert!(resolved.metrics_for(&Provider::Codex).is_none());
        assert_eq!(resolved.windows_for(&Provider::Kimi), Some([].as_slice()));
        assert_eq!(resolved.default_provider, None);
        assert_eq!(resolved.default_window, None);
    }

    #[test]
    fn unsupported_view_version_falls_back_without_rejecting_config() {
        let config =
            AppConfig::parse(r#"{"version":1,"view":{"version":99,"hidden_providers":["codex"]}}"#)
                .unwrap();

        assert_eq!(
            config.resolved_view(&[Provider::Codex]),
            ResolvedView::default()
        );
    }

    #[test]
    fn hidden_provider_is_disabled_at_registry_bootstrap() {
        let mut config = AppConfig::default();
        config.set_view_hidden_providers(&[Provider::Codex, Provider::Kimi]);

        let registry = build_registry(&config).unwrap();

        assert!(!registry.is_enabled(&Provider::Codex).unwrap());
        assert!(!registry.is_enabled(&Provider::Kimi).unwrap());
    }

    #[test]
    fn malformed_and_future_configs_fail_closed() {
        assert!(matches!(
            AppConfig::parse("not json"),
            Err(ConfigError::Parse(_))
        ));
        assert!(matches!(
            AppConfig::parse(r#"{"version": 2}"#),
            Err(ConfigError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn missing_file_uses_defaults() {
        let path = std::env::temp_dir().join(format!(
            "ai-usage-bar-config-missing-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let config = AppConfig::load(&path).unwrap();
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn save_round_trips_without_credentials() {
        let path = std::env::temp_dir().join(format!(
            "ai-usage-bar-config-roundtrip-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        let mut config = AppConfig::default();
        config.set_provider_enabled(&Provider::Codex, false);
        config.save(&path).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("token"));
        assert_eq!(AppConfig::load(&path).unwrap(), config);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn opencode_reset_anchors_round_trip_without_credentials() {
        let path = std::env::temp_dir().join(format!(
            "ai-usage-bar-opencode-reset-config-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        let weekly = DateTime::parse_from_rfc3339("2026-08-10T15:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let monthly = DateTime::parse_from_rfc3339("2026-09-06T15:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut config = AppConfig::default();
        config.set_opencode_reset_settings(OpenCodeResetSettings {
            weekly_reset_at: Some(weekly),
            monthly_reset_at: Some(monthly),
        });
        config.save(&path).unwrap();
        assert_eq!(
            AppConfig::load(&path).unwrap().opencode_reset_settings(),
            OpenCodeResetSettings {
                weekly_reset_at: Some(weekly),
                monthly_reset_at: Some(monthly),
            }
        );
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("token"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn bootstrap_registers_only_the_compiled_hosted_adapters() {
        let registry = build_registry(&AppConfig::default()).unwrap();
        assert_eq!(
            registry.registered_providers(),
            vec![
                Provider::Codex,
                Provider::GrokConsumer,
                Provider::OllamaCloud,
                Provider::OpenCodeGo,
                Provider::Kimi,
                Provider::Zai
            ]
        );
    }

    #[test]
    fn config_can_disable_a_built_in_provider() {
        let config =
            AppConfig::parse(r#"{"version":1,"providers":{"codex":{"enabled":false}}}"#).unwrap();
        let registry = build_registry(&config).unwrap();

        assert!(!registry.is_enabled(&Provider::Codex).unwrap());
        assert!(registry.is_enabled(&Provider::GrokConsumer).unwrap());
        assert!(!registry.is_enabled(&Provider::OllamaCloud).unwrap());
    }
}
