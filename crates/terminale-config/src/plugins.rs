//! Lua plugin loader configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Lua plugin loader configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PluginsConfig {
    /// Load `*.lua` files from this directory on startup. `None` =
    /// use the OS-standard `<config>/plugins/` location.
    pub directory: Option<std::path::PathBuf>,
    /// Master switch — disables the plugin host entirely when `false`.
    pub enabled: bool,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            directory: None,
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn plugins_enabled_default_is_true() {
        assert!(PluginsConfig::default().enabled);
    }

    #[test]
    fn plugins_directory_default_is_none() {
        assert!(PluginsConfig::default().directory.is_none());
    }

    #[test]
    fn plugins_config_roundtrips_via_toml() {
        // Disabled + custom directory must survive a TOML serialise/deserialise.
        let toml_src = "[plugins]\nenabled = false\ndirectory = \"/home/user/.plugins\"\n";
        let cfg: Config = toml::from_str(toml_src).expect("plugins config must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.plugins.enabled);
        assert_eq!(
            cfg.plugins.directory,
            Some(std::path::PathBuf::from("/home/user/.plugins"))
        );
        // Round-trip.
        let s = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&s).expect("deserialize roundtrip");
        assert!(!back.plugins.enabled);
        assert_eq!(back.plugins.directory, cfg.plugins.directory);
    }

    #[test]
    fn plugins_configs_identical_detects_diff() {
        // Two configs that differ only in plugins.enabled must NOT be identical.
        let mut a = Config::default();
        let mut b = Config::default();
        a.plugins.enabled = true;
        b.plugins.enabled = false;
        // The Config::default() `validate()` must still pass.
        a.validate().expect("validate a");
        b.validate().expect("validate b");
        let a_toml = toml::to_string(&a).expect("serialize a");
        let b_toml = toml::to_string(&b).expect("serialize b");
        assert_ne!(
            a_toml, b_toml,
            "differing plugins.enabled must produce different TOML"
        );
    }
}
