//! Desktop / OS integration configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Controls how `terminale` integrates with the host desktop environment.
///
/// On Windows the MSI installer registers Start-Menu / Desktop shortcuts and on
/// macOS the `.app` bundle is placed in `/Applications`, so those platforms are
/// discoverable out of the box. Linux has no install-time hook (the app ships as
/// a plain tarball or via Homebrew), so the binary registers its own
/// `freedesktop` `.desktop` entry on launch — that is what [`Self::desktop_entry`]
/// governs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct IntegrationConfig {
    /// On Linux, register a `freedesktop` desktop entry (and icon) under
    /// `$XDG_DATA_HOME/applications` on launch so `terminale` shows up in the
    /// application menu / launcher and is searchable. The write is idempotent
    /// and only refreshed when the executable path changes. No effect on
    /// Windows or macOS, where the installer/bundle handles this.
    /// Default: `true`.
    pub desktop_entry: bool,
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self {
            desktop_entry: true,
        }
    }
}

impl IntegrationConfig {
    /// Validate field ranges. Currently infallible; the `Result` return type
    /// is kept for uniformity with the other config sections (so `Config::
    /// validate` can call it the same way) and to leave room for future fields.
    ///
    /// # Errors
    ///
    /// Never returns an error.
    #[allow(clippy::unnecessary_wraps)]
    pub fn validate(&self) -> Result<(), ConfigError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registers_desktop_entry() {
        let cfg = IntegrationConfig::default();
        assert!(cfg.desktop_entry);
        cfg.validate().expect("default must validate");
    }

    #[test]
    fn roundtrip_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            integration: IntegrationConfig,
        }
        let w = Wrap {
            integration: IntegrationConfig {
                desktop_entry: false,
            },
        };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert!(!back.integration.desktop_entry);
    }
}
