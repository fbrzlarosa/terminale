//! Cursor shape and animation configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    /// Solid filled rectangle covering the whole cell (classic DOS-style).
    Block,
    /// Hollow rectangle outline — an unfilled (non-solid) block cursor.
    OutlineBlock,
    /// Thin horizontal bar along the bottom of the cell.
    Underline,
    /// Thin vertical bar on the left of the cell (I-beam).
    Beam,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self::Underline
    }
}

impl CursorStyle {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [Self::Block, Self::OutlineBlock, Self::Underline, Self::Beam]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Block => "Block",
            Self::OutlineBlock => "Outline",
            Self::Underline => "Underline",
            Self::Beam => "Beam",
        }
    }
}

/// Cursor configuration. Mirrors the most common settings users tweak in
/// other terminals (style, blink rate, custom colour).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CursorConfig {
    /// Cursor shape.
    pub style: CursorStyle,
    /// Whether the cursor blinks when the terminal has focus.
    pub blink: bool,
    /// Half-period of the blink animation, in milliseconds. Most legacy
    /// terminals use 500–530 ms.
    pub blink_rate_ms: u32,
    /// Override cursor colour. `None` = use the theme's `cursor` palette
    /// entry (which in turn falls back to a soft accent blue).
    #[schemars(with = "Option<String>")]
    pub color: Option<[u8; 3]>,
    /// Stroke thickness in logical pixels (used by Underline / Beam /
    /// OutlineBlock). Block style ignores this.
    pub thickness_px: f32,
    /// Cursor fill opacity (0..1). Lets you dial down its intensity.
    pub opacity: f32,
    /// Optional faint background tint of the cell the cursor is on
    /// (helps locate the cursor in dense text). 0 disables.
    pub cell_tint_opacity: f32,
    /// When `true`, the cursor blink transitions smoothly using a
    /// `smoothstep` easing function instead of hard on/off switching.
    /// Only has an effect when `blink` is also `true`. Default: `false`.
    pub blink_ease: bool,
    /// Target animation frame rate for the blink ease animation, in
    /// frames per second. Higher values produce a smoother fade at the
    /// cost of more redraws. Range: `10..=240`. Default: `60`.
    pub animation_fps: u32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyle::default(),
            blink: false,
            blink_rate_ms: 530,
            color: None,
            thickness_px: 2.0,
            opacity: 1.0,
            cell_tint_opacity: 0.18,
            blink_ease: false,
            animation_fps: 60,
        }
    }
}

impl CursorConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err(ConfigError::Invalid {
                field: "cursor.opacity",
                message: "must be between 0.0 and 1.0",
            });
        }
        if !(0.0..=1.0).contains(&self.cell_tint_opacity) {
            return Err(ConfigError::Invalid {
                field: "cursor.cell_tint_opacity",
                message: "must be between 0.0 and 1.0",
            });
        }
        if !(0.5..=6.0).contains(&self.thickness_px) {
            return Err(ConfigError::Invalid {
                field: "cursor.thickness_px",
                message: "must be between 0.5 and 6.0",
            });
        }
        if !(60..=5000).contains(&self.blink_rate_ms) {
            return Err(ConfigError::Invalid {
                field: "cursor.blink_rate_ms",
                message: "must be between 60 and 5000",
            });
        }
        if !(10..=240).contains(&self.animation_fps) {
            return Err(ConfigError::Invalid {
                field: "cursor.animation_fps",
                message: "must be between 10 and 240",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_defaults_valid() {
        let cfg = CursorConfig::default();
        assert!(cfg.validate().is_ok());
        assert!(!cfg.blink_ease);
        assert_eq!(cfg.animation_fps, 60);
    }

    #[test]
    fn animation_fps_out_of_range() {
        assert!(CursorConfig { animation_fps: 9, ..Default::default() }.validate().is_err());
        assert!(CursorConfig { animation_fps: 241, ..Default::default() }.validate().is_err());
        assert!(CursorConfig { animation_fps: 60, ..Default::default() }.validate().is_ok());
    }

    #[test]
    fn blink_ease_roundtrip() {
        let cfg = CursorConfig { blink_ease: true, animation_fps: 120, ..Default::default() };
        let toml = toml::to_string(&cfg).unwrap();
        let parsed: CursorConfig = toml::from_str(&toml).unwrap();
        assert!(parsed.blink_ease);
        assert_eq!(parsed.animation_fps, 120);
    }
}
