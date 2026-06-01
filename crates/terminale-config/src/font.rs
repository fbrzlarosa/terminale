//! Font configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Font configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct FontConfig {
    /// Family name (e.g. `"JetBrains Mono"`).
    ///
    /// A curated set of monospace typefaces is embedded in the binary and is
    /// always selectable regardless of what is installed on the OS: Ubuntu
    /// Mono, Source Code Pro, IBM Plex Mono, JetBrains Mono, and
    /// Inconsolata. The default (`"JetBrains Mono"`) resolves from this
    /// bundled set on a fresh machine with no extra fonts installed.
    pub family: String,
    /// Override family used for **bold** text.
    /// `None` = synthesize boldness from `family` (default behaviour).
    /// When set, bold cells use this family with normal weight so the
    /// dedicated bold cut provides the letterforms.
    pub bold_family: Option<String>,
    /// Override family used for **italic** text.
    /// `None` = synthesize italics from `family` (default behaviour).
    pub italic_family: Option<String>,
    /// Override family used for text that is **both bold and italic**.
    /// Falls back to `bold_family` or `italic_family` if not set.
    /// `None` = synthesize bold-italic from `family` (default behaviour).
    pub bold_italic_family: Option<String>,
    /// Size in points.
    pub size: f32,
    /// Line height multiplier.
    pub line_height: f32,
    /// Enable ligatures when the font supports them.
    pub ligatures: bool,
    /// Thickness of SGR underlines (single, double, dotted, dashed,
    /// curly) in physical pixels **before** scale-factor is applied.
    /// Valid range: `0.5 ..= 4.0`. Default `1.0`.
    pub underline_thickness_px: f32,
    /// Cell width multiplier applied to the monospace advance. Values
    /// above `1.0` widen each cell; below `1.0` narrow it. Glyphs are
    /// **not** stretched — only the grid spacing changes.
    /// Valid range: `0.8 ..= 2.0`. Default `1.0`.
    pub cell_width: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "JetBrains Mono".to_string(),
            bold_family: None,
            italic_family: None,
            bold_italic_family: None,
            size: 14.0,
            line_height: 1.2,
            ligatures: true,
            underline_thickness_px: 1.0,
            cell_width: 1.0,
        }
    }
}

impl FontConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(4.0..=144.0).contains(&self.size) {
            return Err(ConfigError::Invalid {
                field: "font.size",
                message: "must be between 4.0 and 144.0",
            });
        }
        if !(0.8..=3.0).contains(&self.line_height) {
            return Err(ConfigError::Invalid {
                field: "font.line_height",
                message: "must be between 0.8 and 3.0",
            });
        }
        if !(0.5..=4.0).contains(&self.underline_thickness_px) {
            return Err(ConfigError::Invalid {
                field: "font.underline_thickness_px",
                message: "must be between 0.5 and 4.0",
            });
        }
        if !(0.8..=2.0).contains(&self.cell_width) {
            return Err(ConfigError::Invalid {
                field: "font.cell_width",
                message: "must be between 0.8 and 2.0",
            });
        }
        if self.bold_family.as_deref().is_some_and(str::is_empty) {
            return Err(ConfigError::Invalid {
                field: "font.bold_family",
                message: "must not be empty when set (use null/None to derive from main family)",
            });
        }
        if self.italic_family.as_deref().is_some_and(str::is_empty) {
            return Err(ConfigError::Invalid {
                field: "font.italic_family",
                message: "must not be empty when set (use null/None to derive from main family)",
            });
        }
        if self
            .bold_italic_family
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(ConfigError::Invalid {
                field: "font.bold_italic_family",
                message: "must not be empty when set (use null/None to derive from main family)",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_defaults_valid() {
        let cfg = FontConfig::default();
        assert!(cfg.validate().is_ok());
        assert!((cfg.cell_width - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn cell_width_bounds() {
        assert!(FontConfig { cell_width: 0.79, ..Default::default() }.validate().is_err());
        assert!(FontConfig { cell_width: 2.01, ..Default::default() }.validate().is_err());
        assert!(FontConfig { cell_width: 1.5, ..Default::default() }.validate().is_ok());
    }

    #[test]
    fn cell_width_roundtrip() {
        let cfg = FontConfig { cell_width: 1.2, ..Default::default() };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: FontConfig = toml::from_str(&toml).unwrap();
        assert!((parsed.cell_width - 1.2).abs() < 1e-5);
    }

    // ── per-style font override tests ──────────────────────────────────────

    #[test]
    fn font_override_defaults_none() {
        let cfg = FontConfig::default();
        assert!(cfg.bold_family.is_none());
        assert!(cfg.italic_family.is_none());
        assert!(cfg.bold_italic_family.is_none());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn font_override_some_valid() {
        let cfg = FontConfig {
            bold_family: Some("JetBrains Mono Bold".to_string()),
            italic_family: Some("Fira Code Italic".to_string()),
            bold_italic_family: Some("Cascadia Code Bold Italic".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn font_override_empty_string_invalid() {
        let bold_empty = FontConfig {
            bold_family: Some(String::new()),
            ..Default::default()
        };
        assert!(bold_empty.validate().is_err());

        let italic_empty = FontConfig {
            italic_family: Some(String::new()),
            ..Default::default()
        };
        assert!(italic_empty.validate().is_err());

        let bi_empty = FontConfig {
            bold_italic_family: Some(String::new()),
            ..Default::default()
        };
        assert!(bi_empty.validate().is_err());
    }

    #[test]
    fn font_override_roundtrip() {
        let cfg = FontConfig {
            bold_family: Some("MyBoldFont".to_string()),
            italic_family: None,
            bold_italic_family: Some("MyBoldItalicFont".to_string()),
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: FontConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.bold_family.as_deref(), Some("MyBoldFont"));
        assert!(parsed.italic_family.is_none());
        assert_eq!(parsed.bold_italic_family.as_deref(), Some("MyBoldItalicFont"));
        assert!(parsed.validate().is_ok());
    }
}
