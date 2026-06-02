//! Quake-mode show/hide animation, window-geometry helpers, and related
//! pure-math utilities (lerp, offscreen rect, rects_close, etc.).
//!
//! All functions here operate either on plain geometry types
//! (`terminale_config::WindowRect`, `winit::window::Window`) or on the
//! shared `RunningState` alias for `TermWindow` defined in `main.rs`.

use crate::{QuakeAnim, RunningState};
use winit::window::Window;

// ── apply_window_level / toggle_stay_on_top ─────────────────────────────────

/// Set the OS window level from the "stay on top" flag. `true` pins the
/// window above all others; `false` returns it to the normal stacking
/// order. Centralised so the creation path, the settings live-apply path,
/// and the runtime quick-toggle all behave identically.
pub(crate) fn apply_window_level(window: &Window, always_on_top: bool) {
    let level = if always_on_top {
        winit::window::WindowLevel::AlwaysOnTop
    } else {
        winit::window::WindowLevel::Normal
    };
    window.set_window_level(level);
}

/// Flip the runtime "stay on top" state, apply it to the OS window level,
/// and park the new value for the App to persist + sync into the settings
/// window (mirrors the live-zoom `pending_font_size` mechanism). Shared by
/// the keyboard shortcut, command palette, and right-click menu.
pub(crate) fn toggle_stay_on_top(state: &mut RunningState) {
    let on = !state.always_on_top;
    state.always_on_top = on;
    apply_window_level(&state.window, on);
    state.pending_always_on_top = Some(on);
    state.window.request_redraw();
}

/// Toggle borderless full-screen (F11 convention).
///
/// When currently windowed/maximized, requests borderless full-screen on the
/// current monitor. When already full-screen, restores the windowed state.
/// The prior window geometry is handled automatically by winit — passing
/// `None` to `set_fullscreen` restores whatever geometry was in use before.
pub(crate) fn toggle_fullscreen(state: &mut RunningState) {
    use winit::window::Fullscreen;
    let is_fs = state.window.fullscreen().is_some();
    if is_fs {
        state.window.set_fullscreen(None);
    } else {
        state
            .window
            .set_fullscreen(Some(Fullscreen::Borderless(None)));
    }
    state.window.request_redraw();
}

/// Toggle broadcast-input mode for the focused window.
///
/// When broadcast is **on** every keypress forwarded to the focused pane is
/// simultaneously written (raw bytes) to every other pane whose PTY is still
/// alive, within the scope set by `config.terminal.broadcast_scope`. A tinted
/// border is drawn around the receiving panes so the mode is always visible.
///
/// Toggling off clears the border immediately.
pub(crate) fn toggle_broadcast_input(state: &mut RunningState) {
    state.broadcast_input = !state.broadcast_input;
    state.window.request_redraw();
}

/// Toggle zen (distraction-free) mode.
///
/// When activated the chrome elements named in `config.window.zen_hide` are
/// suppressed without mutating the user's config values:
/// - `tab_bar`     → renderer treats the tab bar as disabled
/// - `status_bar`  → renderer clears the status-bar strip
/// - `pane_headers`→ renderer hides per-pane header strips
/// - `title_bar`   → renderer hides the custom title bar
///
/// When `config.window.zen_fullscreen` is true, the window also enters
/// borderless full-screen. Exiting zen mode restores the prior chrome
/// visibility and, when zen entered full-screen, restores the windowed state.
pub(crate) fn toggle_zen_mode(state: &mut RunningState) {
    if state.zen {
        // ── Exit zen ─────────────────────────────────────────────────────────
        state.zen = false;
        // Restore full-screen only if zen entered it (don't exit FS when the
        // user was already in FS before zen activated).
        if !state.zen_was_fullscreen {
            state.window.set_fullscreen(None);
        }
        // Restore chrome: re-apply the config values that zen had overridden.
        apply_zen_chrome(state);
    } else {
        // ── Enter zen ────────────────────────────────────────────────────────
        // Capture whether full-screen is active NOW so we know to restore it.
        state.zen_was_fullscreen = state.window.fullscreen().is_some();
        state.zen = true;
        // Apply chrome overrides from zen_hide.
        apply_zen_chrome(state);
        if state.config_zen_fullscreen() && !state.zen_was_fullscreen {
            use winit::window::Fullscreen;
            state
                .window
                .set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
    }
    state.window.request_redraw();
}

/// Internal helper: push the current zen state into the renderer so chrome
/// is either shown (zen off, user config wins) or hidden (zen on, overrides).
///
/// Called both when zen toggles and when the user changes `zen_hide` in
/// Settings while zen is active (so changes take effect immediately).
pub(crate) fn apply_zen_chrome(state: &mut RunningState) {
    use terminale_config::ZenHideElement;
    let hide = state.config_zen_hide();

    // The custom title-bar chrome lives inside the tab-bar strip (the window
    // uses `with_decorations(false)`, so window controls are rendered there).
    // Hiding `TitleBar` therefore also suppresses the tab-bar strip.
    let hide_tab_bar = state.zen
        && hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::TabBar | ZenHideElement::TitleBar));
    let hide_status_bar = state.zen && hide.iter().any(|e| matches!(e, ZenHideElement::StatusBar));
    let hide_pane_headers = state.zen
        && hide
            .iter()
            .any(|e| matches!(e, ZenHideElement::PaneHeaders));

    // Tab bar: pass `false` to the renderer when zen hides it; otherwise
    // restore the configured value.
    let tab_bar_enabled = if hide_tab_bar {
        false
    } else {
        state.config_tab_bar_enabled()
    };
    state.renderer.set_tab_bar_enabled(tab_bar_enabled);

    // Status bar: blank it immediately when zen hides it. The App's normal
    // status-bar tick will repopulate it on exit once zen is off.
    if hide_status_bar {
        state.renderer.set_status_bar(None);
    }
    // (When restoring, the tick loop re-renders it automatically on the next
    // about_to_wait cycle — no action needed here.)

    // Pane headers.
    let show_headers = if hide_pane_headers {
        false
    } else {
        state.config_show_pane_headers()
    };
    state.renderer.set_show_pane_headers(show_headers);
    state.show_pane_headers = show_headers;
}

// ── Window reveal (cloak-around-show) ────────────────────────────────────────

/// Paint the first frame into the (still-hidden) window, then reveal it
/// without the white flash of an unpainted surface: on Windows we DWM-cloak
/// around `set_visible(true)` so the compositor never shows the blank window;
/// elsewhere the prior render already filled the surface before it maps.
///
/// Mirrors the AI/settings sub-windows' hidden-render-cloak-reveal sequence,
/// applied to the main terminal window (first window **and** torn-out ones).
pub(crate) fn reveal_window(state: &mut RunningState) {
    // Build the tab bar / overlays and paint the dark UI into the surface
    // while the window is still hidden.
    crate::render_main(state);
    #[cfg(windows)]
    set_dwm_cloak(&state.window, true);
    state.window.set_visible(true);
    #[cfg(windows)]
    set_dwm_cloak(&state.window, false);
    // Schedule one more redraw so the freshly-mapped surface re-presents the
    // painted frame (belt-and-braces on platforms that drop the first paint).
    state.window.request_redraw();
}

/// Toggle the DWM "cloak" on a window. A cloaked window stays mapped (so the
/// GPU surface keeps presenting) but is invisible to the compositor — letting
/// us flip `set_visible(true)` without the OS ever showing an unpainted
/// (white) frame. No-op on non-Windows and when the handle isn't Win32.
#[cfg(windows)]
pub(crate) fn set_dwm_cloak(window: &Window, cloaked: bool) {
    use std::ffi::c_void;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: *mut c_void, attr: u32, val: *const c_void, sz: u32) -> i32;
    }
    const DWMWA_CLOAK: u32 = 13;
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = h.hwnd.get() as *mut c_void;
    let value: i32 = i32::from(cloaked);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            std::ptr::from_ref::<i32>(&value) as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

// ── Ghost window click-through (Windows) ─────────────────────────────────────

/// Best-effort transparent click-through for the floating ghost window on
/// Windows: adds `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE` so
/// the cursor passes through the ghost down to whichever terminal window
/// it's hovering, and the ghost never steals focus. A failure here is
/// harmless — the OS-level mouse capture on the source window still
/// routes the drag's events correctly; the only visible side-effect would
/// be that the ghost is technically clickable while it lives.
#[cfg(target_os = "windows")]
pub(crate) fn set_click_through_windows(window: &Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, n_index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, n_index: i32, dw_new_long: isize) -> isize;
    }
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = h.hwnd.get();
    // SAFETY: hwnd is a live Win32 HWND owned by winit; GetWindowLongPtrW
    // and SetWindowLongPtrW are documented as safe to call with any valid
    // HWND from any thread.
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new = cur | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
    }
}

// ── ghost_window_position ────────────────────────────────────────────────────

/// Screen-space (physical px) top-left position the floating ghost window
/// should sit at, so the pill centred inside it tracks under the cursor at
/// the originally-grabbed offset. `grab_offset_x` is in LOGICAL px (the
/// same units the App captured it at lift time); we apply the window's
/// `scale_factor` to translate it to physical px.
pub(crate) fn ghost_window_position(
    cursor_screen: (i32, i32),
    scale: f32,
    grab_offset_x: f32,
    inner_w_px: u32,
    inner_h_px: u32,
) -> winit::dpi::PhysicalPosition<i32> {
    let grab_off_px = (grab_offset_x * scale) as i32;
    // The pill is centred inside the surface, so the window-top-left is
    // half a window short of where the pill centre needs to be.
    let x = cursor_screen.0 - (inner_w_px / 2) as i32 - grab_off_px;
    let y = cursor_screen.1 - (inner_h_px / 2) as i32;
    winit::dpi::PhysicalPosition::new(x, y)
}

// ── Window rect geometry ─────────────────────────────────────────────────────

/// Apply a `(x, y, w, h)` rect to the window.
///
/// `resize`: when `true` also calls `request_inner_size` (needed for the
/// `Scale` animation which changes size every frame). For `Slide`/`Bounce`
/// (position-only interpolation) pass `false` to avoid unnecessary surface
/// resize round-trips that can cause flicker on Windows ConPTY.
///
/// `set_outer_position` is always called so the window moves each frame.
pub(crate) fn apply_window_rect(window: &Window, rect: terminale_config::WindowRect, resize: bool) {
    let (x, y, w, h) = rect;
    if resize {
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
    }
    window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
}

/// macOS: dock the window to a screen edge using the **native**
/// `NSWindow.setFrame` against `NSScreen.visibleFrame`.
///
/// winit's `set_outer_position` is unusable for docking here: it double-counts
/// the menu-bar height (a window asked for y=0 lands ~2× the menu-bar height
/// below the top) and it can't place a window in the menu-bar band at all, so a
/// top/left/right dock always shows an empty strip above the window. Going
/// straight to AppKit with the screen's *visible* frame (which already excludes
/// the menu bar and the Dock) places the window flush. `animate` runs AppKit's
/// built-in frame animation — smooth, unlike winit per-frame repositioning.
///
/// Returns `false` (caller should fall back to the winit path) when the edge is
/// `Off`, the handle isn't AppKit, or the NSWindow/NSScreen can't be read.
#[cfg(target_os = "macos")]
pub(crate) fn macos_dock_window(
    window: &Window,
    edge: terminale_config::QuakeEdge,
    size_percent: f32,
    margin_px: u32,
    animate: bool,
) -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use terminale_config::QuakeEdge;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NsPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NsSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NsRect {
        origin: NsPoint,
        size: NsSize,
    }
    // SAFETY: layouts match Cocoa's CGPoint/CGSize/CGRect (two/two/nested f64).
    unsafe impl objc2::Encode for NsPoint {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGPoint",
            &[
                <f64 as objc2::Encode>::ENCODING,
                <f64 as objc2::Encode>::ENCODING,
            ],
        );
    }
    unsafe impl objc2::Encode for NsSize {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGSize",
            &[
                <f64 as objc2::Encode>::ENCODING,
                <f64 as objc2::Encode>::ENCODING,
            ],
        );
    }
    unsafe impl objc2::Encode for NsRect {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CGRect",
            &[
                <NsPoint as objc2::Encode>::ENCODING,
                <NsSize as objc2::Encode>::ENCODING,
            ],
        );
    }

    if matches!(edge, QuakeEdge::Off) {
        return false;
    }
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return false;
    };
    // SAFETY: on AppKit the handle's `ns_view` is a valid `NSView*` for the
    // window's lifetime; `-window` returns its owning `NSWindow*` (or nil).
    let ns_window: *mut AnyObject = unsafe {
        let view: *mut AnyObject = appkit.ns_view.as_ptr().cast();
        msg_send![view, window]
    };
    if ns_window.is_null() {
        return false;
    }
    // SAFETY: `-screen` returns the window's NSScreen or nil.
    let ns_screen: *mut AnyObject = unsafe { msg_send![ns_window, screen] };
    if ns_screen.is_null() {
        return false;
    }
    // `visibleFrame` is Cocoa coords: origin bottom-left, y grows up, and it
    // already excludes the menu bar (top) and the Dock.
    // SAFETY: `-visibleFrame` returns an NSRect (4×f64).
    let vf: NsRect = unsafe { msg_send![ns_screen, visibleFrame] };
    let (vx, vy, vw, vh) = (vf.origin.x, vf.origin.y, vf.size.width, vf.size.height);
    let frac = f64::from(size_percent).clamp(0.1, 1.0);
    let m = f64::from(margin_px);
    let rect = match edge {
        QuakeEdge::Top => {
            let h = vh * frac;
            // Flush under the menu bar = top of the visible band, minus margin.
            NsRect {
                origin: NsPoint {
                    x: vx,
                    y: vy + vh - h - m,
                },
                size: NsSize {
                    width: vw,
                    height: h,
                },
            }
        }
        QuakeEdge::Bottom => {
            let h = vh * frac;
            NsRect {
                origin: NsPoint { x: vx, y: vy + m },
                size: NsSize {
                    width: vw,
                    height: h,
                },
            }
        }
        QuakeEdge::Left => {
            let w = vw * frac;
            NsRect {
                origin: NsPoint { x: vx + m, y: vy },
                size: NsSize {
                    width: w,
                    height: vh,
                },
            }
        }
        QuakeEdge::Right => {
            let w = vw * frac;
            NsRect {
                origin: NsPoint {
                    x: vx + vw - w - m,
                    y: vy,
                },
                size: NsSize {
                    width: w,
                    height: vh,
                },
            }
        }
        QuakeEdge::Off => return false,
    };
    // SAFETY: setFrame:display:animate: is a standard NSWindow method.
    unsafe {
        let _: () =
            msg_send![ns_window, setFrame: rect, display: Bool::YES, animate: Bool::new(animate)];
    }
    true
}

/// macOS: bring the app to the foreground so a Quake show triggered from
/// another app (or Space) actually takes keyboard focus. winit's
/// `focus_window` alone does not activate a background app.
#[cfg(target_os = "macos")]
pub(crate) fn macos_activate() {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    // SAFETY: standard NSApplication activation call on the shared app.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if !app.is_null() {
            let _: () = msg_send![app, activateIgnoringOtherApps: Bool::YES];
        }
    }
}

/// Snap the focused window to an edge / centre / full of its **current**
/// monitor, using the pure [`terminale_config::snap_window_rect`] math. No-op
/// when no monitor can be queried. Cancels Quake animation state so the snap
/// position sticks.
pub(crate) fn snap_window(state: &mut RunningState, edge: terminale_config::SnapEdge) {
    let Some(mon) = state
        .window
        .current_monitor()
        .or_else(|| state.window.primary_monitor())
        .or_else(|| state.window.available_monitors().next())
    else {
        return;
    };
    let mpos = mon.position();
    let msize = mon.size();
    let size = state.window.inner_size();
    let pos = state.window.outer_position().unwrap_or_default();
    let rect = terminale_config::snap_window_rect(
        (mpos.x, mpos.y, msize.width, msize.height),
        edge,
        (pos.x, pos.y, size.width, size.height),
    );
    // A snap supersedes any in-flight Quake slide. If a Fade was mid-flight,
    // restore full opacity — the cancelled animation would otherwise leave
    // the window semi-transparent.
    if state.quake_anim.is_some() {
        set_window_alpha(&state.window, 255);
    }
    state.quake_anim = None;
    // An explicit user re-position also supersedes any remembered floating
    // geometry: the next Quake show must re-dock from the current monitor,
    // not replay a stale quake_user_rect captured by a title-bar un-dock.
    state.quake_user_rect = None;
    // The snapped rect is the new docked baseline so a later title-bar drag
    // still detects the un-dock against it (maybe_undock_quake_on_drag
    // compares the live geometry to quake_last_dock_rect). quake_pre_dock_rect
    // is left untouched on purpose — it holds the genuine pre-dock floating
    // size a future drag should restore.
    state.quake_last_dock_rect = Some(rect);
    apply_window_rect(&state.window, rect, true);
    state.window.request_redraw();
}

// ── Snap-layout chooser ───────────────────────────────────────────────────────

/// Open the snap-layout chooser overlay. Sets the renderer state and the
/// `snap_chooser_open` flag so mouse/keyboard handlers can route to it.
pub(crate) fn open_snap_chooser(state: &mut RunningState) {
    state.snap_chooser_open = true;
    state
        .renderer
        .set_snap_chooser(Some(terminale_render::SnapChooserOverlay { hovered: None }));
    state.window.request_redraw();
}

/// Close the snap-layout chooser overlay without applying any snap.
pub(crate) fn close_snap_chooser(state: &mut RunningState) {
    state.snap_chooser_open = false;
    state.renderer.set_snap_chooser(None);
    state.window.request_redraw();
}

/// Apply the snap layout at `cell_idx` (into [`terminale_render::SNAP_CHOOSER_CELLS`])
/// and close the chooser.
pub(crate) fn snap_chooser_apply(state: &mut RunningState, cell_idx: usize) {
    use terminale_render::{SnapChooserCell, SNAP_CHOOSER_CELLS};
    let Some(&cell) = SNAP_CHOOSER_CELLS.get(cell_idx) else {
        return;
    };
    let edge = match cell {
        SnapChooserCell::Left => terminale_config::SnapEdge::Left,
        SnapChooserCell::Right => terminale_config::SnapEdge::Right,
        SnapChooserCell::Top => terminale_config::SnapEdge::Top,
        SnapChooserCell::Bottom => terminale_config::SnapEdge::Bottom,
        SnapChooserCell::TopLeft => terminale_config::SnapEdge::TopLeft,
        SnapChooserCell::TopRight => terminale_config::SnapEdge::TopRight,
        SnapChooserCell::BottomLeft => terminale_config::SnapEdge::BottomLeft,
        SnapChooserCell::BottomRight => terminale_config::SnapEdge::BottomRight,
        SnapChooserCell::Center => terminale_config::SnapEdge::Center,
        SnapChooserCell::Maximize => terminale_config::SnapEdge::Maximize,
    };
    close_snap_chooser(state);
    snap_window(state, edge);
}

// ── rects_close / lerp ───────────────────────────────────────────────────────

/// Whether two window rects are within `tol` pixels on every component.
/// Used to tell whether the user nudged/resized a docked Quake window away
/// from the dock rect we applied (vs. it sitting exactly where we put it).
pub(crate) fn rects_close(
    a: terminale_config::WindowRect,
    b: terminale_config::WindowRect,
    tol: i32,
) -> bool {
    (a.0 - b.0).abs() <= tol
        && (a.1 - b.1).abs() <= tol
        && (i64::from(a.2) - i64::from(b.2)).abs() <= i64::from(tol)
        && (i64::from(a.3) - i64::from(b.3)).abs() <= i64::from(tol)
}

/// Chrome-style un-dock on title-bar drag. Called right before a title-bar
/// `drag_window()`: when the Quake window is currently sitting AT its dock
/// geometry, shrink it back to the floating size it had before the first
/// dock (`quake_pre_dock_rect`), repositioning so the grabbed title-bar
/// point stays under the cursor proportionally — exactly like dragging a
/// maximized browser window. The restored geometry is recorded as the
/// user-adjusted rect so subsequent hide/show cycles keep it instead of
/// re-docking.
pub(crate) fn maybe_undock_quake_on_drag(state: &mut RunningState, cursor_px: (f32, f32)) {
    // Only while the Quake window is shown in dock mode at the dock
    // geometry. A present user rect means it is already un-docked; no
    // animation may be in flight (a mid-slide rect is not "docked").
    if !state.quake_visible || state.quake_user_rect.is_some() || state.quake_anim.is_some() {
        return;
    }
    let Some(dock) = state.quake_last_dock_rect else {
        return;
    };
    let Some(pre) = state.quake_pre_dock_rect else {
        return;
    };
    let pos = state.window.outer_position().unwrap_or_default();
    let size = state.window.inner_size();
    let cur = (pos.x, pos.y, size.width, size.height);
    // Bail unless the window actually sits at the dock rect (tolerance for
    // DWM nudging it by a few px).
    if !rects_close(cur, dock, 12) {
        return;
    }
    // Nothing to restore if the pre-dock geometry is degenerate or already
    // matches the dock size.
    if pre.2 == 0 || pre.3 == 0 || rects_close(pre, cur, 12) {
        return;
    }
    // Keep the grabbed point under the cursor proportionally on the x axis;
    // keep the title bar (window top) at its current screen height so the
    // cursor stays on it when the OS drag takes over.
    #[allow(clippy::cast_possible_truncation)]
    let new_x = (f64::from(pos.x) + f64::from(cursor_px.0)
        - f64::from(cursor_px.0) * f64::from(pre.2) / f64::from(size.width.max(1)))
    .round() as i32;
    let target = (new_x, pos.y, pre.2, pre.3);
    apply_window_rect(&state.window, target, true);
    state.quake_user_rect = Some(target);
    tracing::debug!(?dock, ?pre, ?target, "quake: un-docked on title-bar drag");
}

/// Linear-interpolate **both position and size** between two rects.
/// Used by every geometric Quake animation (Slide/Bounce/Scale): the reveal
/// grows the window from a collapsed rect at the dock edge to the full
/// target, so size always interpolates alongside position.
pub(crate) fn lerp_rect_full(
    a: terminale_config::WindowRect,
    b: terminale_config::WindowRect,
    t: f32,
) -> terminale_config::WindowRect {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lerp_i = |s: i32, e: i32| s + ((e - s) as f32 * t) as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lerp_u = |s: u32, e: u32| (s as f32 + (e as f32 - s as f32) * t).round() as u32;
    (
        lerp_i(a.0, b.0),
        lerp_i(a.1, b.1),
        lerp_u(a.2, b.2),
        lerp_u(a.3, b.3),
    )
}

// ── collapsed_edge_rect / scale_origin_rect ──────────────────────────────────

/// Compute the collapsed start rect for a Quake show animation (and the
/// collapsed end rect for a hide animation) for the Slide/Bounce reveal.
///
/// The window is collapsed to a 1-px strip **at the dock edge, inside the
/// monitor** — the docked edge stays pinned and only the perpendicular extent
/// animates. Unlike the old fully-past-the-edge translation, no interpolated
/// frame ever leaves the monitor, so a display stacked above/beside never
/// shows the window mid-slide. For `QuakeEdge::Off` or when `mon_rect` is
/// unavailable, the window collapses in place at the target's own top edge.
///
/// Unit-testable: no RunningState dependencies.
pub(crate) fn collapsed_edge_rect(
    edge: terminale_config::QuakeEdge,
    mon_rect: Option<terminale_config::MonitorRect>,
    target: terminale_config::WindowRect,
) -> terminale_config::WindowRect {
    use terminale_config::QuakeEdge;
    let (tx, ty, tw, th) = target;

    match (edge, mon_rect) {
        // Top-docked: top edge pinned at the monitor top, height collapsed.
        (QuakeEdge::Top, Some((_, my, _, _))) => (tx, my, tw, 1),
        // Bottom-docked: bottom edge pinned at the monitor bottom.
        (QuakeEdge::Bottom, Some((_, my, _, mh))) => {
            #[allow(clippy::cast_possible_wrap)]
            let y = my + mh as i32 - 1;
            (tx, y, tw, 1)
        }
        // Left-docked: left edge pinned, width collapsed.
        (QuakeEdge::Left, Some((mx, _, _, _))) => (mx, ty, 1, th),
        // Right-docked: right edge pinned.
        (QuakeEdge::Right, Some((mx, _, mw, _))) => {
            #[allow(clippy::cast_possible_wrap)]
            let x = mx + mw as i32 - 1;
            (x, ty, 1, th)
        }
        // Free-floating or no monitor info: collapse in place (top edge of
        // the target rect) — never translate off the visible area.
        _ => (tx, ty, tw, 1),
    }
}

/// Collapsed start/end rect for the `Scale` animation: a 1×1 point at the
/// **centre of the dock edge**, so the window zooms in/out from that point
/// (both axes), staying inside the monitor the whole time. Distinguishes
/// Scale visually from the axis-only Slide reveal.
pub(crate) fn scale_origin_rect(
    edge: terminale_config::QuakeEdge,
    mon_rect: Option<terminale_config::MonitorRect>,
    target: terminale_config::WindowRect,
) -> terminale_config::WindowRect {
    use terminale_config::QuakeEdge;
    let (tx, ty, tw, th) = target;
    #[allow(clippy::cast_possible_wrap)]
    let (cx, cy) = (tx + (tw / 2) as i32, ty + (th / 2) as i32);

    match (edge, mon_rect) {
        (QuakeEdge::Top, Some((_, my, _, _))) => (cx, my, 1, 1),
        (QuakeEdge::Bottom, Some((_, my, _, mh))) => {
            #[allow(clippy::cast_possible_wrap)]
            let y = my + mh as i32 - 1;
            (cx, y, 1, 1)
        }
        (QuakeEdge::Left, Some((mx, _, _, _))) => (mx, cy, 1, 1),
        (QuakeEdge::Right, Some((mx, _, mw, _))) => {
            #[allow(clippy::cast_possible_wrap)]
            let x = mx + mw as i32 - 1;
            (x, cy, 1, 1)
        }
        // Free-floating: zoom from the target's top-centre.
        _ => (cx, ty, 1, 1),
    }
}

/// Pick the animation's collapsed rest rect for the given style. `Fade`
/// keeps the full target geometry (only opacity animates); `None` is
/// handled by the callers (instant).
pub(crate) fn anim_rest_rect(
    kind: terminale_config::QuakeAnimation,
    edge: terminale_config::QuakeEdge,
    mon_rect: Option<terminale_config::MonitorRect>,
    target: terminale_config::WindowRect,
) -> terminale_config::WindowRect {
    use terminale_config::QuakeAnimation;
    match kind {
        QuakeAnimation::Scale => scale_origin_rect(edge, mon_rect, target),
        QuakeAnimation::Fade => target,
        _ => collapsed_edge_rect(edge, mon_rect, target),
    }
}

// ── set_window_alpha ─────────────────────────────────────────────────────────

/// Set the whole-window opacity (0 = fully transparent, 255 = opaque) for
/// the `Fade` Quake animation. Windows-only: flips `WS_EX_LAYERED` on and
/// drives `SetLayeredWindowAttributes`; at `alpha == 255` the layered bit is
/// removed again so the window returns to the normal (non-layered)
/// presentation path. No-op on other platforms (Fade degrades to instant).
#[cfg(target_os = "windows")]
pub(crate) fn set_window_alpha(window: &Window, alpha: u8) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, n_index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, n_index: i32, dw_new_long: isize) -> isize;
        fn SetLayeredWindowAttributes(hwnd: isize, color: u32, alpha: u8, flags: u32) -> i32;
    }
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const LWA_ALPHA: u32 = 0x0000_0002;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = h.hwnd.get();
    // SAFETY: hwnd is a live Win32 HWND owned by winit; these user32 calls
    // are documented as safe with any valid HWND.
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if alpha == 255 {
            // Fully opaque: restore the normal presentation path.
            SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, cur & !WS_EX_LAYERED);
        } else {
            if cur & WS_EX_LAYERED == 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, cur | WS_EX_LAYERED);
            }
            SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_window_alpha(_window: &Window, _alpha: u8) {}

// ── refresh_quake_last_monitor ────────────────────────────────────────────────

/// Refresh the cached `quake_last_monitor` while the window is visible.
///
/// This must be called on every event that can indicate the window has
/// changed monitors (cursor movement during a tab drag, `WindowEvent::Moved`,
/// `Focused(true)`, `Resized`, `ScaleFactorChanged`, `MouseInput::Pressed`).
///
/// # Why skip hidden windows
///
/// Once a Quake window is hidden its position rect is still in memory at
/// wherever it last was.  `Window::current_monitor()` uses that rect to
/// determine the monitor — usually correct, but not guaranteed.  We already
/// snapshot the monitor at hide-time in `toggle_quake`, so there's nothing
/// useful to update while hidden.
///
/// # Cost
///
/// One `Window::current_monitor()` call (winit caches monitor handles) plus
/// one `Option<String>` comparison.  On the hot `CursorMoved` path we skip
/// immediately when the window is not visible.
pub(crate) fn refresh_quake_last_monitor(state: &mut RunningState) {
    // Skip when the window is hidden — the OS-parked rect may be stale.
    if !state.quake_visible {
        return;
    }
    // Skip while a slide is in flight: every animation frame repositions the
    // window, which fires `Moved`, which lands here — and `current_monitor()`
    // is a non-trivial OS round-trip on macOS. Doing it 60×/s mid-slide just
    // adds stutter; the monitor can't change during the animation anyway, and
    // it's refreshed again on the next real `Moved`/`Focused`/`Resized`.
    if state.quake_anim.is_some() {
        return;
    }
    let new_mon = state.window.current_monitor();
    // Only write when the handle has actually changed (name-based comparison
    // because `MonitorHandle` does not implement `PartialEq`).
    let new_name = new_mon
        .as_ref()
        .and_then(winit::monitor::MonitorHandle::name);
    let old_name = state
        .quake_last_monitor
        .as_ref()
        .and_then(winit::monitor::MonitorHandle::name);
    if new_name != old_name {
        state.quake_last_monitor = new_mon;
    }
}

// ── toggle_quake ─────────────────────────────────────────────────────────────

/// Toggle Quake mode. Two behaviours depending on `quake_cfg.edge`:
///
/// * `Off` (default): a pure show/hide — the window's exact geometry
///   (outer position + inner size) is snapshotted on hide and restored
///   on show, so Quake reappears wherever the user last left it.
/// * `Top` / `Bottom` / `Left` / `Right` (edge docking): the window
///   snaps to that edge of the chosen monitor on every show, sized as
///   `size_percent` of the perpendicular extent and inset by `margin_px`
///   along the dock axis. The chosen monitor follows `quake_cfg.display`
///   — current / primary / specific index.
///
/// The transition runs through the configured `animation`:
/// `Slide`/`Bounce`/`Scale` interpolate the OS **window geometry**
/// (sliding the window in/out from the dock edge); `None` is instant.
/// All in-content shader overlays have been removed.
pub(crate) fn toggle_quake(state: &mut RunningState, quake_cfg: &terminale_config::QuakeConfig) {
    use terminale_config::QuakeAnimation;
    let animated = !matches!(quake_cfg.animation, QuakeAnimation::None);
    let dur = std::time::Duration::from_millis(u64::from(quake_cfg.animation_ms.clamp(0, 2000)));

    // The Quake hotkey combo is consumed by the OS (WM_HOTKEY), so winit never
    // delivers the modifier release while the window is hidden. Clear any stale
    // modifier state here too (belt-and-braces with the Focused handler) so the
    // first keypress after a toggle is typed verbatim instead of being read as a
    // Ctrl/Alt+<key> shortcut.
    state.modifiers = winit::keyboard::ModifiersState::empty();

    if state.quake_visible {
        // macOS dock modes are positioned natively (see the show path); hide
        // them with a plain `set_visible(false)` rather than the winit slide,
        // which would reposition frame-by-frame with the menu-bar coordinate
        // bug and stutter. The next show re-docks from the visible frame.
        #[cfg(target_os = "macos")]
        if quake_cfg.edge != terminale_config::QuakeEdge::Off {
            state.quake_visible = false;
            state.quake_anim = None;
            state.window.set_visible(false);
            return;
        }
        // Hiding — snapshot the exact geometry so the next show is a 1:1
        // restore. `outer_position` may fail on some platforms; fall back to
        // any saved rect, else origin.
        let size = state.window.inner_size();
        let pos = state.window.outer_position().unwrap_or_default();
        let mut rect = (pos.x, pos.y, size.width, size.height);
        // A toggle can land while the SHOW animation is still in flight; the
        // live window geometry is then an interpolated mid-slide frame, not
        // where the window rests. Saving it would corrupt the position
        // memory (and be misread as a user adjustment just below, since it
        // differs from the dock rect). Use the animation's resting target.
        if let Some(anim) = &state.quake_anim {
            if anim.showing {
                rect = anim.to;
            }
        }
        state.quake_saved_rect = Some(rect);
        // Dock mode: if the user moved/resized the window away from the dock
        // rect we last applied, remember that geometry so the next show
        // restores it verbatim instead of snapping back to the dock size.
        // (`quake_last_dock_rect` is `None` on the very first hide — before
        // the window has ever been docked — so the first show still docks.)
        if quake_cfg.edge != terminale_config::QuakeEdge::Off {
            if let Some(base) = state.quake_last_dock_rect {
                if !rects_close(base, rect, 6) {
                    state.quake_user_rect = Some(rect);
                }
            } else if state.quake_user_rect.is_some() {
                // Already persisting — keep tracking the latest geometry.
                state.quake_user_rect = Some(rect);
            }
        }
        // Snapshot the current monitor NOW while the window is still visible.
        // `Window::current_monitor()` is reliable only when the window is
        // on-screen; after hiding the rect may sit on the wrong monitor and
        // the call would return a stale result.  We use this snapshot in
        // `compute_quake_target` to resolve `QuakeDisplay::Current` correctly
        // across hide/show cycles. Skip the refresh while an animation is in
        // flight — the window may be mid-slide (even partially off-screen)
        // and would report the wrong monitor; the previous snapshot is the
        // accurate one in that case.
        if state.quake_anim.is_none() {
            state.quake_last_monitor = state.window.current_monitor();
        }
        state.quake_visible = false;

        if animated && dur.as_millis() > 0 {
            // Slide/Bounce: collapse the window onto the dock edge (reveal in
            // reverse); Scale: shrink to a point at the edge centre; Fade:
            // geometry stays put and only the opacity animates. Every variant
            // stays inside the monitor — no frame ever crosses onto a
            // neighbouring display.
            let mon_rect = compute_quake_target(state, quake_cfg).and_then(|(_, m)| m);
            let off = anim_rest_rect(quake_cfg.animation, quake_cfg.edge, mon_rect, rect);
            // Rapid-toggle: if a SHOW animation is still in flight, collapse
            // from the live (mid-reveal) geometry instead of jumping back to
            // the resting rect first. The SAVED rect above stays `anim.to`
            // (the resting geometry) — only the animation start differs.
            let from = if state.quake_anim.is_some() {
                let p = state.window.outer_position().unwrap_or_default();
                let s = state.window.inner_size();
                (p.x, p.y, s.width, s.height)
            } else {
                rect
            };
            state.quake_anim = Some(QuakeAnim {
                start: std::time::Instant::now(),
                duration: dur,
                showing: false,
                from,
                to: off,
                anim_kind: quake_cfg.animation,
            });
            state.window.request_redraw();
        } else {
            state.quake_anim = None;
            state.window.set_visible(false);
        }
        return;
    }

    // Showing — compute the target rect:
    //   edge == Off  → restore exact saved geometry (legacy behaviour);
    //   edge != Off  → compute from the selected monitor + size + margin.
    state.quake_visible = true;
    // Swallow keypresses for a short window after the show: when shown via the
    // global hotkey, the still-held trigger key (e.g. the "1" in Ctrl+Shift+1)
    // would otherwise leak into the shell once the window gains focus.
    state.quake_input_suppress_until =
        Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
    // Free-floating mode never docks, so any persisted dock geometry is
    // irrelevant — drop it so a later switch back to dock mode starts clean.
    if quake_cfg.edge == terminale_config::QuakeEdge::Off {
        state.quake_user_rect = None;
        state.quake_last_dock_rect = None;
        state.quake_pre_dock_rect = None;
    }
    let target_and_mon = compute_quake_target(state, quake_cfg);
    // Record the dock rect we're about to apply as the baseline for
    // detecting later user adjustments — but only when we actually docked
    // (no user-adjusted geometry is overriding it).
    if quake_cfg.edge != terminale_config::QuakeEdge::Off && state.quake_user_rect.is_none() {
        state.quake_last_dock_rect = target_and_mon.map(|(r, _)| r);
        // Capture the pre-dock floating geometry once, so a title-bar drag can
        // pop the window back out to that size (Chrome un-maximize style).
        if state.quake_pre_dock_rect.is_none() {
            state.quake_pre_dock_rect = state.quake_saved_rect;
        }
    }

    // Apply the window-wide always-on-top flag — Quake no longer overrides
    // it. `window.always_on_top` is the single source of truth for all
    // window modes including docked Quake.
    apply_window_level(&state.window, state.always_on_top);

    // macOS dock modes (top/bottom/left/right): position natively against the
    // screen's visible frame. winit's `set_outer_position` double-counts the
    // menu bar, leaving an empty strip above the window; AppKit's `setFrame`
    // places it flush and animates smoothly. Free-floating (`Off`) keeps the
    // winit path below.
    #[cfg(target_os = "macos")]
    if quake_cfg.edge != terminale_config::QuakeEdge::Off {
        state.window.set_visible(true);
        macos_activate();
        let want_anim = animated && dur.as_millis() > 0;
        if macos_dock_window(
            &state.window,
            quake_cfg.edge,
            quake_cfg.size_percent,
            quake_cfg.margin_px,
            want_anim,
        ) {
            state.quake_anim = None;
            state.window.focus_window();
            return;
        }
    }

    if let Some((rect, mon_rect)) = target_and_mon {
        if animated && dur.as_millis() > 0 {
            let is_fade = matches!(quake_cfg.animation, terminale_config::QuakeAnimation::Fade);
            // Begin collapsed at the dock edge (Slide/Bounce/Scale) or at the
            // final rect with alpha 0 (Fade), then animate in. Place the
            // window at the start geometry BEFORE making it visible so the
            // first frame is never at the final rect.
            //
            // Rapid-toggle case: if the HIDE animation is still in flight the
            // window is visible at an intermediate position — animate back in
            // from THERE instead of teleporting first (the jump made fast
            // toggles flicker).
            let from = if state.quake_anim.is_some() {
                let p = state.window.outer_position().unwrap_or_default();
                let s = state.window.inner_size();
                (p.x, p.y, s.width, s.height)
            } else {
                let off = anim_rest_rect(quake_cfg.animation, quake_cfg.edge, mon_rect, rect);
                apply_window_rect(&state.window, off, true);
                off
            };
            if is_fade {
                // Start fully transparent; pump ramps the alpha up.
                set_window_alpha(&state.window, 0);
            }
            state.window.set_visible(true);
            state.window.focus_window();
            state.quake_anim = Some(QuakeAnim {
                start: std::time::Instant::now(),
                duration: dur,
                showing: true,
                from,
                to: rect,
                anim_kind: quake_cfg.animation,
            });
            state.window.request_redraw();
            return;
        }
        // Instant: position exactly, then reveal.
        apply_window_rect(&state.window, rect, true);
    }
    state.quake_anim = None;
    state.window.set_visible(true);
    state.window.focus_window();
}

// ── compute_quake_target ──────────────────────────────────────────────────────

/// Resolve the target rect for a Quake show. For dock-mode (`edge != Off`)
/// the rect is computed from the chosen monitor + size/margin via
/// [`terminale_config::quake_dock_rect`]. For free-floating mode (`edge ==
/// Off`) it's the last saved exact geometry. Returns `None` only if there
/// is no monitor and no saved rect (extremely unusual).
/// Returns `(target_rect, monitor_rect)`. `monitor_rect` is `None` for
/// free-floating mode (`edge == Off`) or when a user-adjusted geometry is
/// being used (the monitor is not relevant in that case).
pub(crate) fn compute_quake_target(
    state: &RunningState,
    cfg: &terminale_config::QuakeConfig,
) -> Option<(
    terminale_config::WindowRect,
    Option<terminale_config::MonitorRect>,
)> {
    use terminale_config::QuakeDisplay;

    if cfg.edge == terminale_config::QuakeEdge::Off {
        return state.quake_saved_rect.map(|r| (r, None));
    }

    // Dock mode: a user-adjusted geometry wins so the window reappears
    // exactly as it disappeared (e.g. a manually resized height persists
    // across hide/show).
    if let Some(u) = state.quake_user_rect {
        return Some((u, None));
    }

    // Pick the target monitor following `cfg.display`.
    let monitors: Vec<_> = state.window.available_monitors().collect();
    let mon = match cfg.display {
        // `Current` means "the monitor containing the OS cursor at the moment
        // the hotkey fires".  We resolve this by polling the OS cursor
        // position synchronously via `os_cursor_position` + `monitor_at_point`
        // — this is a pull (not a push/cache), so it is always correct even
        // when the Quake window has been hidden and the user has moved to a
        // different monitor in another application.
        //
        // Fallback chain (in priority order):
        //  1. OS cursor → monitor at that point  (Windows; None on macOS/Linux)
        //  2. quake_last_monitor snapshot          (set at hide-time and on
        //                                           visible-window events;
        //                                           the only signal on macOS/Linux)
        //  3. Window::current_monitor()            (window's last-known rect)
        //  4. Window::primary_monitor()            (last resort)
        //  5. First available monitor              (degenerate: no other info)
        QuakeDisplay::Current => {
            let cursor_mon = crate::monitor_names::os_cursor_position()
                .and_then(|p| crate::monitor_names::monitor_at_point(&monitors, p));
            tracing::debug!(
                os_cursor = ?crate::monitor_names::os_cursor_position(),
                monitor_from_cursor = ?cursor_mon.as_ref().and_then(winit::monitor::MonitorHandle::name),
                snapshot = ?state.quake_last_monitor.as_ref().and_then(winit::monitor::MonitorHandle::name),
                "compute_quake_target: Current resolution chain"
            );
            cursor_mon
                .or_else(|| state.quake_last_monitor.clone())
                .or_else(|| state.window.current_monitor())
                .or_else(|| state.window.primary_monitor())
                .or_else(|| monitors.first().cloned())
        }
        // `Primary` uses the OS-authoritative primary on Windows (via
        // EnumDisplayMonitors + MONITORINFOF_PRIMARY) so that we pick the
        // correct display regardless of which monitor the application window
        // currently lives on. On macOS/Linux winit's `primary_monitor()` is
        // already authoritative, so `os_primary_monitor` returns None and
        // we fall through to the winit call.
        QuakeDisplay::Primary => {
            let winit_primary = state.window.primary_monitor();
            let os_primary = crate::monitor_names::os_primary_monitor(&monitors);
            // Log a warning when the two sources disagree so we can file a
            // winit issue with concrete data from user reports.
            if let (Some(ref wp), Some(ref op)) = (&winit_primary, &os_primary) {
                if wp.name() != op.name() {
                    tracing::warn!(
                        winit_primary = ?wp.name(),
                        os_primary = ?op.name(),
                        "QuakeDisplay::Primary: winit and OS disagree on primary monitor; \
                         using OS value"
                    );
                }
            }
            os_primary
                .or(winit_primary)
                .or_else(|| state.window.current_monitor())
                .or_else(|| monitors.first().cloned())
        }
        QuakeDisplay::Index(i) => {
            let handle = monitors.get(i as usize).cloned();
            if handle.is_none() {
                tracing::warn!(
                    "QuakeDisplay::Index({i}) is out of range \
                     ({} monitor(s) connected); falling back to current/primary",
                    monitors.len()
                );
            }
            handle
                .or_else(|| state.window.current_monitor())
                .or_else(|| state.window.primary_monitor())
        }
    }?;
    tracing::debug!(
        display = ?cfg.display,
        monitor = ?mon.name(),
        "compute_quake_target: resolved monitor"
    );
    let pos = mon.position();
    let size = mon.size();
    let mon_rect: terminale_config::MonitorRect = (pos.x, pos.y, size.width, size.height);
    terminale_config::quake_dock_rect(mon_rect, cfg.edge, cfg.size_percent, cfg.margin_px)
        .map(|r| (r, Some(mon_rect)))
}

// ── pump_quake_anim ───────────────────────────────────────────────────────────

/// Advance any in-flight Quake slide animation by one frame. Returns the
/// duration until the next frame is needed (`Some`) while animating, or
/// `None` when idle/finished. Called from `about_to_wait`.
pub(crate) fn pump_quake_anim(state: &mut RunningState) -> Option<std::time::Duration> {
    use terminale_config::QuakeAnimation;
    let anim = state.quake_anim.as_ref()?;
    let elapsed = anim.start.elapsed();
    let total = anim.duration;
    let is_fade = matches!(anim.anim_kind, QuakeAnimation::Fade);
    if elapsed >= total {
        // Finished.
        let showing = anim.showing;
        let to = anim.to;
        state.quake_anim = None;
        if showing {
            // Snap to exact final rect on the last frame. Every geometric
            // variant is now a reveal (size interpolates), so the final
            // frame must also resize.
            apply_window_rect(&state.window, to, true);
        } else {
            state.window.set_visible(false);
        }
        // Fade: always return to full opacity once the animation is over —
        // a hidden transparent window would otherwise reappear invisible on
        // a later instant/slide show.
        if is_fade {
            set_window_alpha(&state.window, 255);
        }
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let t = elapsed.as_secs_f32() / total.as_secs_f32();

    // Showing: t goes 0→1 (collapsed/transparent → resting rect).
    // Hiding:  t goes 0→1 but from resting → collapsed/transparent.
    // Either way the same easing applies.

    // Choose the easing curve per animation variant.
    let eased = match anim.anim_kind {
        QuakeAnimation::Bounce => {
            // Springy growth: cubic-out with a sin-damped dip mid-flight.
            // Clamped to 1.0 — the reveal interpolates SIZE, and overshooting
            // past the target rect could poke beyond the monitor edge.
            use std::f32::consts::PI;
            let base = 1.0 - (1.0 - t).powi(3);
            let wobble = (1.0 - (t * PI).sin().abs() * 0.18).clamp(0.9, 1.06);
            (base * wobble).clamp(0.0, 1.0)
        }
        _ => {
            // Ease-out cubic (Slide, Scale, Fade).
            1.0 - (1.0 - t).powi(3)
        }
    };

    if is_fade {
        // Fade: geometry is constant (from == to == resting rect); only the
        // whole-window opacity animates. No-op on non-Windows (degrades to
        // an instant show/hide at animation end).
        let a = if anim.showing { eased } else { 1.0 - eased };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        set_window_alpha(&state.window, (a * 255.0).round().clamp(0.0, 255.0) as u8);
    } else {
        // Slide/Bounce (axis reveal) and Scale (point zoom) interpolate both
        // position and size — the docked edge stays pinned and the window
        // never leaves the monitor. The PTY grid is NOT resized during the
        // animation (see the pending_resize guard in main.rs): the surface
        // clips the full-size frame, which is what makes it read as a reveal.
        let cur = lerp_rect_full(anim.from, anim.to, eased);
        apply_window_rect(&state.window, cur, true);
    }
    // A pure reposition/alpha change doesn't always generate a paint on
    // Windows, so the animation can look like it "jumps". Force a redraw
    // each frame so it actually plays.
    state.window.request_redraw();
    // ~60 Hz frame cadence.
    Some(std::time::Duration::from_millis(16))
}

// ── apply_theme / cursor_params_from_config / gpu_options_from_config ────────

/// Push the active theme down to every emulator + the renderer. The
/// emulator uses its palette to resolve ANSI named/indexed colours into
/// pixels; the renderer uses the theme's background for the window clear.
pub(crate) fn apply_theme(state: &mut RunningState, cfg: &terminale_config::Config) {
    let theme = cfg.appearance.resolved();
    tracing::debug!(name = %theme.name, "applying theme");
    let palette = terminale_term::AnsiPalette {
        foreground: theme.foreground,
        background: theme.background,
        normal: theme.normal,
        bright: theme.bright,
    };
    for tab in &state.tabs {
        tab.emulator.lock().set_palette(palette);
    }
    state.palette = palette;
    state.theme_name = cfg.appearance.theme.clone();
    state.theme_names = cfg
        .appearance
        .all_themes()
        .into_iter()
        .map(|t| t.name)
        .collect();
    state.renderer.set_background_color(theme.background);
    state.renderer.set_background_alpha(cfg.window.opacity);
    state.renderer.set_selection_color(theme.selection);
    // The theme's cursor colour is the fallback tint when the user hasn't
    // pinned an explicit `cursor.color`, so switching themes recolours the
    // cursor too (e.g. Matrix → green).
    state.renderer.set_cursor_theme_color(Some(theme.cursor));
    state.window.request_redraw();
}

/// Map the user-facing [`terminale_config::CursorConfig`] onto the
/// renderer's `CursorParams`. Kept in `main` so the renderer crate stays
/// free of config-layer types.
pub(crate) fn cursor_params_from_config(
    cfg: &terminale_config::Config,
) -> terminale_render::CursorParams {
    use terminale_config::CursorStyle as CfgCursorStyle;
    use terminale_render::CursorStyle as RenderCursorStyle;
    let style = match cfg.cursor.style {
        CfgCursorStyle::Block => RenderCursorStyle::Block,
        CfgCursorStyle::OutlineBlock => RenderCursorStyle::OutlineBlock,
        CfgCursorStyle::Underline => RenderCursorStyle::Underline,
        CfgCursorStyle::Beam => RenderCursorStyle::Beam,
    };
    terminale_render::CursorParams {
        style,
        blink: cfg.cursor.blink,
        blink_rate_ms: cfg.cursor.blink_rate_ms,
        color: cfg.cursor.color,
        thickness_px: cfg.cursor.thickness_px,
        opacity: cfg.cursor.opacity,
        cell_tint_opacity: cfg.cursor.cell_tint_opacity,
        blink_ease: cfg.cursor.blink_ease,
        animation_fps: cfg.cursor.animation_fps,
    }
}

/// Map the user-facing `[appearance] tab_bar_position` config value onto the
/// renderer's [`terminale_render::TabBarPlacement`]. Keeps the renderer crate
/// config-agnostic: this is the single translation point used by both the
/// initial setup path and the live-apply paths.
pub(crate) fn tab_bar_placement_from_config(
    cfg: &terminale_config::Config,
) -> terminale_render::TabBarPlacement {
    match cfg.appearance.tab_bar_position {
        terminale_config::TabBarPosition::Top => terminale_render::TabBarPlacement::Top,
        terminale_config::TabBarPosition::Bottom => terminale_render::TabBarPlacement::Bottom,
        terminale_config::TabBarPosition::Left => terminale_render::TabBarPlacement::Left,
        terminale_config::TabBarPosition::Right => terminale_render::TabBarPlacement::Right,
    }
}

/// Map the user-facing [`terminale_config::GpuConfig`] onto the renderer's
/// `GpuOptions`. Keeps the renderer crate config-agnostic: here is the one
/// place that translates `[gpu] backend`/`power_preference` enum strings into
/// raw wgpu bitflags and the software (CPU fallback) request.
pub(crate) fn gpu_options_from_config(
    cfg: &terminale_config::Config,
) -> terminale_render::GpuOptions {
    use terminale_config::{GpuBackend, GpuPowerPreference};
    // `Auto` lets wgpu pick the best API; an explicit variant restricts the
    // instance to that single backend; `Software` keeps the default backend
    // set but flips `force_fallback_adapter`, which selects a CPU adapter and
    // so disables hardware GPU acceleration.
    let backends = match cfg.gpu.backend {
        GpuBackend::Auto | GpuBackend::Software => wgpu::Backends::all(),
        GpuBackend::Vulkan => wgpu::Backends::VULKAN,
        GpuBackend::Dx12 => wgpu::Backends::DX12,
        GpuBackend::Metal => wgpu::Backends::METAL,
        GpuBackend::Gl => wgpu::Backends::GL,
    };
    let power_preference = match cfg.gpu.power_preference {
        GpuPowerPreference::Auto => wgpu::PowerPreference::None,
        GpuPowerPreference::Low => wgpu::PowerPreference::LowPower,
        GpuPowerPreference::High => wgpu::PowerPreference::HighPerformance,
    };
    terminale_render::GpuOptions {
        backends,
        power_preference,
        force_fallback_adapter: matches!(cfg.gpu.backend, GpuBackend::Software),
    }
}

// ── translate_bg_fx_params ────────────────────────────────────────────────────

/// Convert a user `BackgroundFxConfig` into the render-crate `BgFxParams`,
/// resolving `None` tints to per-style defaults and converting sRGB → linear
/// (matching the bg pipeline's `powf(2.2)` convention).
pub(crate) fn translate_bg_fx_params(
    cfg: &terminale_config::BackgroundFxConfig,
) -> terminale_render::BgFxParams {
    use terminale_config::BackgroundFxStyle;
    let to_linear = |c: [u8; 3]| {
        [
            (f32::from(c[0]) / 255.0).powf(2.2),
            (f32::from(c[1]) / 255.0).powf(2.2),
            (f32::from(c[2]) / 255.0).powf(2.2),
        ]
    };
    // Per-style default tints (already linear-ish, hand-picked).
    let (def1, def2) = match cfg.style {
        BackgroundFxStyle::None | BackgroundFxStyle::AuroraPlasma => {
            ([0.32, 0.08, 0.62], [0.04, 0.55, 0.62])
        }
        BackgroundFxStyle::Starfield => ([0.85, 0.88, 1.0], [0.15, 0.25, 0.7]),
        BackgroundFxStyle::Matrix => ([0.0, 0.45, 0.10], [0.45, 1.0, 0.55]),
        BackgroundFxStyle::PixelCrt => ([0.75, 0.10, 0.60], [0.10, 0.70, 0.90]),
    };
    terminale_render::BgFxParams {
        enabled: cfg.enabled,
        mode: cfg.style.shader_mode(),
        intensity: cfg.intensity.clamp(0.0, 1.0),
        speed: cfg.speed.clamp(0.1, 5.0),
        color1: cfg.color1.map_or(def1, to_linear),
        color2: cfg.color2.map_or(def2, to_linear),
        band_lifetime_secs: cfg.band_lifetime_secs.clamp(0.5, 8.0),
        matrix_band_width: cfg.matrix_band_width.clamp(1, 8),
        matrix_fall_speed: cfg.matrix_fall_speed.clamp(4.0, 60.0),
        max_emitters: cfg
            .max_emitters
            .clamp(1, terminale_render::MAX_EMITTERS as u32),
    }
}

/// Convert a user `BackgroundImageConfig` into the render-crate `BgImageParams`.
pub(crate) fn translate_bg_image_params(
    cfg: &terminale_config::BackgroundImageConfig,
) -> terminale_render::BgImageParams {
    use terminale_config::BgImageFit as CfgFit;
    use terminale_render::BgImageFit as RndFit;
    terminale_render::BgImageParams {
        path: cfg.path.clone(),
        opacity: cfg.opacity.clamp(0.0, 1.0),
        fit: match cfg.fit {
            CfgFit::Fill => RndFit::Fill,
            CfgFit::Fit => RndFit::Fit,
            CfgFit::Stretch => RndFit::Stretch,
            CfgFit::Center => RndFit::Center,
            CfgFit::Tile => RndFit::Tile,
        },
        brightness: cfg.brightness.clamp(0.0, 2.0),
        saturation: cfg.saturation.clamp(0.0, 2.0),
        hue: cfg.hue.clamp(0.0, 360.0),
    }
}
