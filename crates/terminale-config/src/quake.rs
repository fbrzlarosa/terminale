//! Quake drop-down terminal mode — docking edge, animation, and display.

use crate::window::{MonitorRect, WindowRect};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Open/close animation for the Quake show/hide toggle.
///
/// `Slide`/`Bounce`/`Scale` animate the **OS window geometry** as an
/// edge-pinned reveal that never leaves the monitor; `Fade` animates the
/// whole-window opacity (Windows; instant elsewhere). There are no
/// in-content shader effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuakeAnimation {
    /// Show/hide instantly, no animation.
    None,
    /// Slide (default): an edge-pinned reveal — the docked edge stays put
    /// and the window's perpendicular extent grows/shrinks with an ease-out
    /// cubic curve. Never crosses onto a neighbouring monitor.
    ///
    /// Old config values `zoom`, `pixel_dissolve`, `glitch`, and
    /// `scanline_wipe` are silently mapped to `Slide` for backward
    /// compatibility.
    #[serde(
        alias = "zoom",
        alias = "pixel_dissolve",
        alias = "glitch",
        alias = "scanline_wipe"
    )]
    Slide,
    /// Bounce — like Slide but with a springy, sin-damped growth curve.
    Bounce,
    /// Scale — the window zooms from a point at the centre of the dock edge,
    /// interpolating both axes each frame.
    Scale,
    /// Fade — the window stays at its resting geometry and the whole-window
    /// opacity animates (Windows layered-window alpha). On macOS/Linux this
    /// currently degrades to an instant show/hide.
    Fade,
}

impl Default for QuakeAnimation {
    fn default() -> Self {
        Self::Slide
    }
}

impl QuakeAnimation {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [
            Self::None,
            Self::Slide,
            Self::Bounce,
            Self::Scale,
            Self::Fade,
        ]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None (instant)",
            Self::Slide => "Slide",
            Self::Bounce => "Bounce",
            Self::Scale => "Scale",
            Self::Fade => "Fade",
        }
    }
}

/// Which edge of the target monitor the Quake terminal docks to. `Off`
/// keeps the historical "pure show/hide with exact-geometry restore"
/// behaviour — Quake reappears wherever the user last left it. The four
/// edge variants compute the dock rect on every show from
/// `size_percent` + `margin_px` + the chosen [`QuakeDisplay`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuakeEdge {
    /// No docking — Quake is a free-floating window with exact-geometry
    /// restore on show/hide (the previous default behaviour).
    Off,
    /// Dock to the top edge — full width minus margin, height =
    /// `size_percent` of the monitor's height.
    Top,
    /// Dock to the bottom edge.
    Bottom,
    /// Dock to the left edge — full height minus margin, width =
    /// `size_percent` of the monitor's width.
    Left,
    /// Dock to the right edge.
    Right,
}

impl Default for QuakeEdge {
    fn default() -> Self {
        Self::Off
    }
}

impl QuakeEdge {
    /// Every variant, in display order, for UI dropdowns / segmented
    /// pickers.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [Self::Off, Self::Top, Self::Bottom, Self::Left, Self::Right]
    }

    /// Human-readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

/// Which monitor the Quake terminal docks on. `Current` follows the
/// window's current monitor at toggle time; `Primary` always uses the
/// OS-designated primary; `Index(n)` pins it to the n-th enumerated
/// monitor (the order winit returns from `available_monitors()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuakeDisplay {
    /// Use whichever monitor the Quake window is currently sitting on
    /// (or, if it's hidden, the one it was last on).
    Current,
    /// Always use the OS primary monitor.
    Primary,
    /// Pin to a specific 0-based monitor index.
    Index(u8),
}

impl Default for QuakeDisplay {
    fn default() -> Self {
        Self::Current
    }
}

/// Quake-mode behaviour. Quake can either be:
/// * a **docked** terminal (`edge != Off`) — the window snaps to one
///   edge of the chosen monitor on every show, sized as a fraction of
///   the monitor's perpendicular extent and inset by `margin_px` along
///   the dock axis;
/// * a **free-floating** terminal (`edge == Off`, default) — a pure
///   show/hide toggle that restores the window's exact last geometry.
///
/// Unlike most config structs, this one does **not** use
/// `deny_unknown_fields`: the pre-rework schema had top/height knobs
/// (`height_ratio`, `width_ratio`, `top_offset_px`, …) that were dropped
/// when Quake became pure show/hide. Tolerating obsolete fields here lets
/// older user configs keep loading instead of falling back to defaults
/// (which silently loses ALL the user's other settings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct QuakeConfig {
    /// Open/close animation style. Defaults to [`QuakeAnimation::Slide`].
    pub animation: QuakeAnimation,
    /// Animation duration in milliseconds. Clamped to a sane range when used.
    pub animation_ms: u32,
    /// Which edge to dock to. `Off` (default) preserves the legacy
    /// "free-floating with exact-geometry restore" behaviour.
    pub edge: QuakeEdge,
    /// Which monitor to dock on (only consulted when `edge != Off`).
    pub display: QuakeDisplay,
    /// Fraction of the monitor's perpendicular extent the docked window
    /// occupies — height for Top/Bottom, width for Left/Right. Clamped
    /// to `0.1..=1.0` when applied. Default: `0.5` (half the monitor).
    pub size_percent: f32,
    /// Margin (logical pixels) along the dock edge — keeps the docked
    /// window from sitting flush against the perpendicular screen
    /// edges. Default: `0`.
    pub margin_px: u32,
    /// Auto-hide the Quake window when it loses focus. Default: off.
    pub hide_on_focus_loss: bool,
}

impl Default for QuakeConfig {
    fn default() -> Self {
        Self {
            animation: QuakeAnimation::default(),
            animation_ms: 120,
            edge: QuakeEdge::default(),
            display: QuakeDisplay::default(),
            size_percent: 0.5,
            margin_px: 0,
            hide_on_focus_loss: false,
        }
    }
}

/// Compute the docked window rect from settings + the target monitor.
/// `mon` is the monitor's physical pixel rect; `edge` decides the
/// orientation; `size_percent` (clamped 0.1..=1.0) is the fraction of
/// the perpendicular extent the window occupies; `margin_px` is the
/// gap along the dock axis (logical, but we treat it as physical here
/// — callers convert if needed).
///
/// Returns `None` for `QuakeEdge::Off` since there's no computed rect.
#[must_use]
pub fn quake_dock_rect(
    mon: MonitorRect,
    edge: QuakeEdge,
    size_percent: f32,
    margin_px: u32,
) -> Option<WindowRect> {
    let (mx, my, mw, mh) = mon;
    let frac = size_percent.clamp(0.1, 1.0);
    let margin = margin_px as i32;
    match edge {
        QuakeEdge::Off => None,
        QuakeEdge::Top => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let h = ((mh as f32) * frac) as u32;
            Some((mx, my + margin, mw, h))
        }
        QuakeEdge::Bottom => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let h = ((mh as f32) * frac) as u32;
            #[allow(clippy::cast_possible_wrap)]
            let y = my + (mh as i32) - (h as i32) - margin;
            Some((mx, y, mw, h))
        }
        QuakeEdge::Left => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let w = ((mw as f32) * frac) as u32;
            Some((mx + margin, my, w, mh))
        }
        QuakeEdge::Right => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let w = ((mw as f32) * frac) as u32;
            #[allow(clippy::cast_possible_wrap)]
            let x = mx + (mw as i32) - (w as i32) - margin;
            Some((x, my, w, mh))
        }
    }
}
