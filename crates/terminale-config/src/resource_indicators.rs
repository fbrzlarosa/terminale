//! Bottom resource-indicator strip configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Controls the pixel-art CPU/RAM/GPU indicator strip drawn in a reserved band
/// at the very bottom of the window (below the terminal grid, so it never
/// overlaps content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ResourceIndicatorsConfig {
    /// Show the bottom resource-indicator strip (CPU%, RAM%, GPU label). When
    /// enabled the grid is shortened by the strip's height. Default: `true`.
    pub enabled: bool,
}

impl Default for ResourceIndicatorsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl ResourceIndicatorsConfig {
    /// Validate field ranges. Currently infallible; the `Result` keeps it
    /// uniform with the other config sections and leaves room for future fields.
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
    fn default_enabled() {
        assert!(ResourceIndicatorsConfig::default().enabled);
        ResourceIndicatorsConfig::default()
            .validate()
            .expect("default must validate");
    }

    #[test]
    fn roundtrip_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            resource_indicators: ResourceIndicatorsConfig,
        }
        let w = Wrap {
            resource_indicators: ResourceIndicatorsConfig { enabled: false },
        };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert!(!back.resource_indicators.enabled);
    }
}
