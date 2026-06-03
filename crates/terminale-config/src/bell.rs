//! Audible / visual bell configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Audible / visual feedback when an app emits `BEL` (`\x07`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum BellMode {
    /// In-window flash overlay only (default).
    #[default]
    Visual,
    /// System beep / taskbar attention only.
    Audio,
    /// Both visual flash and system beep fire.
    Both,
    /// Bell is fully silenced.
    None,
}

impl BellMode {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [Self::Visual, Self::Audio, Self::Both, Self::None]
    }
}

/// Bell configuration block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct BellConfig {
    /// What to do when the focused app emits `BEL`.
    pub mode: BellMode,
}
