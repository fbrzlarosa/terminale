//! Clipboard history ring-buffer configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the clipboard history ring buffer.
///
/// The history is kept in memory only — nothing is written to disk.
/// Entries are added every time text is copied (selection, copy-mode yank,
/// block copy). The picker (`OpenClipboardHistory`) lets you fuzzy-search
/// and re-paste any retained entry into the focused pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ClipboardHistoryConfig {
    /// Enable the clipboard history ring buffer. When `false`, no entries
    /// are captured and the picker returns an empty list.
    /// Default: `true`.
    pub enabled: bool,
    /// Maximum number of entries the ring keeps. Oldest entries are evicted
    /// once the ring is full. Must be in `[1, 500]`.
    /// Default: `20`.
    pub size: usize,
    /// When `true`, text written to the clipboard via OSC 52 (programmatic
    /// clipboard set from a running application) is also captured in the
    /// history. Defaults to `false` for privacy — OSC 52 payloads often
    /// contain tokens, passwords, or secrets that should not accumulate in a
    /// browseable list.
    pub capture_osc52: bool,
}

impl Default for ClipboardHistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 20,
            capture_osc52: false,
        }
    }
}

impl ClipboardHistoryConfig {
    /// Validate field ranges.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `size` is outside `[1, 500]`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.size == 0 || self.size > 500 {
            return Err(ConfigError::Invalid {
                field: "clipboard_history.size",
                message: "must be between 1 and 500",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enabled_and_size_20() {
        let cfg = ClipboardHistoryConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.size, 20);
        assert!(!cfg.capture_osc52);
        cfg.validate().expect("default must validate");
    }

    #[test]
    fn rejects_size_zero() {
        let cfg = ClipboardHistoryConfig {
            size: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_size_over_500() {
        let cfg = ClipboardHistoryConfig {
            size: 501,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn accepts_size_1_and_500() {
        let min = ClipboardHistoryConfig {
            size: 1,
            ..Default::default()
        };
        min.validate().expect("size 1 must validate");
        let max = ClipboardHistoryConfig {
            size: 500,
            ..Default::default()
        };
        max.validate().expect("size 500 must validate");
    }

    #[test]
    fn roundtrip_toml() {
        let cfg = ClipboardHistoryConfig {
            size: 30,
            capture_osc52: true,
            ..Default::default()
        };
        // Wrap in a helper struct to test TOML serde.
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            clipboard_history: ClipboardHistoryConfig,
        }
        let w = Wrap {
            clipboard_history: cfg,
        };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.clipboard_history.size, 30);
        assert!(back.clipboard_history.capture_osc52);
    }
}
