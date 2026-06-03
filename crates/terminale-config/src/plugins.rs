//! Lua plugin loader configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Upper bound for [`PluginsConfig::scrollback_read_cap`] — keeps a plugin
/// read from cloning an unbounded (up to 1M-line) scrollback every tick.
pub const SCROLLBACK_READ_CAP_MAX: usize = 200_000;

/// Lua plugin loader configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PluginsConfig {
    /// Load `*.lua` files from this directory on startup. `None` =
    /// use the OS-standard `<config>/plugins/` location.
    pub directory: Option<std::path::PathBuf>,
    /// Master switch — disables the plugin host entirely when `false`.
    pub enabled: bool,
    /// Allow plugins to read terminal contents (`get_scrollback`,
    /// `get_visible_text`). Default `false`: terminal output can contain
    /// secrets, so content reads are a privacy opt-in. When off, those APIs
    /// return empty results. Applied live.
    pub allow_scrollback_read: bool,
    /// Maximum number of scrollback lines a plugin can read per call.
    /// Bounds the per-tick copy regardless of the configured scrollback
    /// depth. Default `10_000`; max `200_000`. Applied live.
    pub scrollback_read_cap: usize,
    /// Allow plugins to register keyboard shortcuts via
    /// `register_keybinding`. Plugin bindings can never shadow the user's
    /// own keybinds or config shortcuts. Default `true`. Applied live.
    pub allow_keybindings: bool,
    /// Maximum wall-clock milliseconds a single plugin hook (or the plugin's
    /// top-level load chunk) may run before it is aborted with an error and
    /// the offending handler is dropped. Protects the UI thread — hooks run
    /// synchronously on it, so a `while true do end` would otherwise freeze
    /// the whole app. `0` disables the budget. Default `100`. Applied live.
    pub hook_budget_ms: u64,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            directory: None,
            enabled: true,
            allow_scrollback_read: false,
            scrollback_read_cap: 10_000,
            allow_keybindings: true,
            hook_budget_ms: 100,
        }
    }
}

impl PluginsConfig {
    /// Validate field ranges.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `scrollback_read_cap` exceeds
    /// [`SCROLLBACK_READ_CAP_MAX`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.scrollback_read_cap > SCROLLBACK_READ_CAP_MAX {
            return Err(ConfigError::Invalid {
                field: "plugins.scrollback_read_cap",
                message: "must be at most 200000 lines",
            });
        }
        Ok(())
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
    fn plugins_new_fields_default() {
        let c = PluginsConfig::default();
        assert!(
            !c.allow_scrollback_read,
            "scrollback read must be a privacy OPT-IN (default off)"
        );
        assert_eq!(c.scrollback_read_cap, 10_000);
        assert!(c.allow_keybindings, "keybindings default on");
        c.validate().expect("default must validate");
    }

    #[test]
    fn plugins_scrollback_cap_validates() {
        let mut c = PluginsConfig {
            scrollback_read_cap: SCROLLBACK_READ_CAP_MAX,
            ..Default::default()
        };
        c.validate().expect("cap at the max must pass");
        c.scrollback_read_cap = SCROLLBACK_READ_CAP_MAX + 1;
        assert!(
            c.validate().is_err(),
            "cap above SCROLLBACK_READ_CAP_MAX must be rejected"
        );
    }

    #[test]
    fn plugins_new_fields_roundtrip() {
        let toml_src = "[plugins]\nallow_scrollback_read = true\nscrollback_read_cap = 500\nallow_keybindings = false\n";
        let cfg: Config = toml::from_str(toml_src).expect("must parse");
        cfg.validate().expect("must validate");
        assert!(cfg.plugins.allow_scrollback_read);
        assert_eq!(cfg.plugins.scrollback_read_cap, 500);
        assert!(!cfg.plugins.allow_keybindings);
        let s = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&s).expect("roundtrip");
        assert_eq!(back.plugins, cfg.plugins);
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
