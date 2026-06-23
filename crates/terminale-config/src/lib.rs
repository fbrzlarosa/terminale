//! TOML-backed configuration for `terminale`.
//!
//! Loading is layered (see [`figment`]):
//! 1. Hard-coded defaults from [`Config::default`].
//! 2. User config TOML on disk at the OS-standard path.
//! 3. Optional `--config <path>` override.
//! 4. Environment variables under the `TERMINALE_` prefix.
//!
//! Every field implements [`schemars::JsonSchema`] so a JSON schema can be
//! generated for editor integrations.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

// ── Pre-existing submodules ───────────────────────────────────────────────────
pub mod backup;
pub mod paths;
pub mod profile;
pub mod secrets;
pub mod ssh;
pub mod theme;
pub mod theme_catalog;

// ── Domain submodules (split from the former monolithic lib.rs) ───────────────
pub mod ai;
pub mod appearance;
pub mod background_fx;
pub mod bell;
pub mod clipboard_history;
pub mod context_rules;
pub mod cursor;
pub mod directory_jump;
pub mod font;
pub mod gpu;
pub mod integration;
pub mod keybinds;
pub mod logging;
pub mod plugins;
pub mod profiles_config;
pub mod quake;
pub mod quick_select;
pub mod resource_indicators;
pub mod snippets;
pub mod status_bar;
pub mod terminal;
pub mod updates;
pub mod window;

// ── Re-exports: pre-existing modules ─────────────────────────────────────────
pub use backup::{BackupCredential, BackupError, BackupPayload};
pub use profile::{auto_detect_profiles, Profile};
pub use secrets::{delete_secret, get_secret, store_secret, SecretError};
pub use ssh::{
    dedupe_imported_hosts, default_known_hosts_path, default_openssh_config_path, default_ssh_port,
    parse_ssh_config, HostKeyPolicy, ImportOpenSshConfig, ParsedSshHost, SshAuthMethod, SshConfig,
    SshHost,
};
pub use theme::{builtin_themes, ResolvedTheme, Theme};

// ── Re-exports: domain modules (identical public API as before) ───────────────
pub use ai::{
    AiConfig, AiSuggestionsConfig, ClaudeAiConfig, OllamaAiConfig, OpenAiAiConfig,
    SuggestionTrigger,
};
pub use appearance::{
    scan_themes_dir, AppearanceConfig, BackgroundImageConfig, BgImageFit, CloseButtonStyle,
    TabBarPosition,
};
pub use background_fx::{BackgroundFxConfig, BackgroundFxStyle};
pub use bell::{BellConfig, BellMode};
pub use clipboard_history::ClipboardHistoryConfig;
pub use context_rules::{evaluate_context_rules, ContextRule};
pub use cursor::{CursorConfig, CursorStyle};
pub use directory_jump::DirectoryJumpConfig;
pub use font::FontConfig;
pub use gpu::{GpuBackend, GpuConfig, GpuPowerPreference};
pub use integration::IntegrationConfig;
pub use keybinds::{
    decode_send_string, CustomKeybind, KeyActionSpec, KeyTable, KeyTableEntry, KeybindsConfig,
    MouseBinding, ShortcutsConfig,
};
pub use logging::LoggingConfig;
pub use plugins::PluginsConfig;
pub use profiles_config::ProfilesConfig;
pub use quake::{quake_dock_rect, QuakeAnimation, QuakeConfig, QuakeDisplay, QuakeEdge};
pub use quick_select::{quick_select_validate_alphabet, QuickSelectConfig};
pub use resource_indicators::ResourceIndicatorsConfig;
pub use snippets::Snippet;
pub use status_bar::{StatusBarConfig, StatusBarPosition, StatusSegment};
pub use terminal::{
    default_hyperlink_rules, BroadcastScope, ClipboardReadPolicy, CommandHistoryScope,
    DropPathQuoting, EditorConfig, ExitBehavior, HyperlinkRule, ImageProtocolsConfig,
    KeyboardEncoding, LinkUnderline, ScrollbackExportFormat, TerminalConfig,
};
pub use updates::UpdatesConfig;
pub use window::{
    snap_window_rect, MonitorRect, RestoreSession, ScrollbarMode, SnapEdge, WindowConfig,
    WindowRect, ZenHideElement,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// Errors returned by configuration loading and validation.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A field was outside the accepted range.
    #[error("invalid value for `{field}`: {message}")]
    Invalid {
        /// Dotted path of the offending field.
        field: &'static str,
        /// Human-readable message.
        message: &'static str,
    },
    /// IO error reading or writing the config file.
    #[error("config I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parsing error.
    #[error("config parse error: {0}")]
    Parse(#[from] toml::de::Error),
    /// TOML serialisation error.
    #[error("config serialise error: {0}")]
    Serialise(#[from] toml::ser::Error),
}

/// Root configuration object loaded from `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Font settings.
    pub font: FontConfig,
    /// Window settings.
    pub window: WindowConfig,
    /// Named shell profile settings.
    pub profiles: ProfilesConfig,
    /// Theme settings.
    pub appearance: AppearanceConfig,
    /// Cursor look-and-feel.
    pub cursor: CursorConfig,
    /// User-configurable keybinds (only the ones that aren't hard-coded
    /// app shortcuts).
    pub keybinds: KeybindsConfig,
    /// AI providers + default assistant settings (v2.0).
    pub ai: AiConfig,
    /// Lua plugin loader settings (v2.0).
    pub plugins: PluginsConfig,
    /// Bell notification settings (visual flash, audio beep, both, none).
    pub bell: BellConfig,
    /// Quake-mode drop-down geometry.
    pub quake: QuakeConfig,
    /// External editor used when Ctrl+clicking a `file:line:col` reference.
    pub editor: EditorConfig,
    /// GPU backend selection and software-fallback controls.
    pub gpu: GpuConfig,
    /// Terminal grid behaviour (selection word boundaries, …).
    pub terminal: TerminalConfig,
    /// Tombstone for the removed `[keystroke_fx]` table. Kept so old
    /// `config.toml` files that still contain `[keystroke_fx]` continue to
    /// load without error (Config uses `deny_unknown_fields`). The value is
    /// never read at runtime; the field is never serialised.
    #[serde(rename = "keystroke_fx", default, skip_serializing)]
    #[schemars(skip)]
    _keystroke_fx: Option<toml::Value>,
    /// Animated background "wallpaper" effect (aurora, starfield, matrix,
    /// pixel-CRT) rendered behind the terminal grid. Off by default.
    pub background_fx: BackgroundFxConfig,
    /// Quick-select / label-hint mode: regex patterns, label alphabet, and
    /// keybinds for the overlay. See [`QuickSelectConfig`].
    pub quick_select: QuickSelectConfig,
    /// Configurable status bar: a thin strip at the top or bottom of the
    /// terminal with left- and right-aligned text segments. Off by default.
    pub status_bar: StatusBarConfig,
    /// Configured SSH hosts. Each entry surfaces in the command palette as
    /// `SSH: <host name>` and in the "New SSH tab" picker; selecting one opens a
    /// tab whose I/O is an interactive remote shell. Written in TOML as
    /// repeated `[[ssh_hosts]]` tables.
    #[serde(default)]
    pub ssh_hosts: Vec<SshHost>,
    /// Global SSH connection settings: known-hosts file path and host-key
    /// verification policy.
    #[serde(default)]
    pub ssh: SshConfig,
    /// User-defined text snippets. Each entry is a named body that can be
    /// inserted into the active pane via the snippet picker (opened from the
    /// command palette or a configurable keybind). The body supports the same
    /// escape sequences as the `send:` keybind action (`\n`, `\t`, `\e`, …).
    /// Written in TOML as repeated `[[snippets]]` tables. Default empty.
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    /// Context auto-switch rules. Each entry can tint the tab chip in a colour
    /// and/or show a badge text when the tab's SSH host name or current
    /// working directory matches the configured glob. Written in TOML as
    /// repeated `[[context_rules]]` tables. Default empty (no rules active).
    ///
    /// Rules are evaluated in order; the first match wins. When no rule
    /// matches, any previously-applied colour / badge is cleared.
    #[serde(default)]
    pub context_rules: Vec<ContextRule>,
    /// Clipboard history ring buffer. Retains the last `size` text entries
    /// produced by copy actions so they can be re-pasted via the fuzzy
    /// clipboard-history picker (`OpenClipboardHistory`). Memory-only —
    /// entries are never written to disk.
    pub clipboard_history: ClipboardHistoryConfig,
    /// Directory-jump frecency store: tracks visited directories (via OSC 7
    /// cwd reports), ranks them by frequency + recency, and surfaces a fuzzy
    /// picker (`OpenDirectoryJump`) to jump the active shell to any of them by
    /// sending `cd <path>` to the focused pane. Works with any OSC-7-capable
    /// shell — no third-party tool required.
    pub directory_jump: DirectoryJumpConfig,
    /// Desktop / OS integration. On Linux, controls whether the binary
    /// registers its own `.desktop` application-menu entry on launch.
    pub integration: IntegrationConfig,
    /// Bottom resource-indicator strip (CPU/RAM/GPU, pixel-art).
    pub resource_indicators: ResourceIndicatorsConfig,
    /// Built-in self-updater (check GitHub releases, stage updates safely).
    pub updates: UpdatesConfig,
    /// Diagnostic file logging (rolling daily file next to the config).
    pub logging: LoggingConfig,
}

impl Config {
    /// Validate cross-field invariants and return the first error encountered.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when any field is outside its accepted
    /// range. See the individual section types for details.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.font.validate()?;
        self.window.validate()?;
        self.profiles.validate()?;
        self.cursor.validate()?;
        self.appearance.validate()?;
        self.terminal.validate()?;
        self.background_fx.validate()?;
        self.quick_select.validate()?;
        self.status_bar.validate()?;
        self.ssh.validate()?;
        for snippet in &self.snippets {
            snippet.validate()?;
        }
        for rule in &self.context_rules {
            rule.validate()?;
        }
        self.clipboard_history.validate()?;
        self.directory_jump.validate()?;
        self.ai.validate()?;
        self.integration.validate()?;
        self.resource_indicators.validate()?;
        self.updates.validate()?;
        self.plugins.validate()?;
        self.logging.validate()?;
        Ok(())
    }

    /// Load config from the OS-standard path. If no file exists yet, write
    /// a sensible default first (auto-detected shells) and return it.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on parse failure or filesystem error.
    pub fn load_or_init() -> Result<(Self, std::path::PathBuf), ConfigError> {
        Self::load_or_init_at(None)
    }

    /// Like [`Self::load_or_init`] but honours an explicit `override_path`
    /// (from the `--config` flag / `TERMINALE_CONFIG` env). Falls back to
    /// the standard per-platform config path when `override_path` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on parse failure or filesystem error.
    pub fn load_or_init_at(
        override_path: Option<std::path::PathBuf>,
    ) -> Result<(Self, std::path::PathBuf), ConfigError> {
        let Some(path) = override_path.or_else(paths::config_file) else {
            // No home dir and no override — return a built-in default but
            // don't write.
            return Ok((
                Self::with_auto_profiles(),
                std::path::PathBuf::from("config.toml"),
            ));
        };
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            let mut cfg: Self = toml::from_str(&text)?;
            cfg.validate()?;
            cfg.hydrate_ai_keys();
            Ok((cfg, path))
        } else {
            let cfg = Self::with_auto_profiles();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let body = render_default_toml(&cfg);
            std::fs::write(&path, body)?;
            restrict_config_permissions(&path);
            Ok((cfg, path))
        }
    }

    /// Hydrate the in-memory AI API keys from the OS keychain, migrating any
    /// legacy plaintext values found in `config.toml` into the keychain
    /// (the `skip_serializing` on those fields drops them from the file on
    /// the next save). Call after deserializing a config from disk — both
    /// at startup and on hot-reload, otherwise a reload would wipe the
    /// in-memory keys (the file intentionally no longer contains them).
    pub fn hydrate_ai_keys(&mut self) {
        hydrate_ai_key(&mut self.ai.claude.api_key, secrets::AI_CLAUDE_KEY_ID);
        hydrate_ai_key(&mut self.ai.openai.api_key, secrets::AI_OPENAI_KEY_ID);
        *ai_keys_synced()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((
            self.ai.claude.api_key.clone(),
            self.ai.openai.api_key.clone(),
        ));
    }

    /// Persist the in-memory AI keys to the OS keychain when they changed
    /// since the last hydrate/sync. A key cleared by the user (non-empty →
    /// empty) is deleted from the keychain; an untouched pair is a no-op so
    /// ordinary config saves never pay a keychain round-trip.
    fn sync_ai_keys_to_keychain(&self) {
        let current = (
            self.ai.claude.api_key.clone(),
            self.ai.openai.api_key.clone(),
        );
        let mut cache = ai_keys_synced()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.as_ref() == Some(&current) {
            return;
        }
        let prev = cache.clone();
        sync_ai_key(
            &current.0,
            prev.as_ref().is_some_and(|p| !p.0.is_empty()),
            secrets::AI_CLAUDE_KEY_ID,
        );
        sync_ai_key(
            &current.1,
            prev.as_ref().is_some_and(|p| !p.1.is_empty()),
            secrets::AI_OPENAI_KEY_ID,
        );
        *cache = Some(current);
    }

    /// Construct a fresh `Config` with profiles auto-detected on this host.
    #[must_use]
    pub fn with_auto_profiles() -> Self {
        let detected = auto_detect_profiles();
        let default_profile = detected.first().map(|p| p.name.clone());
        Self {
            profiles: ProfilesConfig {
                default: default_profile,
                profiles: detected,
            },
            ..Self::default()
        }
    }

    /// Resolve the profile picked by `default_profile`, or the first profile
    /// if the named default is missing. Returns `None` only when there are no
    /// profiles at all.
    #[must_use]
    pub fn resolve_default_profile(&self) -> Option<&Profile> {
        if let Some(name) = &self.profiles.default {
            if let Some(p) = self.profiles.profiles.iter().find(|p| &p.name == name) {
                return Some(p);
            }
        }
        self.profiles.profiles.first()
    }

    /// Write a TOML rendering of the current config back to `path`.
    ///
    /// The write is atomic: the text lands in a `.tmp` sibling first and is
    /// then renamed over the target, so a crash or power loss mid-write can
    /// never leave a torn/empty `config.toml` behind (the settings window
    /// saves on a 400 ms debounce, so this runs often).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] or [`ConfigError::Serialise`].
    pub fn write_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = render_default_toml(self);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        restrict_config_permissions(&tmp);
        // Windows refuses to rename over an existing file in some setups;
        // `rename` is atomic on Unix and effectively so on NTFS once the
        // destination is replaceable. Fall back to a direct write only if
        // the rename itself fails (better a rare torn write than no save).
        if let Err(e) = std::fs::rename(&tmp, path) {
            tracing::warn!(
                ?e,
                "atomic config rename failed; falling back to direct write"
            );
            let text = render_default_toml(self);
            std::fs::write(path, text)?;
            let _ = std::fs::remove_file(&tmp);
        }
        restrict_config_permissions(path);
        // Secrets ride along on every save: AI keys live in the OS keychain,
        // never in the TOML (see `hydrate_ai_keys`); no-op when unchanged.
        self.sync_ai_keys_to_keychain();
        Ok(())
    }
}

/// Last AI-key pair synced with the OS keychain in this process —
/// `(claude, openai)`. Lets `write_to` skip keychain round-trips when the
/// keys didn't change (the settings window saves on a 400 ms debounce).
fn ai_keys_synced() -> &'static std::sync::Mutex<Option<(String, String)>> {
    static SYNCED: std::sync::OnceLock<std::sync::Mutex<Option<(String, String)>>> =
        std::sync::OnceLock::new();
    SYNCED.get_or_init(|| std::sync::Mutex::new(None))
}

/// Fill `field` from the keychain when empty; migrate a legacy plaintext
/// value into the keychain when present. Keychain failures degrade to the
/// in-memory/env-var behaviour and are logged, never fatal.
fn hydrate_ai_key(field: &mut String, id: &str) {
    if field.is_empty() {
        match secrets::get_secret(id) {
            Ok(Some(v)) => *field = v,
            Ok(None) => {}
            Err(e) => tracing::warn!(?e, id, "could not read AI key from the OS keychain"),
        }
    } else {
        // Legacy plaintext key found in config.toml — move it to the
        // keychain; `skip_serializing` drops it from the file on next save.
        match secrets::store_secret(id, field) {
            Ok(()) => tracing::info!(id, "migrated plaintext AI key to the OS keychain"),
            Err(e) => {
                tracing::warn!(
                    ?e,
                    id,
                    "could not migrate AI key to keychain; kept in memory"
                );
            }
        }
    }
}

/// Store a (changed) AI key in the keychain, or delete the entry when the
/// user cleared a previously stored key. Empty-and-never-stored is a no-op.
fn sync_ai_key(value: &str, was_stored: bool, id: &str) {
    if !value.is_empty() {
        if let Err(e) = secrets::store_secret(id, value) {
            tracing::warn!(?e, id, "could not store AI key in the OS keychain");
        }
    } else if was_stored {
        if let Err(e) = secrets::delete_secret(id) {
            tracing::warn!(
                ?e,
                id,
                "could not delete cleared AI key from the OS keychain"
            );
        }
    }
}

/// Restrict `config.toml` to owner read/write on Unix (`0600`). The config
/// can reference secrets-adjacent material (workspace commands, hosts) and
/// historically held AI keys — keep it private by default. Best-effort:
/// permission errors are ignored (e.g. exotic filesystems). On Windows the
/// profile directory ACL already scopes access to the user.
fn restrict_config_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perm = meta.permissions();
            perm.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn render_default_toml(cfg: &Config) -> String {
    // Hand-crafted preamble so a first-time user sees the most useful
    // settings up top with comments.
    let mut buf = String::new();
    buf.push_str("# terminale configuration\n");
    buf.push_str("# Edit this file freely — settings reload on the next launch.\n");
    buf.push_str("# Docs: https://github.com/fbrzlarosa/terminale/blob/main/docs/config.md\n\n");
    buf.push_str(&toml::to_string_pretty(cfg).unwrap_or_default());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn rejects_invalid_font_size() {
        let mut cfg = Config::default();
        cfg.font.size = 0.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn default_scrollback_is_10k() {
        assert_eq!(Config::default().window.scrollback_lines, 10_000);
    }

    #[test]
    fn default_always_on_top_is_off() {
        // Stay-on-top must default to disabled so it never surprises users.
        assert!(!Config::default().window.always_on_top);
        // And its quick-toggle shortcut is unbound by default to avoid
        // colliding with existing keybinds.
        assert!(Config::default().keybinds.shortcuts.stay_on_top.is_empty());
    }

    #[test]
    fn parses_always_on_top_and_shortcut() {
        let toml_src = r#"
[window]
always_on_top = true

[keybinds.shortcuts]
stay_on_top = "Ctrl+Shift+Period"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("stay-on-top config must parse");
        cfg.validate().expect("stay-on-top config must validate");
        assert!(cfg.window.always_on_top);
        assert_eq!(
            cfg.keybinds.shortcuts.stay_on_top,
            "Ctrl+Shift+Period".to_string()
        );

        // Round-trip: serialise then re-parse and confirm the flag survives.
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(back.window.always_on_top);
    }

    #[test]
    fn write_to_is_atomic_and_overwrites() {
        let dir = std::env::temp_dir().join("terminale-config-atomic-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        let _ = std::fs::remove_file(&path);

        let cfg = Config::default();
        // First write creates the file; second must replace it via rename
        // (regression: rename-over-existing must work on every OS).
        cfg.write_to(&path).expect("first write");
        cfg.write_to(&path).expect("overwrite");

        let text = std::fs::read_to_string(&path).expect("config readable");
        assert!(text.contains("# terminale configuration"));
        // No staging file may linger after a successful save.
        assert!(
            !dir.join("config.toml.tmp").exists(),
            "tmp staging file must not be left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_excessive_scrollback() {
        let mut cfg = Config::default();
        cfg.window.scrollback_lines = 2_000_000;
        assert!(cfg.validate().is_err());
        // Zero (no scrollback) and the cap itself are both valid.
        cfg.window.scrollback_lines = 0;
        assert!(cfg.validate().is_ok());
        cfg.window.scrollback_lines = 1_000_000;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn readme_example_config_parses() {
        // Mirrors the `## Configuration` example in README.md. Because the
        // config structs use `deny_unknown_fields`, any typo in the README
        // (wrong section, kebab-vs-snake key, bad enum value) fails here —
        // keeping the documented example honest.
        let toml_src = r#"
[font]
family = "JetBrains Mono"
size = 14.0
ligatures = true

[appearance]
theme = "Tokyo Night"

[window]
opacity = 0.97
padding = 8
scrollback_lines = 10000
copy_on_select = false

[cursor]
style = "block"
blink = true
blink_rate_ms = 530

[bell]
mode = "visual"

[ai]
default_provider = "ollama"
render_markdown = true

[keybinds]
quake = "Ctrl+`"

[keybinds.shortcuts]
new_tab = "Ctrl+T"
command_palette = "Ctrl+Shift+P"
ai_assistant = "Ctrl+Shift+I"
explain_selection = "Ctrl+Shift+E"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("README config example must parse");
        cfg.validate().expect("README config example must validate");
        assert_eq!(cfg.appearance.theme, "Tokyo Night");
        assert_eq!(cfg.window.scrollback_lines, 10_000);
        assert_eq!(cfg.font.family, "JetBrains Mono");
        assert_eq!(cfg.keybinds.shortcuts.new_tab, "Ctrl+T");
    }

    #[test]
    fn parses_ssh_hosts_array() {
        let toml_src = r#"
[[ssh_hosts]]
name = "prod"
host = "10.0.0.5"
user = "deploy"

[[ssh_hosts]]
name = "build"
host = "ci.example.com"
port = 2222
user = "runner"
auth = "key"
key_path = "/home/me/.ssh/id_ed25519"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("ssh_hosts must parse");
        cfg.validate().unwrap();
        assert_eq!(cfg.ssh_hosts.len(), 2);
        assert_eq!(cfg.ssh_hosts[0].name, "prod");
        assert_eq!(cfg.ssh_hosts[0].port, 22);
        assert_eq!(cfg.ssh_hosts[0].auth, SshAuthMethod::Agent);
        assert_eq!(cfg.ssh_hosts[1].port, 2222);
        assert_eq!(cfg.ssh_hosts[1].auth, SshAuthMethod::Key);
        assert!(cfg.ssh_hosts[1].key_path.is_some());
    }

    #[test]
    fn default_has_no_ssh_hosts() {
        assert!(Config::default().ssh_hosts.is_empty());
    }

    #[test]
    fn ssh_hosts_survive_toml_roundtrip() {
        let mut cfg = Config::default();
        cfg.ssh_hosts.push(SshHost {
            id: "prod-id".into(),
            name: "prod".into(),
            host: "10.0.0.5".into(),
            port: 22,
            user: "deploy".into(),
            auth: SshAuthMethod::Agent,
            key_path: None,
        });
        cfg.ssh_hosts.push(SshHost {
            id: "build-id".into(),
            name: "build".into(),
            host: "ci.example.com".into(),
            port: 2222,
            user: "runner".into(),
            auth: SshAuthMethod::Key,
            key_path: Some(std::path::PathBuf::from("/home/me/.ssh/id_ed25519")),
        });
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.ssh_hosts.len(), 2);
        assert_eq!(back.ssh_hosts[1].port, 2222);
        assert_eq!(back.ssh_hosts[1].auth, SshAuthMethod::Key);
        assert_eq!(back.ssh_hosts[0].auth, SshAuthMethod::Agent);
    }

    #[test]
    fn toml_roundtrip() {
        let original = Config::default();
        let s = toml::to_string(&original).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.font.family, original.font.family);
        // Defaults are exact bit patterns, so equality is safe here.
        assert!((back.window.opacity - original.window.opacity).abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_unknown_field() {
        let bad = r#"
            [font]
            family = "Foo"
            size = 14.0
            line_height = 1.2
            ligatures = true
            mystery_field = "boom"
        "#;
        let res: Result<Config, _> = toml::from_str(bad);
        assert!(res.is_err());
    }

    #[test]
    fn gpu_defaults_are_auto() {
        let cfg = Config::default();
        assert_eq!(cfg.gpu.backend, GpuBackend::Auto);
        assert_eq!(cfg.gpu.power_preference, GpuPowerPreference::Auto);
        cfg.validate().expect("default gpu config must validate");
    }

    #[test]
    fn gpu_parses_valid_backends() {
        for (s, want) in [
            ("auto", GpuBackend::Auto),
            ("vulkan", GpuBackend::Vulkan),
            ("dx12", GpuBackend::Dx12),
            ("metal", GpuBackend::Metal),
            ("gl", GpuBackend::Gl),
            ("software", GpuBackend::Software),
        ] {
            let toml_src = format!("[gpu]\nbackend = \"{s}\"\npower_preference = \"high\"\n");
            let cfg: Config = toml::from_str(&toml_src).expect("valid gpu backend must parse");
            assert_eq!(cfg.gpu.backend, want);
            assert_eq!(cfg.gpu.power_preference, GpuPowerPreference::High);
        }
    }

    #[test]
    fn default_word_separators_keep_paths_and_idents_joined() {
        let seps = TerminalConfig::default().word_separators;
        // Identifier / path glue chars must NOT be separators.
        for c in ['_', '-', '.', '/'] {
            assert!(
                !seps.contains(c),
                "`{c}` should not be a word separator by default"
            );
        }
        // Common shell punctuation IS a separator.
        for c in ['(', ')', '|', ';', '"'] {
            assert!(seps.contains(c), "`{c}` should be a word separator");
        }
    }

    #[test]
    fn default_link_underline_is_hover() {
        // `hover` is the default so banner URLs printed before any output
        // scrolls don't leave a stray persistent accent line on startup.
        assert_eq!(
            Config::default().terminal.link_underline,
            LinkUnderline::Hover
        );
    }

    #[test]
    fn offer_save_ssh_hosts_defaults_on_and_roundtrips() {
        // Opt-out, not opt-in: the save prompt is offered out of the box.
        assert!(Config::default().terminal.offer_save_ssh_hosts);
        // An older config without the key falls back to the default (true).
        let legacy: Config = toml::from_str("[terminal]\nlink_underline = \"never\"\n").unwrap();
        assert!(legacy.terminal.offer_save_ssh_hosts);
        // Explicitly turning it off parses + validates + roundtrips.
        let off: Config = toml::from_str("[terminal]\noffer_save_ssh_hosts = false\n").unwrap();
        off.validate().expect("config must validate");
        assert!(!off.terminal.offer_save_ssh_hosts);
        let serialized = toml::to_string(&off).unwrap();
        let reparsed: Config = toml::from_str(&serialized).unwrap();
        assert!(!reparsed.terminal.offer_save_ssh_hosts);
    }

    #[test]
    fn parses_all_link_underline_modes() {
        for (raw, want) in [
            ("always", LinkUnderline::Always),
            ("hover", LinkUnderline::Hover),
            ("never", LinkUnderline::Never),
        ] {
            let toml_src = format!("[terminal]\nlink_underline = \"{raw}\"\n");
            let cfg: Config =
                toml::from_str(&toml_src).unwrap_or_else(|e| panic!("`{raw}` must parse: {e}"));
            cfg.validate().expect("link_underline config must validate");
            assert_eq!(cfg.terminal.link_underline, want, "mode `{raw}`");
        }
        // An unknown mode is rejected at parse time.
        let bad: Result<Config, _> = toml::from_str("[terminal]\nlink_underline = \"sometimes\"\n");
        assert!(bad.is_err(), "unknown link_underline must be rejected");
    }

    #[test]
    fn gpu_rejects_invalid_backend() {
        let bad = "[gpu]\nbackend = \"opengl_es_2\"\n";
        let res: Result<Config, _> = toml::from_str(bad);
        assert!(res.is_err(), "unknown gpu backend must be rejected");
    }

    #[test]
    fn gpu_rejects_invalid_power_preference() {
        let bad = "[gpu]\npower_preference = \"turbo\"\n";
        let res: Result<Config, _> = toml::from_str(bad);
        assert!(res.is_err(), "unknown power preference must be rejected");
    }

    #[test]
    fn snap_top_is_full_width_top_half() {
        // 1920x1080 monitor at the origin; a 800x600 window snaps to the top.
        let rect = snap_window_rect((0, 0, 1920, 1080), SnapEdge::Top, (10, 20, 800, 600));
        assert_eq!(rect, (0, 0, 1920, 540));
    }

    #[test]
    fn snap_bottom_pins_to_bottom_half() {
        let rect = snap_window_rect((0, 0, 1920, 1080), SnapEdge::Bottom, (0, 0, 800, 600));
        // Bottom half: y = monitor mid, height = remaining (handles odd splits).
        assert_eq!(rect, (0, 540, 1920, 540));
    }

    #[test]
    fn snap_left_right_are_full_height_halves() {
        let left = snap_window_rect((0, 0, 1000, 1000), SnapEdge::Left, (0, 0, 300, 300));
        assert_eq!(left, (0, 0, 500, 1000));
        let right = snap_window_rect((0, 0, 1000, 1000), SnapEdge::Right, (0, 0, 300, 300));
        assert_eq!(right, (500, 0, 500, 1000));
    }

    #[test]
    fn snap_center_keeps_size_and_centres() {
        // 600x600 window centred on a 1000x1000 monitor -> 200px slack each.
        let rect = snap_window_rect((0, 0, 1000, 1000), SnapEdge::Center, (33, 44, 600, 600));
        assert_eq!(rect, (200, 200, 600, 600));
    }

    #[test]
    fn snap_center_clamps_oversized_window_to_monitor() {
        // A window bigger than the monitor is clamped down, then centred (0,0).
        let rect = snap_window_rect((0, 0, 800, 600), SnapEdge::Center, (0, 0, 5000, 5000));
        assert_eq!(rect, (0, 0, 800, 600));
    }

    #[test]
    fn snap_maximize_fills_the_monitor() {
        let rect = snap_window_rect((0, 0, 1920, 1080), SnapEdge::Maximize, (10, 10, 400, 300));
        assert_eq!(rect, (0, 0, 1920, 1080));
    }

    #[test]
    fn snap_respects_monitor_origin() {
        // Secondary monitor offset to the right of the primary.
        let rect = snap_window_rect((1920, 0, 1280, 1024), SnapEdge::Top, (0, 0, 400, 300));
        assert_eq!(rect, (1920, 0, 1280, 512));
        let max = snap_window_rect((1920, 0, 1280, 1024), SnapEdge::Maximize, (0, 0, 400, 300));
        assert_eq!(max, (1920, 0, 1280, 1024));
    }

    #[test]
    fn quake_config_defaults() {
        let q = QuakeConfig::default();
        assert_eq!(q.animation, QuakeAnimation::Slide);
        assert_eq!(q.animation_ms, 120);
    }

    #[test]
    fn quake_animation_parses_and_defaults() {
        // Explicit override parses.
        let cfg: Config =
            toml::from_str("[quake]\nanimation = \"bounce\"\nanimation_ms = 200\n").unwrap();
        assert_eq!(cfg.quake.animation, QuakeAnimation::Bounce);
        assert_eq!(cfg.quake.animation_ms, 200);
        // Missing fields fall back to the slide default.
        let cfg2: Config = toml::from_str("[quake]\n").unwrap();
        assert_eq!(cfg2.quake.animation, QuakeAnimation::Slide);
        assert_eq!(cfg2.quake.animation_ms, 120);
    }

    #[test]
    fn quake_dock_rect_top_uses_full_width_and_size_percent_height() {
        let mon: MonitorRect = (0, 0, 1920, 1080);
        let r = quake_dock_rect(mon, QuakeEdge::Top, 0.5, 0).unwrap();
        assert_eq!(r, (0, 0, 1920, 540));
    }

    #[test]
    fn quake_dock_rect_bottom_anchors_to_bottom_edge() {
        let mon: MonitorRect = (0, 0, 1920, 1080);
        let r = quake_dock_rect(mon, QuakeEdge::Bottom, 0.3, 0).unwrap();
        // height = 324, y = 1080 - 324 = 756
        assert_eq!(r, (0, 756, 1920, 324));
    }

    #[test]
    fn quake_dock_rect_left_uses_size_percent_width() {
        let mon: MonitorRect = (100, 50, 1920, 1080);
        let r = quake_dock_rect(mon, QuakeEdge::Left, 0.25, 0).unwrap();
        // width = 480, x stays at mon.x = 100, y = mon.y = 50
        assert_eq!(r, (100, 50, 480, 1080));
    }

    #[test]
    fn quake_dock_rect_right_anchors_to_right_edge() {
        let mon: MonitorRect = (100, 50, 1920, 1080);
        let r = quake_dock_rect(mon, QuakeEdge::Right, 0.25, 0).unwrap();
        // width = 480, x = 100 + 1920 - 480 = 1540
        assert_eq!(r, (1540, 50, 480, 1080));
    }

    #[test]
    fn quake_dock_rect_margin_inset_top() {
        let mon: MonitorRect = (0, 0, 1920, 1080);
        let r = quake_dock_rect(mon, QuakeEdge::Top, 0.5, 24).unwrap();
        assert_eq!(r, (0, 24, 1920, 540));
    }

    #[test]
    fn quake_dock_rect_clamps_size_percent_low() {
        let mon: MonitorRect = (0, 0, 1000, 1000);
        // 0.01 should clamp to 0.1.
        let r = quake_dock_rect(mon, QuakeEdge::Top, 0.01, 0).unwrap();
        assert_eq!(r.3, 100);
    }

    #[test]
    fn quake_dock_rect_off_returns_none() {
        let mon: MonitorRect = (0, 0, 1920, 1080);
        assert!(quake_dock_rect(mon, QuakeEdge::Off, 0.5, 0).is_none());
    }

    #[test]
    fn quake_tolerates_obsolete_pre_rework_fields() {
        // Pre-rework configs had top/height/width knobs baked into Quake.
        // Those fields are gone, but a user upgrading must still get their
        // config loaded — falling back to defaults would silently lose
        // every other section (profiles, theme, keybinds, …).
        let toml_src = r#"
[quake]
animation = "fade"
animation_ms = 250
height_ratio = 0.5
width_ratio = 0.9
top_offset_px = 24
"#;
        // `fade` is a REAL variant again as of 0.1.12 (Windows layered-window
        // opacity animation) — a pre-rework `fade` config now gets the actual
        // fade instead of silently degrading to Slide.
        let cfg: Config = toml::from_str(toml_src).expect("obsolete fields must be tolerated");
        assert_eq!(cfg.quake.animation, QuakeAnimation::Fade);
        assert_eq!(cfg.quake.animation_ms, 250);
    }

    #[test]
    fn profiles_batch_parses() {
        let toml_src = r#"
[window]
confirm_close = true

[appearance]
theme = "Tokyo Night"
tab_min_width = 70.0
tab_max_width = 300.0

[terminal]
word_separators = " ()[]"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("profiles-config batch must parse");
        cfg.validate().expect("profiles-config batch must validate");
        assert!(cfg.window.confirm_close);
        assert!((cfg.appearance.tab_min_width - 70.0).abs() < f32::EPSILON);
        assert!((cfg.appearance.tab_max_width - 300.0).abs() < f32::EPSILON);
        assert_eq!(cfg.terminal.word_separators, " ()[]");
    }

    #[test]
    fn rejects_inverted_tab_widths() {
        let mut cfg = Config::default();
        cfg.appearance.tab_min_width = 200.0;
        cfg.appearance.tab_max_width = 100.0;
        assert!(cfg.validate().is_err());
        // Equal bounds are fine.
        cfg.appearance.tab_max_width = 200.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn rejects_out_of_range_tab_width() {
        let mut cfg = Config::default();
        cfg.appearance.tab_min_width = 0.0;
        assert!(cfg.validate().is_err());
        cfg.appearance.tab_min_width = 90.0;
        cfg.appearance.tab_max_width = 5000.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn confirm_close_defaults_off() {
        assert!(!Config::default().window.confirm_close);
    }

    #[test]
    fn animated_tab_drag_defaults_on() {
        assert!(Config::default().appearance.animated_tab_drag);
    }

    #[test]
    fn divider_defaults_are_sane() {
        let cfg = Config::default();
        assert!((cfg.appearance.divider_thickness_logical - 4.0).abs() < f32::EPSILON);
        assert!((cfg.appearance.divider_grab_padding_logical - 3.0).abs() < f32::EPSILON);
        assert!(cfg.appearance.divider_color.is_none());
        assert!(cfg.terminal.live_pane_resize);
        cfg.validate().expect("divider defaults must validate");
    }

    #[test]
    fn divider_thickness_range_validates() {
        let mut cfg = Config::default();
        cfg.appearance.divider_thickness_logical = 0.5;
        assert!(cfg.validate().is_err());
        cfg.appearance.divider_thickness_logical = 100.0;
        assert!(cfg.validate().is_err());
        cfg.appearance.divider_thickness_logical = 1.0;
        assert!(cfg.validate().is_ok());
        cfg.appearance.divider_thickness_logical = 12.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn divider_grab_padding_range_validates() {
        let mut cfg = Config::default();
        cfg.appearance.divider_grab_padding_logical = -1.0;
        assert!(cfg.validate().is_err());
        cfg.appearance.divider_grab_padding_logical = 21.0;
        assert!(cfg.validate().is_err());
        cfg.appearance.divider_grab_padding_logical = 0.0;
        assert!(cfg.validate().is_ok());
        cfg.appearance.divider_grab_padding_logical = 20.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn divider_color_roundtrips() {
        let toml_src = r#"
[appearance]
divider_color = [120, 130, 140]
"#;
        let cfg: Config = toml::from_str(toml_src).expect("divider_color must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.appearance.divider_color, Some([120, 130, 140]));
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.appearance.divider_color, Some([120, 130, 140]));
    }

    #[test]
    fn focus_border_defaults_are_sane() {
        let cfg = Config::default();
        assert!(
            (cfg.appearance.focus_border_thickness_logical - 2.0).abs() < f32::EPSILON,
            "focus_border_thickness_logical default must be 2.0"
        );
        assert!(
            cfg.appearance.focus_border_color.is_none(),
            "focus_border_color default must be None (auto)"
        );
        cfg.validate().expect("focus border defaults must validate");
    }

    #[test]
    fn focus_border_thickness_range_validates() {
        let mut cfg = Config::default();
        cfg.appearance.focus_border_thickness_logical = -1.0;
        assert!(cfg.validate().is_err());
        cfg.appearance.focus_border_thickness_logical = 9.0;
        assert!(cfg.validate().is_err());
        cfg.appearance.focus_border_thickness_logical = 0.0;
        assert!(cfg.validate().is_ok(), "0 (disabled) must validate");
        cfg.appearance.focus_border_thickness_logical = 8.0;
        assert!(cfg.validate().is_ok(), "max value must validate");
    }

    #[test]
    fn focus_border_color_roundtrips() {
        let toml_src = r#"
[appearance]
focus_border_color = [125, 166, 255]
"#;
        let cfg: Config = toml::from_str(toml_src).expect("focus_border_color must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.appearance.focus_border_color, Some([125, 166, 255]));
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.appearance.focus_border_color, Some([125, 166, 255]));
    }

    #[test]
    fn live_pane_resize_parses_and_roundtrips() {
        let toml_src = "[terminal]\nlive_pane_resize = false\n";
        let cfg: Config = toml::from_str(toml_src).expect("must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.terminal.live_pane_resize);
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(!back.terminal.live_pane_resize);
    }

    #[test]
    fn show_pane_headers_default_and_roundtrip() {
        // Default is `true`.
        let cfg = Config::default();
        assert!(cfg.appearance.show_pane_headers, "default must be true");

        // Explicit `false` parses.
        let toml_src = "[appearance]\nshow_pane_headers = false\n";
        let parsed: Config = toml::from_str(toml_src).expect("must parse");
        assert!(
            !parsed.appearance.show_pane_headers,
            "parsed value must be false"
        );
        parsed.validate().expect("must validate");

        // Round-trip.
        let s = toml::to_string(&parsed).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(
            !back.appearance.show_pane_headers,
            "roundtrip must be false"
        );
    }

    #[test]
    fn animated_tab_drag_parses() {
        let toml_src = r#"
[appearance]
animated_tab_drag = false
"#;
        let cfg: Config = toml::from_str(toml_src).expect("must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.appearance.animated_tab_drag);
    }

    #[test]
    fn load_or_init_at_honours_override_path() {
        // Unique dir per process so parallel test runs don't collide.
        let dir = std::env::temp_dir().join(format!("terminale_cfg_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("config.toml");

        // Missing file at the override path is created with defaults, and the
        // returned path is exactly the override (not the platform default).
        let (cfg, returned) = Config::load_or_init_at(Some(path.clone())).unwrap();
        assert_eq!(returned, path);
        assert!(path.exists(), "override path should have been initialised");
        assert_eq!(cfg.appearance.theme, AppearanceConfig::default().theme);

        // A hand-written override is read back verbatim.
        std::fs::write(&path, "[appearance]\ntheme = \"Dracula\"\nthemes = []\n").unwrap();
        let (cfg2, _) = Config::load_or_init_at(Some(path.clone())).unwrap();
        assert_eq!(cfg2.appearance.theme, "Dracula");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn background_fx_default_disabled_and_validates() {
        assert!(!Config::default().background_fx.enabled);
        BackgroundFxConfig::default()
            .validate()
            .expect("default must validate");
    }

    #[test]
    fn background_fx_intensity_and_speed_bounds() {
        let mut cfg = Config::default();
        cfg.background_fx.intensity = 1.5;
        assert!(cfg.validate().is_err(), "intensity > 1.0 must be rejected");
        cfg.background_fx.intensity = 0.5;
        cfg.background_fx.speed = 0.0;
        assert!(cfg.validate().is_err(), "speed < 0.1 must be rejected");
        cfg.background_fx.speed = 6.0;
        assert!(cfg.validate().is_err(), "speed > 5.0 must be rejected");
    }

    #[test]
    fn background_fx_style_modes_are_stable_and_unique() {
        // The shader relies on these indices; keep them pinned.
        assert_eq!(BackgroundFxStyle::None.shader_mode(), 0);
        assert_eq!(BackgroundFxStyle::AuroraPlasma.shader_mode(), 1);
        assert_eq!(BackgroundFxStyle::Starfield.shader_mode(), 2);
        assert_eq!(BackgroundFxStyle::Matrix.shader_mode(), 3);
        assert_eq!(BackgroundFxStyle::PixelCrt.shader_mode(), 4);
        assert_eq!(BackgroundFxStyle::all().len(), 5);
    }

    #[test]
    fn background_fx_roundtrips() {
        let mut cfg = Config::default();
        cfg.background_fx.enabled = true;
        cfg.background_fx.style = BackgroundFxStyle::Matrix;
        cfg.background_fx.intensity = 0.6;
        cfg.background_fx.speed = 2.0;
        cfg.background_fx.color1 = Some([10, 250, 80]);
        let toml_src = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&toml_src).expect("deserialize");
        assert!(back.background_fx.enabled);
        assert_eq!(back.background_fx.style, BackgroundFxStyle::Matrix);
        assert!((back.background_fx.intensity - 0.6).abs() < f32::EPSILON);
        assert!((back.background_fx.speed - 2.0).abs() < f32::EPSILON);
        assert_eq!(back.background_fx.color1, Some([10, 250, 80]));
    }

    #[test]
    fn quake_animation_all_returns_five() {
        assert_eq!(QuakeAnimation::all().len(), 5);
    }

    #[test]
    fn quake_animation_labels_unique() {
        let labels: Vec<_> = QuakeAnimation::all().iter().map(|a| a.label()).collect();
        let mut seen = std::collections::HashSet::new();
        for l in &labels {
            assert!(seen.insert(*l), "duplicate label: {l}");
        }
    }

    #[test]
    fn quake_animation_current_variants_parse() {
        for (raw, want) in [
            ("none", QuakeAnimation::None),
            ("slide", QuakeAnimation::Slide),
            ("bounce", QuakeAnimation::Bounce),
            ("scale", QuakeAnimation::Scale),
            ("fade", QuakeAnimation::Fade),
        ] {
            let toml_src = format!("[quake]\nanimation = \"{raw}\"\n");
            let cfg: Config = toml::from_str(&toml_src)
                .unwrap_or_else(|e| panic!("animation `{raw}` must parse: {e}"));
            assert_eq!(cfg.quake.animation, want, "animation `{raw}`");
        }
    }

    #[test]
    fn quake_animation_legacy_variants_map_to_slide() {
        // Removed overlay variants (zoom, pixel_dissolve, glitch,
        // scanline_wipe) must still parse without error — they map to Slide
        // for backward compatibility with existing user configs. (`fade` is
        // a REAL variant again as of 0.1.12 — covered by the parse test
        // above, not here.)
        for raw in ["zoom", "pixel_dissolve", "glitch", "scanline_wipe"] {
            let toml_src = format!("[quake]\nanimation = \"{raw}\"\n");
            let cfg: Config = toml::from_str(&toml_src)
                .unwrap_or_else(|e| panic!("legacy animation `{raw}` must parse: {e}"));
            assert_eq!(
                cfg.quake.animation,
                QuakeAnimation::Slide,
                "legacy `{raw}` must map to Slide"
            );
        }
    }

    #[test]
    fn quake_animation_roundtrip() {
        // Every current variant must survive a toml serialize → deserialize
        // round-trip with no data loss.
        for anim in QuakeAnimation::all() {
            let mut cfg = Config::default();
            cfg.quake.animation = anim;
            let s = toml::to_string(&cfg).unwrap();
            let back: Config = toml::from_str(&s).unwrap();
            assert_eq!(
                back.quake.animation, anim,
                "QuakeAnimation::{anim:?} must roundtrip"
            );
        }
    }

    /// Legacy `[quake] stay_on_top` must parse without error. The field was
    /// removed from `QuakeConfig` (superseded by `window.always_on_top`);
    /// `QuakeConfig` uses `#[serde(default)]` without `deny_unknown_fields`,
    /// so stale keys in the `[quake]` table are silently ignored.
    #[test]
    fn quake_stay_on_top_field_is_tolerated_but_ignored() {
        let toml_src = r#"
[quake]
stay_on_top = true

[window]
always_on_top = false
"#;
        let cfg: Config =
            toml::from_str(toml_src).expect("legacy quake.stay_on_top must parse cleanly");
        cfg.validate()
            .expect("legacy quake.stay_on_top config must validate");
        // The window-level flag must NOT be affected by the removed quake field.
        assert!(
            !cfg.window.always_on_top,
            "window.always_on_top must remain false; quake.stay_on_top is removed"
        );
    }

    // ── underline_thickness_px ──────────────────────────────────────────────

    #[test]
    fn underline_thickness_defaults_to_one() {
        let cfg = Config::default();
        assert!(
            (cfg.font.underline_thickness_px - 1.0).abs() < f32::EPSILON,
            "underline_thickness_px default must be 1.0"
        );
        cfg.validate().expect("default config must validate");
    }

    #[test]
    fn underline_thickness_bounds_validate() {
        let mut cfg = Config::default();
        // Too thin.
        cfg.font.underline_thickness_px = 0.1;
        assert!(cfg.validate().is_err(), "0.1 is below the 0.5 minimum");
        // Exactly at the minimum.
        cfg.font.underline_thickness_px = 0.5;
        assert!(
            cfg.validate().is_ok(),
            "0.5 is the minimum and must validate"
        );
        // Exactly at the maximum.
        cfg.font.underline_thickness_px = 4.0;
        assert!(
            cfg.validate().is_ok(),
            "4.0 is the maximum and must validate"
        );
        // Too thick.
        cfg.font.underline_thickness_px = 5.0;
        assert!(cfg.validate().is_err(), "5.0 is above the 4.0 maximum");
    }

    #[test]
    fn pane_keyboard_shortcuts_default_values() {
        let sc = ShortcutsConfig::default();
        // Focus navigation is unbound by default (no collision risk).
        assert!(sc.focus_pane_left.is_empty());
        assert!(sc.focus_pane_right.is_empty());
        assert!(sc.focus_pane_up.is_empty());
        assert!(sc.focus_pane_down.is_empty());
        // Zoom defaults to Ctrl+Shift+Z.
        assert_eq!(sc.toggle_pane_zoom, "Ctrl+Shift+Z");
        // Resize is unbound by default.
        assert!(sc.resize_pane_left.is_empty());
        assert!(sc.resize_pane_right.is_empty());
        assert!(sc.resize_pane_up.is_empty());
        assert!(sc.resize_pane_down.is_empty());
    }

    #[test]
    fn pane_keyboard_shortcuts_roundtrip() {
        let toml_src = r#"
[keybinds.shortcuts]
focus_pane_left  = "Alt+ArrowLeft"
focus_pane_right = "Alt+ArrowRight"
focus_pane_up    = "Alt+ArrowUp"
focus_pane_down  = "Alt+ArrowDown"
toggle_pane_zoom = "Ctrl+Shift+Z"
resize_pane_left  = "Ctrl+Alt+ArrowLeft"
resize_pane_right = "Ctrl+Alt+ArrowRight"
resize_pane_up    = "Ctrl+Alt+ArrowUp"
resize_pane_down  = "Ctrl+Alt+ArrowDown"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("pane keyboard shortcuts must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.keybinds.shortcuts.focus_pane_left, "Alt+ArrowLeft");
        assert_eq!(cfg.keybinds.shortcuts.focus_pane_right, "Alt+ArrowRight");
        assert_eq!(cfg.keybinds.shortcuts.focus_pane_up, "Alt+ArrowUp");
        assert_eq!(cfg.keybinds.shortcuts.focus_pane_down, "Alt+ArrowDown");
        assert_eq!(cfg.keybinds.shortcuts.toggle_pane_zoom, "Ctrl+Shift+Z");
        assert_eq!(
            cfg.keybinds.shortcuts.resize_pane_left,
            "Ctrl+Alt+ArrowLeft"
        );
        assert_eq!(
            cfg.keybinds.shortcuts.resize_pane_right,
            "Ctrl+Alt+ArrowRight"
        );
        assert_eq!(cfg.keybinds.shortcuts.resize_pane_up, "Ctrl+Alt+ArrowUp");
        assert_eq!(
            cfg.keybinds.shortcuts.resize_pane_down,
            "Ctrl+Alt+ArrowDown"
        );
        // Serialise + re-parse (TOML roundtrip).
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        back.validate().expect("roundtripped config must validate");
        assert_eq!(back.keybinds.shortcuts.focus_pane_left, "Alt+ArrowLeft");
        assert_eq!(back.keybinds.shortcuts.toggle_pane_zoom, "Ctrl+Shift+Z");
    }

    #[test]
    fn pane_resize_step_cells_default_and_bounds() {
        // Default must be 2.
        assert_eq!(Config::default().terminal.pane_resize_step_cells, 2);
        Config::default()
            .validate()
            .expect("default config must validate");

        // Out-of-range values are rejected.
        let mut cfg = Config::default();
        cfg.terminal.pane_resize_step_cells = 0;
        assert!(cfg.validate().is_err(), "0 is below the minimum 1");
        cfg.terminal.pane_resize_step_cells = 21;
        assert!(cfg.validate().is_err(), "21 exceeds the maximum 20");
        // Boundary values pass.
        cfg.terminal.pane_resize_step_cells = 1;
        assert!(cfg.validate().is_ok(), "minimum value 1 must validate");
        cfg.terminal.pane_resize_step_cells = 20;
        assert!(cfg.validate().is_ok(), "maximum value 20 must validate");
    }

    #[test]
    fn pane_resize_step_cells_roundtrips() {
        let toml_src = "[terminal]\npane_resize_step_cells = 5\n";
        let cfg: Config = toml::from_str(toml_src).expect("pane_resize_step_cells must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.terminal.pane_resize_step_cells, 5);
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.terminal.pane_resize_step_cells, 5);
    }

    #[test]
    fn underline_thickness_roundtrips() {
        let toml_src = "[font]\nunderline_thickness_px = 2.5\n";
        let cfg: Config = toml::from_str(toml_src).expect("underline_thickness_px must parse");
        cfg.validate().expect("must validate");
        assert!(
            (cfg.font.underline_thickness_px - 2.5).abs() < f32::EPSILON,
            "parsed value must match"
        );
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(
            (back.font.underline_thickness_px - 2.5).abs() < f32::EPSILON,
            "roundtrip must preserve value"
        );
    }

    // ── tab-nav shortcuts (activate_tab_1..9, last_tab) ──────────────────────

    #[test]
    fn activate_tab_shortcuts_default_to_ctrl_digits() {
        let sc = ShortcutsConfig::default();
        assert_eq!(sc.activate_tab_1, "Ctrl+1");
        assert_eq!(sc.activate_tab_2, "Ctrl+2");
        assert_eq!(sc.activate_tab_3, "Ctrl+3");
        assert_eq!(sc.activate_tab_4, "Ctrl+4");
        assert_eq!(sc.activate_tab_5, "Ctrl+5");
        assert_eq!(sc.activate_tab_6, "Ctrl+6");
        assert_eq!(sc.activate_tab_7, "Ctrl+7");
        assert_eq!(sc.activate_tab_8, "Ctrl+8");
        assert_eq!(sc.activate_tab_9, "Ctrl+9");
    }

    #[test]
    fn last_tab_shortcut_is_unbound_by_default() {
        // last_tab starts empty so it never collides with existing binds.
        assert!(ShortcutsConfig::default().last_tab.is_empty());
    }

    #[test]
    fn activate_tab_shortcuts_roundtrip() {
        let toml_src = r#"
[keybinds.shortcuts]
activate_tab_1 = "Alt+1"
activate_tab_9 = "Alt+9"
last_tab       = "Ctrl+Shift+Q"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("activate_tab shortcuts must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.keybinds.shortcuts.activate_tab_1, "Alt+1");
        assert_eq!(cfg.keybinds.shortcuts.activate_tab_9, "Alt+9");
        assert_eq!(cfg.keybinds.shortcuts.last_tab, "Ctrl+Shift+Q");
        // TOML roundtrip.
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        back.validate().expect("roundtripped config must validate");
        assert_eq!(back.keybinds.shortcuts.activate_tab_1, "Alt+1");
        assert_eq!(back.keybinds.shortcuts.last_tab, "Ctrl+Shift+Q");
    }

    // ── os_notifications ────────────────────────────────────────────────────

    /// `os_notifications` must default to `true` (opt-in, not opt-out).
    #[test]
    fn os_notifications_default_is_true() {
        assert!(
            Config::default().terminal.os_notifications,
            "os_notifications must default to true"
        );
    }

    /// Explicitly disabling `os_notifications` in TOML must parse, validate,
    /// and survive a TOML round-trip.
    #[test]
    fn os_notifications_roundtrip() {
        let toml_src = "[terminal]\nos_notifications = false\n";
        let cfg: Config = toml::from_str(toml_src).expect("os_notifications must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.terminal.os_notifications);
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(!back.terminal.os_notifications);
    }

    /// A legacy config without the `os_notifications` key must fall back
    /// to the default of `true` (forward-compatibility).
    #[test]
    fn os_notifications_absent_key_defaults_to_true() {
        let toml_src = "[terminal]\nword_separators = \"()[]{}\"";
        let cfg: Config = toml::from_str(toml_src).expect("must parse");
        assert!(
            cfg.terminal.os_notifications,
            "absent os_notifications key must fall back to true"
        );
    }

    // ── status_bar ──────────────────────────────────────────────────────────

    #[test]
    fn status_bar_default_disabled() {
        let cfg = Config::default();
        assert!(
            !cfg.status_bar.enabled,
            "status_bar must default to disabled"
        );
        cfg.validate()
            .expect("default status_bar config must validate");
    }

    #[test]
    fn status_bar_default_position_is_bottom() {
        assert_eq!(
            Config::default().status_bar.position,
            StatusBarPosition::Bottom
        );
    }

    #[test]
    fn status_bar_update_interval_bounds() {
        let mut cfg = Config::default();
        cfg.status_bar.update_interval_ms = 100; // below 200
        assert!(cfg.validate().is_err(), "100 ms should be rejected");
        cfg.status_bar.update_interval_ms = 70000; // above 60000
        assert!(cfg.validate().is_err(), "70000 ms should be rejected");
        cfg.status_bar.update_interval_ms = 200;
        assert!(cfg.validate().is_ok(), "200 ms should be accepted");
        cfg.status_bar.update_interval_ms = 60000;
        assert!(cfg.validate().is_ok(), "60000 ms should be accepted");
    }

    #[test]
    fn status_bar_segments_roundtrip() {
        let mut cfg = Config::default();
        cfg.status_bar.enabled = true;
        cfg.status_bar.position = StatusBarPosition::Top;
        cfg.status_bar.left_segments = vec![
            StatusSegment::Cwd,
            StatusSegment::Literal { text: " | ".into() },
            StatusSegment::TabIndex,
        ];
        cfg.status_bar.right_segments = vec![
            StatusSegment::Profile,
            StatusSegment::UserVar {
                name: "git_branch".into(),
            },
            StatusSegment::Clock {
                format: "%H:%M:%S".into(),
            },
        ];
        cfg.validate().expect("must validate");
        let s = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&s).expect("deserialize");
        back.validate().expect("roundtripped must validate");
        assert!(back.status_bar.enabled);
        assert_eq!(back.status_bar.position, StatusBarPosition::Top);
        assert_eq!(back.status_bar.left_segments.len(), 3);
        assert_eq!(back.status_bar.right_segments.len(), 3);
        // Spot-check one variant in each side.
        assert_eq!(back.status_bar.left_segments[0], StatusSegment::Cwd);
        assert!(
            matches!(&back.status_bar.right_segments[2], StatusSegment::Clock { format } if format == "%H:%M:%S")
        );
    }

    #[test]
    fn status_bar_has_time_segment_clock_only() {
        let mut cfg = StatusBarConfig::default();
        // Default right side has a Clock — so has_time_segment must be true.
        assert!(
            cfg.has_time_segment(),
            "default config with Clock must report has_time_segment"
        );
        // Removing Clock from both sides must flip to false.
        cfg.right_segments = vec![StatusSegment::Profile];
        cfg.left_segments = vec![StatusSegment::Cwd];
        assert!(
            !cfg.has_time_segment(),
            "no Clock anywhere must report false"
        );
    }

    #[test]
    fn status_bar_spacer_roundtrips() {
        let mut cfg = Config::default();
        cfg.status_bar.left_segments = vec![StatusSegment::Cwd, StatusSegment::Spacer];
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.status_bar.left_segments[1], StatusSegment::Spacer);
    }

    // ── auto_reload_config ──────────────────────────────────────────────────

    /// `auto_reload_config` must default to `true` so the feature works out
    /// of the box without any user action.
    #[test]
    fn auto_reload_config_default_is_true() {
        assert!(
            Config::default().window.auto_reload_config,
            "auto_reload_config must default to true"
        );
    }

    /// Disabling `auto_reload_config` in TOML must parse, validate, and
    /// survive a round-trip.
    #[test]
    fn auto_reload_config_roundtrip() {
        let toml_src = "[window]\nauto_reload_config = false\n";
        let cfg: Config = toml::from_str(toml_src).expect("auto_reload_config must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.window.auto_reload_config);
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(
            !back.window.auto_reload_config,
            "roundtrip must preserve false"
        );
    }

    /// A config that does not mention `auto_reload_config` must fall back to
    /// `true` (the default) — forward-compatibility.
    #[test]
    fn auto_reload_config_absent_key_defaults_to_true() {
        let toml_src = "[window]\nopacity = 0.9\n";
        let cfg: Config = toml::from_str(toml_src).expect("must parse");
        assert!(
            cfg.window.auto_reload_config,
            "absent auto_reload_config must fall back to true"
        );
    }

    // ── reload_config shortcut ──────────────────────────────────────────────

    /// `reload_config` shortcut must default to empty (unbound) to avoid
    /// colliding with any existing binding.
    #[test]
    fn reload_config_shortcut_default_is_unbound() {
        assert!(
            ShortcutsConfig::default().reload_config.is_empty(),
            "reload_config shortcut must be unbound by default"
        );
    }

    /// Setting a `reload_config` binding in TOML must parse, validate, and
    /// survive a TOML round-trip.
    #[test]
    fn reload_config_shortcut_roundtrip() {
        let toml_src = "[keybinds.shortcuts]\nreload_config = \"Ctrl+Shift+F5\"\n";
        let cfg: Config = toml::from_str(toml_src).expect("reload_config shortcut must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.keybinds.shortcuts.reload_config, "Ctrl+Shift+F5");
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        back.validate()
            .expect("roundtripped reload_config config must validate");
        assert_eq!(
            back.keybinds.shortcuts.reload_config, "Ctrl+Shift+F5",
            "reload_config binding must survive TOML roundtrip"
        );
    }

    // ── background_image ────────────────────────────────────────────────────

    #[test]
    fn background_image_defaults_are_disabled_and_valid() {
        let cfg = Config::default();
        assert!(
            cfg.appearance.background_image.path.is_none(),
            "background_image must default to disabled"
        );
        cfg.validate()
            .expect("default background_image config must validate");
    }

    #[test]
    fn background_image_all_fits_roundtrip() {
        for (raw, want) in [
            ("fill", BgImageFit::Fill),
            ("fit", BgImageFit::Fit),
            ("stretch", BgImageFit::Stretch),
            ("center", BgImageFit::Center),
            ("tile", BgImageFit::Tile),
        ] {
            let toml_src = format!(
                "[appearance.background_image]\npath = \"/tmp/test.png\"\nfit = \"{raw}\"\n"
            );
            let cfg: Config =
                toml::from_str(&toml_src).unwrap_or_else(|e| panic!("`{raw}` must parse: {e}"));
            cfg.validate().expect("must validate");
            assert_eq!(cfg.appearance.background_image.fit, want, "fit `{raw}`");
        }
    }

    #[test]
    fn background_image_param_roundtrip() {
        let mut cfg = Config::default();
        cfg.appearance.background_image.path = Some("/home/user/wallpaper.png".into());
        cfg.appearance.background_image.opacity = 0.7;
        cfg.appearance.background_image.fit = BgImageFit::Fit;
        cfg.appearance.background_image.brightness = 0.8;
        cfg.appearance.background_image.saturation = 1.2;
        cfg.appearance.background_image.hue = 45.0;
        cfg.validate().expect("must validate");
        let s = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&s).expect("deserialize");
        back.validate().expect("roundtripped must validate");
        assert_eq!(
            back.appearance.background_image.path,
            Some("/home/user/wallpaper.png".into())
        );
        assert!(
            (back.appearance.background_image.opacity - 0.7).abs() < f32::EPSILON,
            "opacity"
        );
        assert_eq!(back.appearance.background_image.fit, BgImageFit::Fit);
        assert!((back.appearance.background_image.brightness - 0.8).abs() < f32::EPSILON);
        assert!((back.appearance.background_image.saturation - 1.2).abs() < f32::EPSILON);
        assert!((back.appearance.background_image.hue - 45.0).abs() < f32::EPSILON);
    }

    #[test]
    fn background_image_opacity_bounds() {
        let mut cfg = Config::default();
        cfg.appearance.background_image.path = Some("/x.png".into());
        cfg.appearance.background_image.opacity = -0.1;
        assert!(cfg.validate().is_err(), "opacity < 0 must be rejected");
        cfg.appearance.background_image.opacity = 1.1;
        assert!(cfg.validate().is_err(), "opacity > 1 must be rejected");
        cfg.appearance.background_image.opacity = 0.0;
        assert!(cfg.validate().is_ok(), "opacity=0 is valid");
        cfg.appearance.background_image.opacity = 1.0;
        assert!(cfg.validate().is_ok(), "opacity=1 is valid");
    }

    #[test]
    fn background_image_brightness_bounds() {
        let mut cfg = Config::default();
        cfg.appearance.background_image.brightness = -0.1;
        assert!(cfg.validate().is_err(), "brightness < 0 must be rejected");
        cfg.appearance.background_image.brightness = 2.1;
        assert!(cfg.validate().is_err(), "brightness > 2 must be rejected");
        cfg.appearance.background_image.brightness = 0.0;
        assert!(cfg.validate().is_ok());
        cfg.appearance.background_image.brightness = 2.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn background_image_hue_bounds() {
        let mut cfg = Config::default();
        cfg.appearance.background_image.hue = -1.0;
        assert!(cfg.validate().is_err(), "hue < 0 must be rejected");
        cfg.appearance.background_image.hue = 361.0;
        assert!(cfg.validate().is_err(), "hue > 360 must be rejected");
        cfg.appearance.background_image.hue = 0.0;
        assert!(cfg.validate().is_ok());
        cfg.appearance.background_image.hue = 360.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn background_image_none_path_is_valid() {
        // A config with no path (disabled) must always pass validation.
        let mut cfg = Config::default();
        cfg.appearance.background_image.path = None;
        cfg.appearance.background_image.opacity = 0.5;
        cfg.validate().expect("disabled bg_image must validate");
    }

    // ── settings-correctness: regression tests ────────────────────────────────

    /// Old `config.toml` files that contain the now-removed `[keystroke_fx]`
    /// table or the removed `quake.stay_on_top` field must still load without
    /// error.
    ///
    /// * `[keystroke_fx]` is tolerated via the `_keystroke_fx: Option<toml::Value>`
    ///   tombstone field on `Config` — without it, `deny_unknown_fields` would
    ///   reject the section and break existing user installs.
    /// * `quake.stay_on_top` is tolerated because `QuakeConfig` uses
    ///   `#[serde(default)]` without `deny_unknown_fields`, so unknown keys in
    ///   the `[quake]` table are silently ignored.
    #[test]
    fn old_config_with_keystroke_fx_table_loads() {
        let toml_src = r#"
[appearance]
theme = "Tokyo Night"

[keystroke_fx]
enabled = false
style = "pixel_rainbow"
intensity = 1.0
cell_size = 14
decay_ms = 450
max_particles = 256

[quake]
animation = "slide"
stay_on_top = true

[terminal]
word_separators = " ()[]"
"#;
        let cfg: Config = toml::from_str(toml_src)
            .expect("old config with [keystroke_fx] and quake.stay_on_top must parse");
        cfg.validate()
            .expect("old config with [keystroke_fx] and quake.stay_on_top must validate");
        // Spot-check that the correctly-present fields were read.
        assert_eq!(cfg.appearance.theme, "Tokyo Night");
        assert_eq!(cfg.quake.animation, QuakeAnimation::Slide);
        assert_eq!(cfg.terminal.word_separators, " ()[]");
        // quake.stay_on_top is silently dropped; window.always_on_top is unaffected.
        assert!(!cfg.window.always_on_top);
    }

    // ── Custom keybinds: decode_send_string ──────────────────────────────────

    #[test]
    fn send_string_newline() {
        assert_eq!(decode_send_string(r"\n"), b"\n");
    }

    #[test]
    fn send_string_carriage_return() {
        assert_eq!(decode_send_string(r"\r"), b"\r");
    }

    #[test]
    fn send_string_tab() {
        assert_eq!(decode_send_string(r"\t"), b"\t");
    }

    #[test]
    fn send_string_escape() {
        assert_eq!(decode_send_string(r"\e"), &[0x1b]);
    }

    #[test]
    fn send_string_literal_backslash() {
        assert_eq!(decode_send_string(r"\\"), b"\\");
    }

    #[test]
    fn send_string_hex_lower() {
        assert_eq!(decode_send_string(r"\x0a"), &[0x0a]);
    }

    #[test]
    fn send_string_hex_upper() {
        assert_eq!(decode_send_string(r"\x1B"), &[0x1b]);
    }

    #[test]
    fn send_string_combined() {
        // ESC prefix + plain text + newline.
        let result = decode_send_string(r"\egit status\n");
        assert_eq!(&result[0..1], &[0x1b]);
        assert_eq!(&result[1..], b"git status\n");
    }

    #[test]
    fn send_string_plain_text() {
        assert_eq!(decode_send_string("hello"), b"hello");
    }

    #[test]
    fn send_string_unknown_escape_passthrough() {
        // \z is not a recognised escape — backslash is kept.
        assert_eq!(decode_send_string(r"\z"), b"\\z");
    }

    // ── Custom keybinds: KeyActionSpec helpers ───────────────────────────────

    #[test]
    fn key_action_spec_send_prefix_returns_bytes() {
        let spec = KeyActionSpec::Action("send:\\n".to_string());
        let bytes = spec.as_send_bytes().expect("send: prefix detected");
        assert_eq!(bytes, b"\n");
    }

    #[test]
    fn key_action_spec_named_action_returns_name() {
        let spec = KeyActionSpec::Action("NewTab".to_string());
        assert!(spec.as_send_bytes().is_none());
        assert_eq!(spec.action_name(), Some("NewTab"));
    }

    // ── Custom keybinds: TOML config roundtrip ───────────────────────────────

    #[test]
    fn custom_keybind_toml_roundtrip() {
        let toml_src = r#"
quake = "Ctrl+`"

[[custom]]
keys    = "Ctrl+Alt+G"
actions = ["NewTab", "send:git status\\n"]
"#;
        let cfg: KeybindsConfig = toml::from_str(toml_src).expect("TOML parse failed");
        assert_eq!(cfg.custom.len(), 1, "one custom bind expected");
        let bind = &cfg.custom[0];
        assert_eq!(bind.keys, "Ctrl+Alt+G");
        assert_eq!(bind.actions.len(), 2, "two actions expected");
        assert_eq!(bind.actions[0], KeyActionSpec::Action("NewTab".to_string()));
        assert_eq!(
            bind.actions[1],
            KeyActionSpec::Action("send:git status\\n".to_string())
        );

        // Re-serialise and parse back — must be lossless.
        let serialised = toml::to_string(&cfg).expect("serialise failed");
        let cfg2: KeybindsConfig = toml::from_str(&serialised).expect("re-parse failed");
        assert_eq!(cfg.custom, cfg2.custom, "roundtrip must be lossless");
    }

    #[test]
    fn custom_keybind_default_is_empty() {
        let cfg = KeybindsConfig::default();
        assert!(
            cfg.custom.is_empty(),
            "default custom keybinds must be empty"
        );
    }

    // ── [ssh] config section ──────────────────────────────────────────────────

    #[test]
    fn ssh_config_default_policy_is_accept_new() {
        assert_eq!(
            Config::default().ssh.host_key_policy,
            HostKeyPolicy::AcceptNew
        );
    }

    #[test]
    fn ssh_config_parses_policy_variants() {
        for (raw, want) in [
            ("accept_new", HostKeyPolicy::AcceptNew),
            ("strict", HostKeyPolicy::Strict),
            ("off", HostKeyPolicy::Off),
        ] {
            let toml_src = format!("[ssh]\nhost_key_policy = \"{raw}\"\n");
            let cfg: Config = toml::from_str(&toml_src)
                .unwrap_or_else(|e| panic!("ssh.host_key_policy `{raw}` must parse: {e}"));
            cfg.validate().expect("must validate");
            assert_eq!(cfg.ssh.host_key_policy, want, "policy `{raw}`");
        }
    }

    #[test]
    fn ssh_config_known_hosts_roundtrips() {
        let toml_src = "[ssh]\nknown_hosts = \"/tmp/my_known_hosts\"\n";
        let cfg: Config = toml::from_str(toml_src).expect("ssh.known_hosts must parse");
        cfg.validate().expect("must validate");
        assert_eq!(
            cfg.ssh.known_hosts,
            std::path::PathBuf::from("/tmp/my_known_hosts")
        );
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(
            back.ssh.known_hosts,
            std::path::PathBuf::from("/tmp/my_known_hosts")
        );
    }

    #[test]
    fn ssh_config_absent_falls_back_to_defaults() {
        // A config that doesn't mention [ssh] at all must still give us the
        // safe default policy (accept_new) — forward-compat for existing configs.
        let cfg: Config = toml::from_str("[font]\nsize = 14.0\n").expect("must parse");
        assert_eq!(cfg.ssh.host_key_policy, HostKeyPolicy::AcceptNew);
    }

    // ── Zen mode config ─────────────────────────────────────────────────────────

    /// `zen_hide` defaults to all four elements (maximum distraction-free).
    #[test]
    fn zen_hide_default_is_all_elements() {
        let cfg = Config::default();
        assert_eq!(
            cfg.window.zen_hide.len(),
            4,
            "zen_hide must default to all 4 elements"
        );
        // Verify all four variants are present.
        assert!(cfg
            .window
            .zen_hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::TabBar)));
        assert!(cfg
            .window
            .zen_hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::StatusBar)));
        assert!(cfg
            .window
            .zen_hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::PaneHeaders)));
        assert!(cfg
            .window
            .zen_hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::TitleBar)));
    }

    /// `zen_fullscreen` defaults to `true`.
    #[test]
    fn zen_fullscreen_default_is_true() {
        assert!(
            Config::default().window.zen_fullscreen,
            "zen_fullscreen must default to true"
        );
    }

    /// `zen_hide` round-trips through TOML unchanged.
    #[test]
    fn zen_hide_toml_roundtrip() {
        let toml_src = r#"
[window]
zen_fullscreen = false
zen_hide = ["tab_bar", "status_bar"]
"#;
        let cfg: Config = toml::from_str(toml_src).expect("zen_hide must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.window.zen_fullscreen);
        assert_eq!(cfg.window.zen_hide.len(), 2);
        assert!(cfg
            .window
            .zen_hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::TabBar)));
        assert!(cfg
            .window
            .zen_hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::StatusBar)));
        // Serialise and re-parse.
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        back.validate()
            .expect("roundtripped zen config must validate");
        assert!(!back.window.zen_fullscreen);
        assert_eq!(back.window.zen_hide.len(), 2);
    }

    /// An empty `zen_hide = []` is valid (zen hides nothing, just activates).
    #[test]
    fn zen_hide_empty_is_valid() {
        let toml_src = "[window]\nzen_hide = []\n";
        let cfg: Config = toml::from_str(toml_src).expect("empty zen_hide must parse");
        cfg.validate().expect("empty zen_hide must validate");
        assert!(cfg.window.zen_hide.is_empty());
    }

    /// All four zen-hide element variants parse from TOML snake_case strings.
    #[test]
    fn zen_hide_all_variants_parse() {
        let toml_src = r#"
[window]
zen_hide = ["tab_bar", "status_bar", "pane_headers", "title_bar"]
"#;
        let cfg: Config = toml::from_str(toml_src).expect("all zen_hide variants must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.window.zen_hide.len(), 4);
    }

    /// `toggle_fullscreen` shortcut defaults to `"F11"`.
    #[test]
    fn toggle_fullscreen_default_binding_is_f11() {
        assert_eq!(
            ShortcutsConfig::default().toggle_fullscreen,
            "F11",
            "toggle_fullscreen must default to F11"
        );
    }

    /// `toggle_zen_mode` shortcut defaults to empty (unbound).
    #[test]
    fn toggle_zen_mode_default_is_unbound() {
        assert!(
            ShortcutsConfig::default().toggle_zen_mode.is_empty(),
            "toggle_zen_mode must be unbound by default"
        );
    }

    /// Both fullscreen and zen shortcuts survive a TOML roundtrip.
    #[test]
    fn fullscreen_and_zen_shortcuts_roundtrip() {
        let toml_src = r#"
[keybinds.shortcuts]
toggle_fullscreen = "F11"
toggle_zen_mode   = "Ctrl+Shift+Period"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("fullscreen/zen shortcuts must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.keybinds.shortcuts.toggle_fullscreen, "F11");
        assert_eq!(cfg.keybinds.shortcuts.toggle_zen_mode, "Ctrl+Shift+Period");
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        back.validate().expect("roundtripped config must validate");
        assert_eq!(back.keybinds.shortcuts.toggle_fullscreen, "F11");
        assert_eq!(back.keybinds.shortcuts.toggle_zen_mode, "Ctrl+Shift+Period");
    }

    // ── Snippet config tests ──────────────────────────────────────────────────

    /// Default config has no snippets.
    #[test]
    fn snippets_default_empty() {
        assert!(Config::default().snippets.is_empty());
    }

    /// A `[[snippets]]` array parses correctly.
    #[test]
    fn snippets_parse_array() {
        let toml_src = r#"
[[snippets]]
name = "Git status"
body = "git status\n"
description = "Show working-tree status"

[[snippets]]
name = "Docker ps"
body = "docker ps\n"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("snippets must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.snippets.len(), 2);
        assert_eq!(cfg.snippets[0].name, "Git status");
        assert_eq!(cfg.snippets[0].body, "git status\n");
        assert_eq!(
            cfg.snippets[0].description,
            Some("Show working-tree status".to_string())
        );
        assert_eq!(cfg.snippets[1].name, "Docker ps");
        assert!(cfg.snippets[1].description.is_none());
    }

    /// Snippets survive a TOML round-trip unchanged.
    #[test]
    fn snippets_toml_roundtrip() {
        let mut cfg = Config::default();
        cfg.snippets.push(Snippet {
            name: "Hello".to_string(),
            body: "echo hello\n".to_string(),
            description: Some("Print hello".to_string()),
        });
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        back.validate().expect("roundtripped config must validate");
        assert_eq!(back.snippets.len(), 1);
        assert_eq!(back.snippets[0].name, "Hello");
        assert_eq!(back.snippets[0].body, "echo hello\n");
        assert_eq!(
            back.snippets[0].description,
            Some("Print hello".to_string())
        );
    }

    /// A snippet with an empty name fails validation.
    #[test]
    fn snippet_rejects_empty_name() {
        let mut cfg = Config::default();
        cfg.snippets.push(Snippet {
            name: String::new(),
            body: "echo hi\n".to_string(),
            description: None,
        });
        assert!(
            cfg.validate().is_err(),
            "empty snippet name must be rejected"
        );
    }

    /// A snippet with a whitespace-only name fails validation.
    #[test]
    fn snippet_rejects_whitespace_only_name() {
        let mut cfg = Config::default();
        cfg.snippets.push(Snippet {
            name: "   ".to_string(),
            body: "echo hi\n".to_string(),
            description: None,
        });
        assert!(
            cfg.validate().is_err(),
            "whitespace-only snippet name must be rejected"
        );
    }

    // ── ai.offer_fix_on_failure ──────────────────────────────────────────────

    /// `offer_fix_on_failure` defaults to `false` (opt-in, never intrusive).
    #[test]
    fn offer_fix_on_failure_defaults_false() {
        assert!(
            !Config::default().ai.offer_fix_on_failure,
            "offer_fix_on_failure must default to false"
        );
    }

    /// Explicit `true` parses, validates, and roundtrips.
    #[test]
    fn offer_fix_on_failure_parses_and_roundtrips() {
        let toml_src = "[ai]\noffer_fix_on_failure = true\n";
        let cfg: Config = toml::from_str(toml_src).expect("offer_fix_on_failure must parse");
        cfg.validate().expect("must validate");
        assert!(cfg.ai.offer_fix_on_failure, "parsed value must be true");
        let serialized = toml::to_string(&cfg).expect("must serialize");
        let back: Config = toml::from_str(&serialized).expect("must roundtrip");
        assert!(
            back.ai.offer_fix_on_failure,
            "roundtripped value must be true"
        );
    }

    /// A legacy config without `offer_fix_on_failure` falls back to `false`.
    #[test]
    fn offer_fix_on_failure_absent_defaults_to_false() {
        let toml_src = "[ai]\ndefault_provider = \"ollama\"\n";
        let cfg: Config = toml::from_str(toml_src).expect("must parse");
        assert!(
            !cfg.ai.offer_fix_on_failure,
            "absent offer_fix_on_failure must fall back to false"
        );
    }

    // ── keybinds.shortcuts.fix_last_command ──────────────────────────────────

    /// `fix_last_command` defaults to empty (unbound) so it never collides with
    /// existing keybinds.
    #[test]
    fn fix_last_command_shortcut_defaults_empty() {
        assert!(
            Config::default()
                .keybinds
                .shortcuts
                .fix_last_command
                .is_empty(),
            "fix_last_command must default to empty (unbound)"
        );
    }

    /// Explicit binding parses, validates, and roundtrips.
    #[test]
    fn fix_last_command_shortcut_parses_and_roundtrips() {
        let toml_src = "[keybinds.shortcuts]\nfix_last_command = \"Ctrl+Shift+X\"\n";
        let cfg: Config = toml::from_str(toml_src).expect("fix_last_command binding must parse");
        cfg.validate().expect("must validate");
        assert_eq!(
            cfg.keybinds.shortcuts.fix_last_command, "Ctrl+Shift+X",
            "parsed binding must match"
        );
        let serialized = toml::to_string(&cfg).expect("must serialize");
        let back: Config = toml::from_str(&serialized).expect("must roundtrip");
        assert_eq!(
            back.keybinds.shortcuts.fix_last_command, "Ctrl+Shift+X",
            "roundtripped binding must match"
        );
    }
}
