//! Desktop / OS integration configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which windowing-system backend terminale asks winit for on Linux / BSD.
///
/// This matters far more than it looks. Wayland deliberately gives clients **no
/// control over their own window position**: `set_outer_position` is a silent
/// no-op and `outer_position()` returns an error. Everything terminale does
/// with explicit geometry therefore stops working on a native Wayland surface —
/// Quake edge docking, the Snap Top/Bottom/Left/Right actions,
/// `window.startup_position`, right-click menus and dialogs that open at the
/// cursor, and tab tear-out. X11 (including XWayland, which every mainstream
/// Wayland session runs) supports all of it.
///
/// Terminale therefore defaults to [`Self::Auto`], which prefers X11 whenever an
/// X server is reachable (`$DISPLAY` is set) and falls back to Wayland
/// otherwise. Choose [`Self::Wayland`] explicitly if you would rather have a
/// native Wayland surface — fractional scaling and no XWayland round-trip — and
/// can live without window positioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum LinuxBackend {
    /// Prefer X11 (or XWayland) when `$DISPLAY` is set, so window positioning
    /// works; otherwise use Wayland. The default.
    #[default]
    Auto,
    /// Always request the X11 backend. Under a Wayland session this runs the
    /// window through XWayland.
    X11,
    /// Always request the native Wayland backend. Window positioning
    /// (Quake docking, snap actions, startup position) will not work.
    Wayland,
}

impl LinuxBackend {
    /// All variants in display order — for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Auto, Self::X11, Self::Wayland]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (X11 when available)",
            Self::X11 => "X11 / XWayland",
            Self::Wayland => "Wayland (no window positioning)",
        }
    }
}

/// Which classes of control-socket command the running instance will serve.
///
/// The socket itself is per-user and mode 0600 under `$XDG_RUNTIME_DIR`, so only
/// processes already running as you can reach it. That is a meaningful bar — a
/// process running as you can read `~/.ssh` anyway — but it is *not* the same as
/// "nothing can look at my terminal": every editor plugin, shell hook, and AI
/// agent you install runs as you too. These switches therefore scope what the
/// channel is allowed to do, so `terminale ctl` can be handed to an automation
/// tool without also handing it the ability to run commands in your shell.
///
/// The split that matters is [`Self::allow_input`] versus [`Self::allow_submit`]:
/// input may *type* into the shell, submit may *press Enter*. Keeping submit off
/// is the same trust model the AI features already use — a suggestion is
/// injected for you to read and confirm, never executed behind your back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ControlApiConfig {
    /// Serve the query/automation commands (`list-*`, `get-text`, `send-text`,
    /// `send-keys`, `action`, `screenshot`) in addition to the two commands the
    /// socket has always answered (`ping`, `toggle-quake`).
    ///
    /// Turning this off leaves Quake toggling from a window-manager keybinding
    /// working while refusing everything else. Default: `true`.
    pub enabled: bool,
    /// Allow commands that read terminal *content* — `get-text`, `last-command`,
    /// and the titles/working directories in `list-tabs` / `list-panes`.
    ///
    /// This is the privacy-relevant one: scrollback holds whatever your commands
    /// printed, which can include tokens and keys. Default: `true`.
    pub allow_read: bool,
    /// Allow commands that drive the app — `action` (any command-palette
    /// action), `send-text`, and `send-keys`.
    ///
    /// Text is typed into the focused pane exactly as if you had typed it, but
    /// a trailing newline is stripped unless [`Self::allow_submit`] is also on,
    /// so an injected command lands at the prompt for review. Default: `true`.
    pub allow_input: bool,
    /// Allow injected input to *submit* — i.e. to carry a newline / `Enter` and
    /// therefore actually run a command in your shell.
    ///
    /// Off by default, deliberately. With it off, an automation tool (or an AI
    /// agent) can compose a command at your prompt but you press Enter; with it
    /// on, anything that can reach the socket can run arbitrary commands as
    /// you. Turn it on only for scripted/CI use. Default: `false`.
    pub allow_submit: bool,
    /// Allow `screenshot`, which renders the current frame to a PNG file path
    /// of the caller's choosing.
    ///
    /// Separate from [`Self::allow_read`] because a picture of the window leaks
    /// the same content in a form you cannot grep, and because it writes a file.
    /// Default: `true`.
    pub allow_screenshot: bool,
}

impl Default for ControlApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_read: true,
            allow_input: true,
            allow_submit: false,
            allow_screenshot: true,
        }
    }
}

/// Controls how `terminale` integrates with the host desktop environment.
///
/// On Windows the MSI installer registers Start-Menu / Desktop shortcuts and on
/// macOS the `.app` bundle is placed in `/Applications`, so those platforms are
/// discoverable out of the box. Linux has no install-time hook (the app ships as
/// a plain tarball or via Homebrew), so the binary registers its own
/// `freedesktop` `.desktop` entry on launch — that is what [`Self::desktop_entry`]
/// governs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct IntegrationConfig {
    /// On Linux, register a `freedesktop` desktop entry (and icon) under
    /// `$XDG_DATA_HOME/applications` on launch so `terminale` shows up in the
    /// application menu / launcher and is searchable. The write is idempotent
    /// and only refreshed when the executable path changes. No effect on
    /// Windows or macOS, where the installer/bundle handles this.
    /// Default: `true`.
    pub desktop_entry: bool,
    /// Which windowing backend to request on Linux / BSD — see
    /// [`LinuxBackend`]. Ignored on Windows and macOS. Changing it needs a
    /// restart (the backend is chosen once, when the event loop is built).
    /// Default: [`LinuxBackend::Auto`].
    #[serde(default)]
    pub linux_backend: LinuxBackend,
    /// Listen on a per-user control socket so a second `terminale` invocation
    /// can drive the running instance — this is what makes
    /// `terminale --toggle-quake` work, which in turn lets any window manager
    /// or desktop keybinding toggle the drop-down. Unix only (the socket lives
    /// under `$XDG_RUNTIME_DIR`). Default: `true`.
    #[serde(default = "default_true")]
    pub control_socket: bool,
    /// On Wayland, register the Quake hotkey through the XDG
    /// `org.freedesktop.portal.GlobalShortcuts` portal.
    ///
    /// Wayland compositors do not let clients grab keys globally, so the X11
    /// key-grab terminale uses everywhere else only ever fires while an X11
    /// window has focus — which under a Wayland session is almost never. The
    /// portal is the supported replacement (GNOME 48+, KDE Plasma 6+): the
    /// desktop asks you to confirm the binding once, then delivers the
    /// activation to terminale no matter which window has focus.
    ///
    /// When the portal is unavailable the app falls back to the X11 grab and
    /// logs why. Ignored outside Linux/Wayland. Default: `true`.
    #[serde(default = "default_true")]
    pub global_shortcuts_portal: bool,
    /// What the control socket is allowed to do beyond toggling Quake — see
    /// [`ControlApiConfig`]. Ignored entirely when [`Self::control_socket`] is
    /// `false`, since then there is no socket to serve.
    #[serde(default)]
    pub control_api: ControlApiConfig,
    /// Let `terminale --toggle-quake` *start* terminale when it finds nothing
    /// running, instead of failing.
    ///
    /// This is what makes a desktop-owned Quake hotkey work from a fresh login:
    /// the first press has no instance to talk to, so the process that was
    /// spawned to deliver the toggle becomes the instance and comes up as the
    /// drop-down. Every press after that is a plain toggle over the socket.
    /// Without it the hotkey silently does nothing until you have launched
    /// terminale by hand at least once.
    ///
    /// Only a socket that is *absent* triggers this. A socket that exists but
    /// does not answer means an instance is there and wedged, and starting a
    /// second one would not help. Unix only. Default: `true`.
    #[serde(default = "default_true")]
    pub quake_launch_on_demand: bool,
    /// On Linux, also install a second, drop-down-only desktop entry
    /// (`terminale.Quake.desktop`) alongside the application-menu one.
    ///
    /// It exists for GNOME/KDE shell extensions that implement a drop-down
    /// terminal themselves — Quake Terminal on GNOME being the common one.
    /// Those launch an app by desktop-entry id and then look for its window by
    /// application id, so the drop-down needs an entry, and an identity, of its
    /// own: pointed at the ordinary entry, such an extension would sooner or
    /// later grab the terminale window you were working in. Harmless when no
    /// such extension is installed — it is one more (hidden-in-plain-sight)
    /// launcher in the application list. Default: `false`.
    #[serde(default)]
    pub quake_desktop_entry: bool,
}

/// Serde default for the boolean fields that default to `true`.
fn default_true() -> bool {
    true
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self {
            desktop_entry: true,
            linux_backend: LinuxBackend::default(),
            control_socket: true,
            global_shortcuts_portal: true,
            control_api: ControlApiConfig::default(),
            quake_launch_on_demand: true,
            quake_desktop_entry: false,
        }
    }
}

impl IntegrationConfig {
    /// Validate field ranges. Currently infallible; the `Result` return type
    /// is kept for uniformity with the other config sections (so `Config::
    /// validate` can call it the same way) and to leave room for future fields.
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
    fn default_registers_desktop_entry() {
        let cfg = IntegrationConfig::default();
        assert!(cfg.desktop_entry);
        cfg.validate().expect("default must validate");
    }

    #[test]
    fn quake_hotkey_starts_terminale_by_default_but_installs_no_extra_launcher() {
        let cfg = IntegrationConfig::default();
        // A hotkey that does nothing on the first press of the session is the
        // behaviour this defaults away from.
        assert!(cfg.quake_launch_on_demand);
        // The second launcher entry only makes sense with a drop-down shell
        // extension, so it is opt-in.
        assert!(!cfg.quake_desktop_entry);
        cfg.validate().expect("default must validate");
    }

    #[test]
    fn quake_fields_survive_a_config_written_before_they_existed() {
        // Both carry `#[serde(default …)]`, so an older config.toml that names
        // neither must still load — and land on the documented defaults.
        let cfg: IntegrationConfig =
            toml::from_str("desktop_entry = true\ncontrol_socket = true\n")
                .expect("an older config must still parse");
        assert!(cfg.quake_launch_on_demand);
        assert!(!cfg.quake_desktop_entry);
    }

    #[test]
    fn roundtrip_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            integration: IntegrationConfig,
        }
        let w = Wrap {
            integration: IntegrationConfig {
                desktop_entry: false,
                ..IntegrationConfig::default()
            },
        };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert!(!back.integration.desktop_entry);
        assert!(back.integration.quake_launch_on_demand);
        assert!(!back.integration.quake_desktop_entry);
    }

    #[test]
    fn linux_backend_defaults_to_auto() {
        assert_eq!(
            IntegrationConfig::default().linux_backend,
            LinuxBackend::Auto
        );
    }

    #[test]
    fn linux_backend_roundtrips() {
        for backend in LinuxBackend::all() {
            let cfg = IntegrationConfig {
                linux_backend: backend,
                ..IntegrationConfig::default()
            };
            let s = toml::to_string(&cfg).expect("serialize");
            let back: IntegrationConfig = toml::from_str(&s).expect("deserialize");
            assert_eq!(back.linux_backend, backend);
        }
    }

    /// A config written before these fields existed must keep the new
    /// defaults instead of falling back to `false` (which would silently
    /// disable the Quake hotkey on Wayland).
    #[test]
    fn legacy_config_keeps_new_defaults() {
        let legacy: IntegrationConfig =
            toml::from_str("desktop_entry = true").expect("deserialize legacy");
        assert_eq!(legacy.linux_backend, LinuxBackend::Auto);
        assert!(legacy.control_socket);
        assert!(legacy.global_shortcuts_portal);
    }

    /// Submitting must stay opt-in: a default install lets an automation tool
    /// compose a command at the prompt, never run it.
    #[test]
    fn control_api_defaults_deny_submit_only() {
        let api = IntegrationConfig::default().control_api;
        assert!(api.enabled);
        assert!(api.allow_read);
        assert!(api.allow_input);
        assert!(api.allow_screenshot);
        assert!(!api.allow_submit, "submit must be opt-in");
    }

    /// A config written before `[integration.control_api]` existed must inherit
    /// the defaults rather than deserialize to an all-`false` struct, which
    /// would silently disable the whole command surface.
    #[test]
    fn legacy_config_keeps_control_api_defaults() {
        let legacy: IntegrationConfig =
            toml::from_str("desktop_entry = true").expect("deserialize legacy");
        assert_eq!(legacy.control_api, ControlApiConfig::default());
    }

    #[test]
    fn control_api_roundtrips() {
        let cfg = IntegrationConfig {
            control_api: ControlApiConfig {
                enabled: true,
                allow_read: false,
                allow_input: true,
                allow_submit: true,
                allow_screenshot: false,
            },
            ..IntegrationConfig::default()
        };
        let s = toml::to_string(&cfg).expect("serialize");
        let back: IntegrationConfig = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.control_api, cfg.control_api);
    }

    #[test]
    fn control_socket_and_portal_roundtrip() {
        let cfg = IntegrationConfig {
            control_socket: false,
            global_shortcuts_portal: false,
            ..IntegrationConfig::default()
        };
        let s = toml::to_string(&cfg).expect("serialize");
        let back: IntegrationConfig = toml::from_str(&s).expect("deserialize");
        assert!(!back.control_socket);
        assert!(!back.global_shortcuts_portal);
    }
}
