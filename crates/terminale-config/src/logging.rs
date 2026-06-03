//! Diagnostic file-logging configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Controls the rolling diagnostic log file written next to the config
/// (`<config dir>/logs/terminale.log.<date>`). The console log (visible when
/// launching from a shell) is independent and always follows `--log-level`.
///
/// File logging exists so a freeze or crash leaves evidence: a GUI launch has
/// no console, and without a log file there is nothing to inspect afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    /// Write a rolling daily log file. Default: `true`. Files older than
    /// [`LoggingConfig::retention_days`] are deleted at startup. Requires a
    /// restart to take effect.
    pub file_enabled: bool,
    /// Level filter for the log file (`error` / `warn` / `info` / `debug` /
    /// `trace`, or any `tracing` filter directive such as
    /// `terminale=debug`). Default: `info`. Requires a restart.
    pub file_level: String,
    /// Delete log files older than this many days at startup. Default: `7`;
    /// max `365`.
    pub retention_days: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            file_enabled: true,
            file_level: "info".to_owned(),
            retention_days: 7,
        }
    }
}

impl LoggingConfig {
    /// Validate field ranges.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `file_level` is empty or
    /// `retention_days` is out of range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.file_level.trim().is_empty() {
            return Err(ConfigError::Invalid {
                field: "logging.file_level",
                message: "must not be empty (use e.g. \"info\" or \"debug\")",
            });
        }
        if self.retention_days == 0 || self.retention_days > 365 {
            return Err(ConfigError::Invalid {
                field: "logging.retention_days",
                message: "must be between 1 and 365",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enabled_info_seven_days() {
        let c = LoggingConfig::default();
        assert!(c.file_enabled);
        assert_eq!(c.file_level, "info");
        assert_eq!(c.retention_days, 7);
        c.validate().expect("default must validate");
    }

    #[test]
    fn rejects_empty_level_and_bad_retention() {
        let mut c = LoggingConfig {
            file_level: "  ".into(),
            ..Default::default()
        };
        assert!(c.validate().is_err(), "empty level must be rejected");
        c.file_level = "debug".into();
        c.retention_days = 0;
        assert!(c.validate().is_err(), "0 retention must be rejected");
        c.retention_days = 400;
        assert!(c.validate().is_err(), ">365 retention must be rejected");
    }

    #[test]
    fn roundtrip_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            logging: LoggingConfig,
        }
        let w = Wrap {
            logging: LoggingConfig {
                file_enabled: false,
                file_level: "terminale=trace".into(),
                retention_days: 30,
            },
        };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.logging, w.logging);
    }
}
