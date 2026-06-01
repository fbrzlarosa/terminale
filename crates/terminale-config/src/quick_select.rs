//! Quick-select / label-hint overlay configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Quick-select / label-hint mode configuration.
///
/// Quick-select overlays short keyboard labels on regex matches in the
/// visible screen + scrollback so the user can copy them without the mouse.
/// The same label renderer powers pane-select mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct QuickSelectConfig {
    /// Regex patterns scanned for matches when quick-select is activated.
    /// Each string must be a valid Rust `regex` crate pattern. Invalid
    /// patterns are silently skipped at runtime. The default set covers
    /// URLs, file paths, git SHAs, IPv4 addresses, hex colours, and UUIDs.
    pub patterns: Vec<String>,
    /// Characters used to build label keys, shortest-used first. Must be
    /// non-empty and contain only unique characters. Default is a
    /// home-row-biased alphabet.
    pub alphabet: String,
    /// Opacity of the full-screen dimming layer drawn behind the label badges
    /// while quick-select or pane-select is active. `0.0` = no dim (badges
    /// only), `1.0` = fully opaque. Clamped to `[0.0, 1.0]` at runtime.
    /// Default `0.45`.
    pub overlay_dim: f32,
}

impl Default for QuickSelectConfig {
    fn default() -> Self {
        Self {
            patterns: vec![
                r"https?://[^\s\x00-\x1f\x7f]{2,}".into(),
                r"ftp://[^\s\x00-\x1f\x7f]{2,}".into(),
                r"file://[^\s\x00-\x1f\x7f]{2,}".into(),
                r"\b[0-9a-fA-F]{7,40}\b".into(),
                r"\b(?:\d{1,3}\.){3}\d{1,3}\b".into(),
                r"#[0-9a-fA-F]{3}(?:[0-9a-fA-F]{3})?\b".into(),
                r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
                    .into(),
                r"(?:~|/)[^\s\x00-\x1f\x7f]{2,}".into(),
                r"[A-Za-z]:\\[^\s\x00-\x1f\x7f]{2,}".into(),
            ],
            alphabet: "asdfjklqwerzxcvghtybnuiopm".into(),
            overlay_dim: 0.45,
        }
    }
}

impl QuickSelectConfig {
    /// Validate the config, returning the first error encountered.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.alphabet.is_empty() {
            return Err(ConfigError::Invalid {
                field: "quick_select.alphabet",
                message: "must not be empty",
            });
        }
        // Check for duplicate characters.
        let mut seen = std::collections::HashSet::new();
        for c in self.alphabet.chars() {
            if !seen.insert(c) {
                return Err(ConfigError::Invalid {
                    field: "quick_select.alphabet",
                    message: "must contain only unique characters",
                });
            }
        }
        if !(0.0..=1.0).contains(&self.overlay_dim) {
            return Err(ConfigError::Invalid {
                field: "quick_select.overlay_dim",
                message: "must be in [0.0, 1.0]",
            });
        }
        Ok(())
    }
}

/// Validate a quick-select alphabet string for use in the settings UI.
///
/// Returns `None` when the alphabet is valid, or a human-readable error
/// message when it is empty or contains duplicate characters.
#[must_use]
pub fn quick_select_validate_alphabet(alphabet: &str) -> Option<String> {
    if alphabet.is_empty() {
        return Some("Alphabet must not be empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    for c in alphabet.chars() {
        if !seen.insert(c) {
            return Some(format!("Alphabet contains duplicate character '{c}'"));
        }
    }
    None
}
