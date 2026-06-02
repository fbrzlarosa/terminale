//! Self-update configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Controls the built-in self-updater (checks GitHub releases and replaces the
/// on-disk binary; the running session is never interrupted and the new version
/// applies on the next launch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UpdatesConfig {
    /// Check GitHub for a newer release on startup (in the background, never
    /// blocking the UI). Default: `true`.
    pub check_on_startup: bool,
    /// When a newer release is found, download and stage it automatically
    /// instead of only notifying. The update is verified (SHA-256) and the
    /// on-disk binary is replaced atomically; the running process is untouched
    /// and the new version applies on the next launch — never a forced restart.
    /// Default: `false` (notify only; the user installs on demand).
    pub auto_install: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            auto_install: false,
        }
    }
}

impl UpdatesConfig {
    /// Validate field ranges. Currently infallible; the `Result` keeps it
    /// uniform with the other config sections.
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
    fn defaults_check_on_startup_notify_only() {
        let c = UpdatesConfig::default();
        assert!(c.check_on_startup);
        assert!(!c.auto_install);
        c.validate().expect("default must validate");
    }

    #[test]
    fn roundtrip_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            updates: UpdatesConfig,
        }
        let w = Wrap {
            updates: UpdatesConfig {
                check_on_startup: false,
                auto_install: true,
            },
        };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert!(!back.updates.check_on_startup);
        assert!(back.updates.auto_install);
    }
}
