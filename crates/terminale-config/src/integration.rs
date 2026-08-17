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
