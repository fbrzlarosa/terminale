//! Window geometry, scroll, and display configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Session restore ───────────────────────────────────────────────────────────

/// What terminale should do with the previous session on next launch.
///
/// Defaults to `Off` so no one is surprised by a restore happening
/// without opting in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RestoreSession {
    /// Do not restore the previous session (default).
    #[default]
    Off,
    /// Silently restore the last session on launch.
    LastSession,
}

impl RestoreSession {
    /// All variants in display order.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Off, Self::LastSession]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::LastSession => "Restore last session",
        }
    }
}

/// A monitor rectangle in physical pixels: `(origin_x, origin_y, width, height)`.
/// Matches winit's `MonitorHandle::position()` + `size()`.
pub type MonitorRect = (i32, i32, u32, u32);

/// A window rectangle in physical pixels: `(x, y, width, height)`.
pub type WindowRect = (i32, i32, u32, u32);

/// When the scrollback scrollbar on the right edge is shown. The bar is
/// interactive in every visible mode: grab the thumb to drag through
/// history, click the track to jump there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollbarMode {
    /// Visible while panning history, and revealed when the pointer hovers
    /// the right edge — so it can be grabbed even from the live bottom.
    #[default]
    Auto,
    /// Visible whenever any scrollback history exists.
    Always,
    /// Never drawn (and never grabbable).
    Never,
}

impl ScrollbarMode {
    /// All variants in display order — useful for UI / iteration.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Auto, Self::Always, Self::Never]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (on scroll or hover)",
            Self::Always => "Always",
            Self::Never => "Never",
        }
    }
}

/// Edge (centre, or full-screen) a window snaps to on its current monitor.
/// Drives the standalone `Snap*` shortcut actions (independent of Quake mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapEdge {
    /// Top half of the monitor (full width, half height).
    Top,
    /// Bottom half of the monitor (full width, half height).
    Bottom,
    /// Left half of the monitor (half width, full height).
    Left,
    /// Right half of the monitor (half width, full height).
    Right,
    /// Keep the current size, centred on the monitor.
    Center,
    /// Fill the entire monitor work area.
    Maximize,
    /// Top-left quarter of the monitor (half width, half height).
    TopLeft,
    /// Top-right quarter of the monitor (half width, half height).
    TopRight,
    /// Bottom-left quarter of the monitor (half width, half height).
    BottomLeft,
    /// Bottom-right quarter of the monitor (half width, half height).
    BottomRight,
}

impl SnapEdge {
    /// All variants in display order — useful for UI / iteration.
    #[must_use]
    pub fn all() -> [Self; 10] {
        [
            Self::Top,
            Self::Bottom,
            Self::Left,
            Self::Right,
            Self::Center,
            Self::Maximize,
            Self::TopLeft,
            Self::TopRight,
            Self::BottomLeft,
            Self::BottomRight,
        ]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Center => "Center",
            Self::Maximize => "Maximize",
            Self::TopLeft => "Top-Left",
            Self::TopRight => "Top-Right",
            Self::BottomLeft => "Bottom-Left",
            Self::BottomRight => "Bottom-Right",
        }
    }
}

/// Pure placement math for the standalone snap actions: given a monitor
/// rectangle, the target [`SnapEdge`] and the window's *current* rectangle,
/// compute the snapped window rectangle in physical pixels.
///
/// `Top`/`Bottom` are full-width half-height halves pinned to that edge;
/// `Left`/`Right` are half-width full-height halves; `Center` keeps the
/// current size (clamped to the monitor) and centres it on both axes;
/// `Maximize` fills the whole monitor. `current` only matters for `Center`
/// (its size is preserved) — the edge/maximize variants ignore it.
///
/// This is deliberately free of any windowing-system types so it can be
/// unit-tested in isolation.
#[must_use]
pub fn snap_window_rect(monitor: MonitorRect, edge: SnapEdge, current: WindowRect) -> WindowRect {
    let (mon_x, mon_y, mon_w, mon_h) = monitor;
    let half_w = mon_w / 2;
    let half_h = mon_h / 2;
    match edge {
        SnapEdge::Top => (mon_x, mon_y, mon_w, half_h),
        SnapEdge::Bottom => (mon_x, mon_y + half_h as i32, mon_w, mon_h - half_h),
        SnapEdge::Left => (mon_x, mon_y, half_w, mon_h),
        SnapEdge::Right => (mon_x + half_w as i32, mon_y, mon_w - half_w, mon_h),
        SnapEdge::Maximize => (mon_x, mon_y, mon_w, mon_h),
        SnapEdge::Center => {
            // Keep the window's size, but never larger than the monitor, and
            // centre it. Slack can't go negative because `w`/`h` are clamped.
            let (_, _, cur_w, cur_h) = current;
            let w = cur_w.min(mon_w);
            let h = cur_h.min(mon_h);
            let slack_x = (mon_w - w) as i32;
            let slack_y = (mon_h - h) as i32;
            (mon_x + slack_x / 2, mon_y + slack_y / 2, w, h)
        }
        // Quarter snaps: half-width × half-height, anchored at each corner.
        SnapEdge::TopLeft => (mon_x, mon_y, half_w, half_h),
        SnapEdge::TopRight => (mon_x + half_w as i32, mon_y, mon_w - half_w, half_h),
        SnapEdge::BottomLeft => (mon_x, mon_y + half_h as i32, half_w, mon_h - half_h),
        SnapEdge::BottomRight => (
            mon_x + half_w as i32,
            mon_y + half_h as i32,
            mon_w - half_w,
            mon_h - half_h,
        ),
    }
}

/// Which chrome elements zen mode hides. Serialised as a list of strings, e.g.
/// `zen_hide = ["tab_bar", "status_bar", "pane_headers", "title_bar"]`.
///
/// Default: all four elements are hidden (maximum distraction-free focus).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZenHideElement {
    /// The horizontal or vertical tab bar.
    TabBar,
    /// The optional status bar strip.
    StatusBar,
    /// Per-pane header strips (visible when a tab has multiple panes).
    PaneHeaders,
    /// The custom window title bar / window-controls row.
    TitleBar,
}

impl ZenHideElement {
    /// All four elements in canonical order.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [
            Self::TabBar,
            Self::StatusBar,
            Self::PaneHeaders,
            Self::TitleBar,
        ]
    }

    /// Display label shown in Settings.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::TabBar => "Tab bar",
            Self::StatusBar => "Status bar",
            Self::PaneHeaders => "Pane headers",
            Self::TitleBar => "Title bar",
        }
    }
}

/// Convenience: the default zen-hide list hides all four chrome elements.
fn default_zen_hide() -> Vec<ZenHideElement> {
    ZenHideElement::all().to_vec()
}

/// Convenience: restore working dirs is on by default.
fn default_restore_working_dirs() -> bool {
    true
}

/// Serde default for [`WindowConfig::session_autosave_secs`].
fn default_session_autosave_secs() -> u32 {
    15
}

fn default_restore_window_geometry() -> bool {
    true
}

/// Convenience: zen mode enters full-screen by default.
fn default_zen_fullscreen() -> bool {
    true
}

/// Window configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WindowConfig {
    /// Window opacity in `[0.0, 1.0]`.
    pub opacity: f32,
    /// Internal padding (px) on all sides.
    pub padding: u32,
    /// Terminal rows scrolled per mouse-wheel notch when on the main
    /// screen. Clamp range: 1..=50.
    pub scroll_step_lines: u8,
    /// Terminal rows forwarded per wheel notch when the app has claimed the
    /// alt-screen (e.g. editors, pagers). Clamp range: 1..=50.
    /// Default: 3.
    pub alt_screen_scroll_lines: u8,
    /// How many pixels of high-resolution (trackpad) scroll input equal one
    /// terminal row. Lower values = faster scrolling on precision trackpads.
    /// Clamp range: 1.0..=128.0. Default: 16.0.
    pub touchpad_pixels_per_row: f32,
    /// Accumulate sub-row trackpad deltas across events instead of
    /// discarding the fractional remainder each time. When enabled, slow
    /// precision-trackpad gestures scroll smoothly by carrying the leftover
    /// fraction into the next event. Off by default.
    pub smooth_scroll: bool,
    /// Snap the viewport back to the live edge (newest output) whenever you
    /// type a key or paste while scrolled up into history — the standard
    /// "type to return to the prompt" behaviour of iTerm2, Windows Terminal,
    /// and friends. When `false`, input is sent but the view stays parked in
    /// the scrollback (you keep reading history while typing blind). On by
    /// default. Note: this is independent of follow-on-output, which always
    /// keeps you pinned to the bottom while you are already there.
    pub scroll_on_input: bool,
    /// Copy the selection to the clipboard automatically as soon as a
    /// mouse selection is made (classic X11 behaviour). Off by default so
    /// it doesn't surprise users by clobbering their clipboard.
    pub copy_on_select: bool,
    /// Maximum number of scrollback (history) lines retained per terminal.
    /// `0` disables scrollback entirely (only the visible screen is kept).
    /// Applies live to every open tab; capped at 1_000_000 to bound memory.
    pub scrollback_lines: usize,
    /// When the scrollback scrollbar on the right edge is shown — `auto`
    /// (default: on scroll or right-edge hover), `always`, or `never`. The
    /// bar is interactive whenever visible: grab the thumb to drag through
    /// history, click the track to jump there. Applies live.
    pub scrollbar: ScrollbarMode,
    /// Require a confirming second close action before a tab (or the last
    /// tab / window) actually closes. When `true`, the first close arms a
    /// short window (~1.5 s); a second close within it goes through, while
    /// any other action cancels it. No modal dialog is shown. Off by
    /// default so single-action close stays instant.
    pub confirm_close: bool,
    /// Keep the window pinned above all other application windows
    /// ("always on top" / "stay on top"). Off by default. Applies live
    /// when toggled from Settings, the command palette, or the right-click
    /// menu. Honoured for all window modes including Quake.
    pub always_on_top: bool,
    /// Where the window should sit on launch — `None` lets the OS pick
    /// (default), or pick a [`SnapEdge`] to position the window on its
    /// monitor (top/bottom/left/right half, centred, or maximized). The
    /// same Snap* shortcut actions snap an already-open window at runtime.
    pub startup_position: Option<SnapEdge>,
    /// Automatically reload the config from disk when `config.toml` is
    /// modified by an external editor. Changes are debounced and
    /// live-applied exactly as if you had made them in the Settings window.
    /// Default `true`.
    pub auto_reload_config: bool,
    /// Which chrome elements `ToggleZenMode` hides when zen mode is
    /// activated. Any combination of `"tab_bar"`, `"status_bar"`,
    /// `"pane_headers"`, and `"title_bar"`. Default: all four.
    #[serde(default = "default_zen_hide")]
    pub zen_hide: Vec<ZenHideElement>,
    /// When `true`, activating zen mode also enters borderless full-screen.
    /// Exiting zen mode restores the prior windowed / maximized state.
    /// Default `true`.
    #[serde(default = "default_zen_fullscreen")]
    pub zen_fullscreen: bool,
    /// Profile used for the first tab of a new window opened via the
    /// `NewWindow` shortcut or command palette. `None` (the default) uses the
    /// overall default profile (same as opening a fresh tab with Ctrl+T).
    /// Must match a name from `[profiles.profiles]` — unknown names are
    /// silently treated as `None` at runtime.
    #[serde(default)]
    pub new_window_profile: Option<String>,
    /// What to do with the previous session on the next launch.
    /// `off` (default) — nothing; `last_session` — silently restore the
    /// saved layout + working directories. Live process state cannot be
    /// restored; only the tab/pane layout and each shell's last directory
    /// are brought back.
    #[serde(default)]
    pub restore_session: RestoreSession,
    /// When `restore_session` is active, also restore each pane to its last
    /// working directory (as announced via OSC 7). If `false`, shells open
    /// in the profile's default directory. Default `true`.
    #[serde(default = "default_restore_working_dirs")]
    pub restore_working_dirs: bool,
    /// When `restore_session` is active, also restore the window's geometry
    /// (position + size), the monitor it was on, and — if it was closed as a
    /// Quake drop-down — reopen it in Quake mode on that same monitor. The
    /// monitor is matched by its OS friendly name, so it survives reboots and
    /// origin shifts. If `false`, only the tab/pane layout is restored and the
    /// window opens at its default geometry. Default `true`.
    #[serde(default = "default_restore_window_geometry")]
    pub restore_window_geometry: bool,
    /// How often the "last session" snapshot is auto-saved to disk while the
    /// app is running, in seconds. `0` means "save only on graceful close"
    /// (the legacy behaviour — a crash or power loss will lose the session).
    /// Any non-zero value triggers a periodic autosave on that cadence, so an
    /// unexpected exit still leaves a recent snapshot to restore from. Must be
    /// `0` or in `5..=3600`. Default `15`.
    #[serde(default = "default_session_autosave_secs")]
    pub session_autosave_secs: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            padding: 8,
            scroll_step_lines: 3,
            alt_screen_scroll_lines: 3,
            touchpad_pixels_per_row: 16.0,
            smooth_scroll: false,
            scroll_on_input: true,
            copy_on_select: false,
            scrollback_lines: 10_000,
            scrollbar: ScrollbarMode::default(),
            confirm_close: false,
            always_on_top: false,
            startup_position: None,
            auto_reload_config: true,
            zen_hide: default_zen_hide(),
            zen_fullscreen: default_zen_fullscreen(),
            new_window_profile: None,
            restore_session: RestoreSession::Off,
            restore_working_dirs: true,
            restore_window_geometry: true,
            session_autosave_secs: default_session_autosave_secs(),
        }
    }
}

impl WindowConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err(ConfigError::Invalid {
                field: "window.opacity",
                message: "must be between 0.0 and 1.0",
            });
        }
        if self.scrollback_lines > 1_000_000 {
            return Err(ConfigError::Invalid {
                field: "window.scrollback_lines",
                message: "must be at most 1000000",
            });
        }
        if !(1.0..=128.0).contains(&self.touchpad_pixels_per_row) {
            return Err(ConfigError::Invalid {
                field: "window.touchpad_pixels_per_row",
                message: "must be between 1.0 and 128.0",
            });
        }
        if self.padding > 64 {
            return Err(ConfigError::Invalid {
                field: "window.padding",
                message: "must be at most 64",
            });
        }
        if self.session_autosave_secs != 0 && !(5..=3600).contains(&self.session_autosave_secs) {
            return Err(ConfigError::Invalid {
                field: "window.session_autosave_secs",
                message: "must be 0 (save on close only) or between 5 and 3600",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ScrollbarMode ─────────────────────────────────────────────────────────

    #[test]
    fn scrollbar_defaults_to_auto() {
        assert_eq!(WindowConfig::default().scrollbar, ScrollbarMode::Auto);
    }

    #[test]
    fn scroll_on_input_defaults_to_true() {
        assert!(
            WindowConfig::default().scroll_on_input,
            "scroll_on_input must default to true"
        );
    }

    #[test]
    fn scroll_on_input_roundtrips_and_legacy_defaults_true() {
        let cfg = WindowConfig {
            scroll_on_input: false,
            ..WindowConfig::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let back: WindowConfig = toml::from_str(&toml).unwrap();
        assert!(
            !back.scroll_on_input,
            "scroll_on_input=false must roundtrip"
        );
        // A config file written before the field existed keeps the default.
        let legacy: WindowConfig = toml::from_str("").unwrap();
        assert!(
            legacy.scroll_on_input,
            "absent scroll_on_input must default to true"
        );
    }

    #[test]
    fn scrollbar_mode_roundtrip() {
        for mode in ScrollbarMode::all() {
            let cfg = WindowConfig {
                scrollbar: mode,
                ..WindowConfig::default()
            };
            let toml = toml::to_string(&cfg).unwrap();
            let back: WindowConfig = toml::from_str(&toml).unwrap();
            assert_eq!(back.scrollbar, mode);
        }
        // A config file written before the field existed keeps the default.
        let legacy: WindowConfig = toml::from_str("").unwrap();
        assert_eq!(legacy.scrollbar, ScrollbarMode::Auto);
    }

    // Monitor used in all tests: origin (0, 0), 2000 × 1200 px.
    const MON: MonitorRect = (0, 0, 2000, 1200);
    // Arbitrary current window rect (only matters for Center).
    const CUR: WindowRect = (0, 0, 800, 600);

    // ── Quarter-snap geometry ─────────────────────────────────────────────────

    #[test]
    fn snap_top_left_is_top_left_quarter() {
        let r = snap_window_rect(MON, SnapEdge::TopLeft, CUR);
        // Origin at (0, 0), half width = 1000, half height = 600.
        assert_eq!(r, (0, 0, 1000, 600));
    }

    #[test]
    fn snap_top_right_is_top_right_quarter() {
        let r = snap_window_rect(MON, SnapEdge::TopRight, CUR);
        // x starts at half_w = 1000; w = mon_w - half_w = 1000; h = half_h = 600.
        assert_eq!(r, (1000, 0, 1000, 600));
    }

    #[test]
    fn snap_bottom_left_is_bottom_left_quarter() {
        let r = snap_window_rect(MON, SnapEdge::BottomLeft, CUR);
        // y starts at half_h = 600; w = half_w = 1000; h = mon_h - half_h = 600.
        assert_eq!(r, (0, 600, 1000, 600));
    }

    #[test]
    fn snap_bottom_right_is_bottom_right_quarter() {
        let r = snap_window_rect(MON, SnapEdge::BottomRight, CUR);
        // x = half_w = 1000, y = half_h = 600, w = 1000, h = 600.
        assert_eq!(r, (1000, 600, 1000, 600));
    }

    /// All four quarters must tile exactly — each must have area == 1/4 of monitor.
    #[test]
    fn quarter_snaps_tile_the_monitor() {
        let quarters = [
            snap_window_rect(MON, SnapEdge::TopLeft, CUR),
            snap_window_rect(MON, SnapEdge::TopRight, CUR),
            snap_window_rect(MON, SnapEdge::BottomLeft, CUR),
            snap_window_rect(MON, SnapEdge::BottomRight, CUR),
        ];
        let (_, _, mon_w, mon_h) = MON;
        let total_area: u64 = quarters.iter().map(|r| r.2 as u64 * r.3 as u64).sum();
        assert_eq!(
            total_area,
            mon_w as u64 * mon_h as u64,
            "four quarters must cover the whole monitor area"
        );
    }

    /// Quarter snaps with a non-power-of-two monitor width must not overlap or
    /// leave gaps on the X axis. Test with an odd-width monitor (1999 × 1200).
    #[test]
    fn quarter_snaps_cover_odd_width_monitor() {
        let mon: MonitorRect = (0, 0, 1999, 1200);
        let tl = snap_window_rect(mon, SnapEdge::TopLeft, CUR);
        let tr = snap_window_rect(mon, SnapEdge::TopRight, CUR);
        // TL right edge must exactly meet TR left edge.
        assert_eq!(
            tl.0 + tl.2 as i32,
            tr.0,
            "TL right edge ({}) must meet TR left edge ({})",
            tl.0 + tl.2 as i32,
            tr.0
        );
        // Combined width must equal monitor width.
        assert_eq!(
            tl.2 + tr.2,
            mon.2,
            "TL + TR widths ({}) must equal monitor width ({})",
            tl.2 + tr.2,
            mon.2
        );
    }

    // ── startup_position TOML roundtrip (new corner variants) ─────────────────

    #[test]
    fn startup_position_corner_roundtrips() {
        for edge in [
            SnapEdge::TopLeft,
            SnapEdge::TopRight,
            SnapEdge::BottomLeft,
            SnapEdge::BottomRight,
        ] {
            let cfg = WindowConfig {
                startup_position: Some(edge),
                ..WindowConfig::default()
            };
            let serialised = toml::to_string(&cfg).expect("serialise");
            let roundtripped: WindowConfig = toml::from_str(&serialised).expect("deserialise");
            assert_eq!(
                roundtripped.startup_position,
                Some(edge),
                "startup_position {edge:?} must survive a TOML roundtrip",
            );
        }
    }

    // ── Padding validation ────────────────────────────────────────────────────

    /// Default WindowConfig must validate without error.
    #[test]
    fn window_config_default_is_valid() {
        assert!(WindowConfig::default().validate().is_ok());
    }

    /// Default padding value must be 8.
    #[test]
    fn window_config_default_padding_is_8() {
        assert_eq!(WindowConfig::default().padding, 8);
    }

    #[test]
    fn restore_window_geometry_defaults_on() {
        assert!(WindowConfig::default().restore_window_geometry);
    }

    #[test]
    fn restore_window_geometry_roundtrip() {
        let cfg = WindowConfig {
            restore_window_geometry: false,
            ..WindowConfig::default()
        };
        let toml = toml::to_string(&cfg).unwrap();
        let back: WindowConfig = toml::from_str(&toml).unwrap();
        assert!(!back.restore_window_geometry);
    }

    /// padding = 0 is the minimum and must be accepted.
    #[test]
    fn padding_zero_is_valid() {
        let cfg = WindowConfig {
            padding: 0,
            ..WindowConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    /// padding = 64 is the Settings-slider maximum and must be accepted.
    #[test]
    fn padding_64_is_valid() {
        let cfg = WindowConfig {
            padding: 64,
            ..WindowConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    /// padding = 65 exceeds the slider maximum and must be rejected.
    #[test]
    fn padding_65_is_invalid() {
        let cfg = WindowConfig {
            padding: 65,
            ..WindowConfig::default()
        };
        assert!(cfg.validate().is_err(), "padding > 64 must fail validation");
    }

    /// padding range: verify the boundary between valid (64) and invalid (65).
    #[test]
    fn padding_range_validates() {
        let ok = WindowConfig {
            padding: 64,
            ..WindowConfig::default()
        };
        let err = WindowConfig {
            padding: 65,
            ..WindowConfig::default()
        };
        assert!(ok.validate().is_ok(), "padding=64 must validate");
        assert!(err.validate().is_err(), "padding=65 must not validate");
    }
}
