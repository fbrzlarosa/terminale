//! Configurable status bar configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which edge the status bar is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StatusBarPosition {
    /// Draw the status bar at the top of the terminal body (below the tab bar).
    Top,
    /// Draw the status bar at the bottom of the terminal body (default).
    Bottom,
}

impl Default for StatusBarPosition {
    fn default() -> Self {
        Self::Bottom
    }
}

impl StatusBarPosition {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Top, Self::Bottom]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        }
    }
}

/// One segment in the status bar.
///
/// Segments are ordered left-to-right within a side. For TOML round-tripping
/// each variant is serialised with an externally-tagged representation, e.g.
/// `{ type = "clock", format = "%H:%M" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatusSegment {
    /// Current working directory, shortened to `~/last-component` form.
    Cwd,
    /// Wall-clock time formatted with a `strftime`-style format string.
    Clock {
        /// `strftime`-style format. Default `"%H:%M"`.
        format: String,
    },
    /// Active profile name.
    Profile,
    /// `"n/total"` tab index / count.
    TabIndex,
    /// Value of an OSC 1337 `SetUserVar` variable. Empty when unset.
    UserVar {
        /// Variable name passed to `SetUserVar`.
        name: String,
    },
    /// Verbatim text string.
    Literal {
        /// The text to display.
        text: String,
    },
    /// Flexible space between left and right groups (only meaningful in the
    /// left or right list itself — treated as empty string in `compose`).
    Spacer,
}

/// Status-bar configuration block.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StatusBarConfig {
    /// Master enable switch. `false` by default.
    pub enabled: bool,
    /// Which edge the bar appears on: `top` or `bottom`. Default `bottom`.
    pub position: StatusBarPosition,
    /// Segments shown on the left side, ordered left-to-right.
    pub left_segments: Vec<StatusSegment>,
    /// Segments shown on the right side, ordered left-to-right.
    pub right_segments: Vec<StatusSegment>,
    /// How often the bar is refreshed, in milliseconds. Clamped to
    /// `200..=60000`. Default `1000`.
    pub update_interval_ms: u32,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            position: StatusBarPosition::Bottom,
            left_segments: vec![StatusSegment::Cwd],
            right_segments: vec![
                StatusSegment::Profile,
                StatusSegment::Clock {
                    format: "%H:%M".into(),
                },
            ],
            update_interval_ms: 1000,
        }
    }
}

impl StatusBarConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(200..=60000).contains(&self.update_interval_ms) {
            return Err(ConfigError::Invalid {
                field: "status_bar.update_interval_ms",
                message: "must be between 200 and 60000",
            });
        }
        Ok(())
    }

    /// Returns `true` when any segment in either side could show time-varying
    /// content (i.e. a `Clock` segment is present), meaning periodic redraws
    /// are needed even without terminal output.
    #[must_use]
    pub fn has_time_segment(&self) -> bool {
        let is_clock = |s: &StatusSegment| matches!(s, StatusSegment::Clock { .. });
        self.left_segments.iter().any(is_clock) || self.right_segments.iter().any(is_clock)
    }
}
