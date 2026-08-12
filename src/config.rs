//! User configuration and the shared built-in provider bootstrap.
//!
//! Configuration controls which compiled hosted providers are scheduled. It
//! deliberately contains no credentials: each hosted adapter discovers its
//! own local login/session surface.

use crate::{
    opencode_data_available, session_available, CodexAdapter, GrokConsumerAdapter, KimiAdapter,
    OllamaCloudAdapter, OpenCodeGoAdapter, OpenCodeResetSettings, Provider, ProviderRegistry,
    RegistryError,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: u32 = 1;
pub const CONFIG_FILE_NAME: &str = "config.json";
pub const CONFIG_DIR_NAME: &str = "AI Usage Bar";

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

fn default_provider_enabled() -> bool {
    true
}

/// Per-provider settings persisted in the user configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Whether this provider should be scheduled and rendered.
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

/// Versioned, non-secret application configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderSettings>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            providers: BTreeMap::new(),
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
        fs::write(path, format!("{document}\n")).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
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

    fn validate_version(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        Ok(())
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

    for provider in [
        Provider::Codex,
        Provider::GrokConsumer,
        Provider::OllamaCloud,
        Provider::Kimi,
        Provider::OpenCodeGo,
    ] {
        registry.set_enabled(&provider, config.provider_enabled(&provider))?;
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
                Provider::Kimi
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
