//! Quake drop-down terminal mode — docking edge, animation, and display.

use crate::window::{MonitorRect, WindowRect};
use crate::ConfigError;
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
#[derive(Default)]
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
    #[default]
    Slide,
    /// Bounce — like Slide but with a springy, sin-damped growth curve.
    Bounce,
    /// Scale — the window zooms from a point at the centre of the dock edge,
    /// interpolating both axes each frame.
    Scale,
    /// Fade — the window stays at its resting geometry and the whole-window
    /// opacity animates: Windows layered-window alpha, `NSWindow.alphaValue`
    /// on macOS, `_NET_WM_WINDOW_OPACITY` on Linux/X11 (which needs a
    /// compositing window manager — i.e. any modern desktop). A native Wayland
    /// surface has no equivalent, so it degrades to an instant show/hide there.
    Fade,
}

/// Timing curve for the Quake open/close animation.
///
/// The curve maps linear time onto animation progress. It is what decides
/// whether the drop-down *feels* snappy or sluggish at a given
/// `animation_ms` — a duration alone does not, because a curve that spends
/// most of its time near the end leaves the window crawling the last few
/// pixels long after the eye has stopped following it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum QuakeEasing {
    /// Mirror (default): ease-out cubic on open, ease-in cubic on close, so
    /// the close is exactly the open played backwards. This is what keeps a
    /// close from collapsing almost instantly and then creeping the last few
    /// pixels for the rest of the duration.
    #[default]
    Mirror,
    /// Ease-out cubic in both directions — fast off the mark, settling at the
    /// end. This was the behaviour before the mirror default; a close spends
    /// most of its time nearly finished.
    EaseOut,
    /// Ease-in-out cubic in both directions — gentle at both ends.
    EaseInOut,
    /// Constant speed, no easing.
    Linear,
}

impl QuakeEasing {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [Self::Mirror, Self::EaseOut, Self::EaseInOut, Self::Linear]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mirror => "Mirror (open eases out, close eases in)",
            Self::EaseOut => "Ease out",
            Self::EaseInOut => "Ease in-out",
            Self::Linear => "Linear",
        }
    }

    /// Map linear progress `t` (0..=1) onto eased progress for one direction.
    ///
    /// `showing` selects the direction: an open interpolates from the
    /// collapsed rect to the resting rect, a close does the reverse, so
    /// [`Self::Mirror`] flips the curve to keep the two motions time-reverses
    /// of each other.
    #[must_use]
    pub fn apply(self, t: f32, showing: bool) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let ease_out = |t: f32| 1.0 - (1.0 - t).powi(3);
        let ease_in = |t: f32| t.powi(3);
        let ease_in_out = |t: f32| {
            if t < 0.5 {
                4.0 * t.powi(3)
            } else {
                1.0 - (-2.0f32).mul_add(t, 2.0).powi(3) / 2.0
            }
        };
        match self {
            Self::Mirror => {
                if showing {
                    ease_out(t)
                } else {
                    ease_in(t)
                }
            }
            Self::EaseOut => ease_out(t),
            Self::EaseInOut => ease_in_out(t),
            Self::Linear => t,
        }
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
#[derive(Default)]
pub enum QuakeEdge {
    /// No docking — Quake is a free-floating window with exact-geometry
    /// restore on show/hide (the previous default behaviour).
    #[default]
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

/// Which monitor the Quake terminal docks on. `Current` (default) keeps
/// the window anchored to its own monitor — show/hide always happens on
/// the monitor the window was last visible on, and dragging the window to
/// another monitor re-anchors it there. `Pointer` puts it wherever the mouse
/// is. `Primary` always uses the OS-designated primary; `Index(n)` pins it to
/// the n-th enumerated monitor (the order winit returns from
/// `available_monitors()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum QuakeDisplay {
    /// Use the monitor the Quake window is currently sitting on (or, if
    /// it's hidden, the one it was last visible on). Drag the window to a
    /// different monitor to re-anchor the toggle there.
    #[default]
    Current,
    /// Use the monitor the mouse pointer is on at the moment you press the
    /// hotkey, wherever the window was before.
    ///
    /// This is the behaviour people usually mean by a drop-down terminal: it
    /// opens where you are looking. The trade-off, and the reason it is not the
    /// default, is that the window moves between monitors on its own — which
    /// is disorienting if you keep the drop-down parked somewhere deliberately.
    ///
    /// Needs a way to ask the system where the pointer is. X11 answers; Wayland
    /// does not tell a client the global pointer position, and there this falls
    /// back to [`Self::Current`] — the same boundary that already stops a
    /// Wayland client from placing its own window at all.
    Pointer,
    /// Always use the OS primary monitor.
    Primary,
    /// Pin to a specific 0-based monitor index.
    Index(u8),
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
    /// Timing curve for the open/close animation. Defaults to
    /// [`QuakeEasing::Mirror`].
    pub easing: QuakeEasing,
    /// Frame rate the open/close animation is driven at, in frames per second.
    /// Clamped to `15..=240` when applied.
    ///
    /// This is a **ceiling, not a target**: the animation pump runs from the
    /// event loop's idle callback, which fires on every event batch, not on a
    /// timer. Without a cap, the window-resize events the animation itself
    /// generates wake the loop again immediately and the pump re-renders
    /// hundreds of times a second — measured at 327 fps for a single 350 ms
    /// close on X11 — which saturates the compositor and makes the animation
    /// look *slower*, not smoother. Default: `60`.
    pub animation_fps: u32,
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
    /// When the app is closed while the Quake window is showing, reopen in
    /// Quake mode on the next launch (on the same monitor). Only consulted when
    /// `window.restore_session` is active. Default: `true`.
    pub restore_visible: bool,
    /// Keep the Quake window visible on **every virtual desktop / workspace**,
    /// so it stays on screen when you switch desktop — no hide/show needed.
    /// macOS uses `NSWindowCollectionBehaviorCanJoinAllSpaces`; Windows pins the
    /// window through the virtual-desktop COM API. On some Windows builds the
    /// COM IIDs differ and pinning silently degrades to the active desktop only.
    /// Linux/X11 uses the EWMH `_NET_WM_DESKTOP` property; a native Wayland
    /// surface has no equivalent protocol and degrades to a no-op.
    /// Default: `false`.
    pub show_on_all_desktops: bool,
}

impl Default for QuakeConfig {
    fn default() -> Self {
        Self {
            animation: QuakeAnimation::default(),
            animation_ms: 120,
            easing: QuakeEasing::default(),
            animation_fps: 60,
            edge: QuakeEdge::default(),
            display: QuakeDisplay::default(),
            size_percent: 0.5,
            margin_px: 0,
            hide_on_focus_loss: false,
            restore_visible: true,
            show_on_all_desktops: false,
        }
    }
}

impl QuakeConfig {
    /// Validate field ranges.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `margin_px` is unreasonably
    /// large. `margin_px` is cast to `i32` in [`quake_dock_rect`] (physical
    /// pixel arithmetic against the monitor rect); a value above `i32::MAX`
    /// would silently bit-reinterpret to negative there, and even a merely
    /// huge-but-in-range value produces a nonsensical docked window. `2000`
    /// logical pixels is already far beyond any real edge gap.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.margin_px > 2000 {
            return Err(ConfigError::Invalid {
                field: "quake.margin_px",
                message: "must be at most 2000",
            });
        }
        if !(15..=240).contains(&self.animation_fps) {
            return Err(ConfigError::Invalid {
                field: "quake.animation_fps",
                message: "must be between 15 and 240",
            });
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_visible_defaults_on() {
        assert!(QuakeConfig::default().restore_visible);
    }

    #[test]
    fn pointer_display_round_trips_and_is_not_the_default() {
        let cfg: QuakeConfig = toml::from_str("display = \"pointer\"").expect("pointer must parse");
        assert_eq!(cfg.display, QuakeDisplay::Pointer);
        let back = toml::to_string(&cfg).expect("serialize");
        assert!(back.contains("display = \"pointer\""), "{back}");
        // Following the pointer moves the window between monitors on its own,
        // which is a deliberate choice rather than the behaviour to impose.
        assert_ne!(QuakeDisplay::default(), QuakeDisplay::Pointer);
        assert_eq!(QuakeDisplay::default(), QuakeDisplay::Current);
    }

    #[test]
    fn default_validates() {
        QuakeConfig::default()
            .validate()
            .expect("default QuakeConfig must validate");
    }

    #[test]
    fn margin_px_range_validates() {
        let mut cfg = QuakeConfig {
            margin_px: 2001,
            ..QuakeConfig::default()
        };
        assert!(cfg.validate().is_err(), "2001 must be rejected");
        cfg.margin_px = 2000;
        assert!(cfg.validate().is_ok(), "2000 (the max) must be accepted");
        cfg.margin_px = 0;
        assert!(cfg.validate().is_ok(), "0 (the default) must be accepted");
    }

    #[test]
    fn show_on_all_desktops_defaults_off() {
        assert!(!QuakeConfig::default().show_on_all_desktops);
    }

    #[test]
    fn restore_and_all_desktops_roundtrip() {
        let cfg = QuakeConfig {
            restore_visible: false,
            show_on_all_desktops: true,
            ..QuakeConfig::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: QuakeConfig = toml::from_str(&s).unwrap();
        assert!(!back.restore_visible);
        assert!(back.show_on_all_desktops);
    }

    #[test]
    fn easing_defaults_to_mirror_and_fps_to_60() {
        let c = QuakeConfig::default();
        assert_eq!(c.easing, QuakeEasing::Mirror);
        assert_eq!(c.animation_fps, 60);
    }

    #[test]
    fn easing_all_and_labels_are_distinct() {
        let all = QuakeEasing::all();
        assert_eq!(all.len(), 4);
        let mut seen = std::collections::HashSet::new();
        for e in all {
            assert!(seen.insert(e.label()), "duplicate label: {}", e.label());
        }
    }

    #[test]
    fn easing_endpoints_are_exact_in_both_directions() {
        // Whatever the curve, an animation must start where it starts and land
        // exactly on its target — otherwise the final frame snaps.
        for e in QuakeEasing::all() {
            for showing in [true, false] {
                assert!((e.apply(0.0, showing) - 0.0).abs() < 1e-6, "{e:?} at t=0");
                assert!((e.apply(1.0, showing) - 1.0).abs() < 1e-6, "{e:?} at t=1");
            }
        }
    }

    #[test]
    fn easing_is_monotonic_and_bounded() {
        for e in QuakeEasing::all() {
            for showing in [true, false] {
                let mut prev = 0.0_f32;
                for i in 0..=100 {
                    #[allow(clippy::cast_precision_loss)]
                    let v = e.apply(i as f32 / 100.0, showing);
                    assert!((0.0..=1.0).contains(&v), "{e:?} out of range: {v}");
                    assert!(v >= prev - 1e-6, "{e:?} went backwards at t={i}");
                    prev = v;
                }
            }
        }
    }

    #[test]
    fn mirror_close_is_the_open_played_backwards() {
        // The whole point of the default curve: progress on a close at time t
        // is the complement of progress on an open at time 1 - t. Easing a
        // close *out*, as the code used to, made it collapse almost at once and
        // then creep the last pixels for the rest of the duration.
        let e = QuakeEasing::Mirror;
        for i in 0..=100 {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 / 100.0;
            let open = e.apply(t, true);
            let close = e.apply(1.0 - t, false);
            assert!(
                (open - (1.0 - close)).abs() < 1e-5,
                "not a mirror at t={t}: open={open} close={close}"
            );
        }
    }

    #[test]
    fn mirror_close_keeps_the_window_visible_past_the_halfway_point() {
        // Regression guard for the reported "closing animation is slow": under
        // the old ease-out close the window was down to 14 % of its height at
        // half the duration and spent the remaining half crawling the last few
        // pixels. A close must still be most of the way up at the midpoint.
        let progress = QuakeEasing::Mirror.apply(0.5, false);
        assert!(
            progress < 0.25,
            "close is {progress} done at the halfway point; it should still be near full size"
        );
    }

    #[test]
    fn ease_out_preserves_the_previous_behaviour() {
        // Kept as an explicit opt-in, so a user who preferred the old feel can
        // still ask for it.
        let e = QuakeEasing::EaseOut;
        assert!((e.apply(0.5, true) - e.apply(0.5, false)).abs() < 1e-6);
        assert!(e.apply(0.5, false) > 0.8);
    }

    #[test]
    fn animation_fps_out_of_range_is_rejected() {
        for bad in [0_u32, 14, 241, 10_000] {
            let c = QuakeConfig {
                animation_fps: bad,
                ..QuakeConfig::default()
            };
            assert!(c.validate().is_err(), "{bad} fps should not validate");
        }
        for ok in [15_u32, 60, 144, 240] {
            let c = QuakeConfig {
                animation_fps: ok,
                ..QuakeConfig::default()
            };
            assert!(c.validate().is_ok(), "{ok} fps should validate");
        }
    }

    #[test]
    fn easing_and_fps_round_trip_through_toml() {
        let c = QuakeConfig {
            easing: QuakeEasing::EaseInOut,
            animation_fps: 144,
            ..QuakeConfig::default()
        };
        let s = toml::to_string(&c).unwrap();
        let back: QuakeConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.easing, QuakeEasing::EaseInOut);
        assert_eq!(back.animation_fps, 144);
    }

    #[test]
    fn a_config_without_the_new_keys_still_loads() {
        // Existing user configs predate `easing`/`animation_fps`; they must keep
        // loading with the defaults rather than falling back wholesale.
        let back: QuakeConfig =
            toml::from_str("animation = \"bounce\"\nanimation_ms = 200\n").unwrap();
        assert_eq!(back.animation, QuakeAnimation::Bounce);
        assert_eq!(back.animation_ms, 200);
        assert_eq!(back.easing, QuakeEasing::Mirror);
        assert_eq!(back.animation_fps, 60);
    }
}
