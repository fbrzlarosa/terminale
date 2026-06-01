//! Animated background-effect configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Animated background "wallpaper" effect rendered behind the terminal grid.
/// Unlike `KeystrokeFxConfig` (transient particles at
/// the cursor), this is a continuous full-screen shader that plays under the
/// text. Off by default; purely cosmetic but very visible — the "wow" layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundFxStyle {
    /// No animated background (style-level no-op, distinct from the master
    /// `enabled` toggle so the user keeps their selection).
    None,
    /// Flowing aurora / plasma gradient — layered sine fields, slow drift.
    AuroraPlasma,
    /// Parallax starfield with twinkle.
    Starfield,
    /// Falling "Matrix" code-rain columns (procedural glyph streaks).
    Matrix,
    /// Retro pixelated plasma with CRT scanlines.
    PixelCrt,
}

impl Default for BackgroundFxStyle {
    fn default() -> Self {
        Self::AuroraPlasma
    }
}

impl BackgroundFxStyle {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [
            Self::None,
            Self::AuroraPlasma,
            Self::Starfield,
            Self::Matrix,
            Self::PixelCrt,
        ]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::AuroraPlasma => "Aurora / plasma",
            Self::Starfield => "Starfield",
            Self::Matrix => "Matrix rain",
            Self::PixelCrt => "Pixel CRT",
        }
    }

    /// Stable index passed to the shader as the `mode` uniform.
    #[must_use]
    pub fn shader_mode(self) -> u32 {
        match self {
            Self::None => 0,
            Self::AuroraPlasma => 1,
            Self::Starfield => 2,
            Self::Matrix => 3,
            Self::PixelCrt => 4,
        }
    }
}

/// Animated background-effect configuration. Disabled by default.
///
/// Does not `deny_unknown_fields` so obsolete keys from earlier iterations
/// (e.g. `matrix_drops_per_key`, dropped when Matrix became a continuous
/// character rain) keep parsing instead of failing the whole config load.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BackgroundFxConfig {
    /// Master enable switch. `false` by default.
    pub enabled: bool,
    /// Which animated style to draw when `enabled` is `true`.
    pub style: BackgroundFxStyle,
    /// Overall strength / opacity of the effect, `0.0..=1.0`. Kept modest by
    /// default so text stays readable. Clamped on validate.
    pub intensity: f32,
    /// Animation speed multiplier, `0.1..=5.0`.
    pub speed: f32,
    /// Primary tint (sRGB). `None` uses the style's built-in palette.
    pub color1: Option<[u8; 3]>,
    /// Secondary tint (sRGB). `None` uses the style's built-in palette.
    pub color2: Option<[u8; 3]>,
    /// When `true`, every keystroke spawns a new animated band / emitter in
    /// the background effect — each keypress starts a concurrent band that
    /// travels and decays independently. Multiple keystrokes layer visibly.
    /// Default: `true`.
    pub react_to_keystrokes: bool,
    /// How long each per-keystroke emitter lives before fully fading out,
    /// in seconds. `0.5..=8.0`; default `2.5`.
    pub band_lifetime_secs: f32,
    /// For the Matrix style: width of each rain band in character columns.
    /// Wider bands look chunkier. `1..=8`; default `3`.
    pub matrix_band_width: u32,
    /// For the Matrix style: base fall speed in character rows per second.
    /// Each band gets a random variation ±30 %. `4.0..=60.0`; default `14.0`.
    pub matrix_fall_speed: f32,
    /// Maximum number of concurrently alive emitter bands. Oldest bands are
    /// evicted when the cap is reached. `1..=64`; default `48`.
    pub max_emitters: u32,
}

impl Default for BackgroundFxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            style: BackgroundFxStyle::AuroraPlasma,
            intensity: 0.35,
            speed: 1.0,
            color1: None,
            color2: None,
            react_to_keystrokes: true,
            band_lifetime_secs: 2.5,
            matrix_band_width: 3,
            matrix_fall_speed: 14.0,
            max_emitters: 48,
        }
    }
}

impl BackgroundFxConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(0.0..=1.0).contains(&self.intensity) {
            return Err(ConfigError::Invalid {
                field: "background_fx.intensity",
                message: "must be between 0.0 and 1.0",
            });
        }
        if !(0.1..=5.0).contains(&self.speed) {
            return Err(ConfigError::Invalid {
                field: "background_fx.speed",
                message: "must be between 0.1 and 5.0",
            });
        }
        if !(0.5..=8.0).contains(&self.band_lifetime_secs) {
            return Err(ConfigError::Invalid {
                field: "background_fx.band_lifetime_secs",
                message: "must be between 0.5 and 8.0",
            });
        }
        if !(1..=8).contains(&self.matrix_band_width) {
            return Err(ConfigError::Invalid {
                field: "background_fx.matrix_band_width",
                message: "must be between 1 and 8",
            });
        }
        if !(4.0..=60.0).contains(&self.matrix_fall_speed) {
            return Err(ConfigError::Invalid {
                field: "background_fx.matrix_fall_speed",
                message: "must be between 4.0 and 60.0",
            });
        }
        if !(1..=64).contains(&self.max_emitters) {
            return Err(ConfigError::Invalid {
                field: "background_fx.max_emitters",
                message: "must be between 1 and 64",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_passes_validation() {
        BackgroundFxConfig::default().validate().unwrap();
    }

    #[test]
    fn band_lifetime_bounds() {
        assert!(BackgroundFxConfig {
            band_lifetime_secs: 0.4,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(BackgroundFxConfig {
            band_lifetime_secs: 8.1,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(BackgroundFxConfig {
            band_lifetime_secs: 2.5,
            ..Default::default()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn matrix_band_width_bounds() {
        assert!(BackgroundFxConfig {
            matrix_band_width: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(BackgroundFxConfig {
            matrix_band_width: 9,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(BackgroundFxConfig {
            matrix_band_width: 3,
            ..Default::default()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn max_emitters_bounds() {
        assert!(BackgroundFxConfig {
            max_emitters: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(BackgroundFxConfig {
            max_emitters: 65,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(BackgroundFxConfig {
            max_emitters: 48,
            ..Default::default()
        }
        .validate()
        .is_ok());
    }
}
