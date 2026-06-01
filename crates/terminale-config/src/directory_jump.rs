//! Configuration for the directory-jump frecency picker.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the directory-jump frecency navigation feature.
///
/// The directory-jump picker tracks every directory the user visits (via OSC 7
/// cwd reports) and ranks them by a combined frequency + recency score
/// ("frecency"). Opening the picker lets the user fuzzy-search the ranked list
/// and jump the active shell to any of those directories by sending
/// `cd <path>\n` to the focused pane's PTY.
///
/// The feature requires no third-party tools — it works with any shell that
/// emits OSC 7 on directory changes (zsh, bash with `$PROMPT_COMMAND`, fish,
/// PowerShell with the `cd` wrapper, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DirectoryJumpConfig {
    /// Enable directory-jump tracking. When `false`, no new visits are
    /// recorded and the picker returns an empty list. Existing history
    /// is preserved on disk when `persist` is also `true`.
    /// Default: `true`.
    pub enabled: bool,
    /// Maximum number of directory entries the store keeps. Once the cap is
    /// reached the entry with the lowest frecency score is evicted to make
    /// room for the new one. Must be in `[1, 2000]`.
    /// Default: `200`.
    pub max_tracked: usize,
    /// Persist the visit history to disk so it survives restarts. The file
    /// is written to `<data_dir>/dir_history.toml` on each OSC 7 update
    /// (debounced: only when the entry actually changed). When `false`, the
    /// store is kept entirely in memory and is reset on exit.
    /// Default: `true`.
    pub persist: bool,
}

impl Default for DirectoryJumpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tracked: 200,
            persist: true,
        }
    }
}

impl DirectoryJumpConfig {
    /// Validate field ranges.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `max_tracked` is outside `[1, 2000]`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.max_tracked == 0 || self.max_tracked > 2000 {
            return Err(ConfigError::Invalid {
                field: "directory_jump.max_tracked",
                message: "must be between 1 and 2000",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enabled_and_sane() {
        let cfg = DirectoryJumpConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_tracked, 200);
        assert!(cfg.persist);
        cfg.validate().expect("default must validate");
    }

    #[test]
    fn rejects_max_tracked_zero() {
        let cfg = DirectoryJumpConfig { max_tracked: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_max_tracked_over_2000() {
        let cfg = DirectoryJumpConfig { max_tracked: 2001, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn accepts_boundary_values() {
        let min = DirectoryJumpConfig { max_tracked: 1, ..Default::default() };
        min.validate().expect("max_tracked=1 must validate");
        let max = DirectoryJumpConfig { max_tracked: 2000, ..Default::default() };
        max.validate().expect("max_tracked=2000 must validate");
    }

    #[test]
    fn roundtrip_toml() {
        let cfg = DirectoryJumpConfig {
            enabled: false,
            max_tracked: 50,
            persist: false,
        };
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrap {
            directory_jump: DirectoryJumpConfig,
        }
        let w = Wrap { directory_jump: cfg };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert!(!back.directory_jump.enabled);
        assert_eq!(back.directory_jump.max_tracked, 50);
        assert!(!back.directory_jump.persist);
    }
}
