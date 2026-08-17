//! Linux / BSD windowing-system integration.
//!
//! Two unrelated-looking things live here because they share one root cause:
//! **which windowing backend the process ended up on**.
//!
//! * Wayland gives clients no control over their own window position.
//!   `Window::set_outer_position` is a documented no-op and
//!   `Window::outer_position()` returns `Err`. Every terminale feature built on
//!   explicit geometry — Quake edge docking, the Snap actions,
//!   `window.startup_position`, cursor-anchored menus and dialogs, tab
//!   tear-out — therefore does nothing on a native Wayland surface. X11 (and
//!   XWayland, which every mainstream Wayland session runs) supports all of it,
//!   which is why [`crate::main`] prefers the X11 backend by default. See
//!   [`supports_positioning`] and [`warn_positioning_unsupported_once`].
//!
//! * Two window behaviours winit has no API for at all are plain EWMH property
//!   writes on X11: whole-window alpha (`_NET_WM_WINDOW_OPACITY`, which the
//!   Quake `Fade` animation needs) and "show on every workspace"
//!   (`_NET_WM_DESKTOP`). Both were previously no-ops on Linux; on an X11
//!   surface they now work. See [`set_window_opacity`] and
//!   [`set_on_all_desktops`].
//!
//! Everything here is best-effort: a failure to reach the X server, or a
//! Wayland surface where the operation is impossible, degrades to a no-op plus
//! a single log line rather than an error the caller must handle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use winit::window::Window;

/// The windowing backend a live window is actually running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    /// X11 — natively, or through XWayland under a Wayland session.
    X11,
    /// A native Wayland surface.
    Wayland,
    /// Something else entirely (should not happen on Linux/BSD).
    Other,
}

/// Which backend `window` is running on, read from its raw window handle —
/// the authoritative answer, unlike the `$XDG_SESSION_TYPE` guess, because it
/// reflects what winit actually built.
pub(crate) fn backend_of(window: &Window) -> Backend {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else {
        return Backend::Other;
    };
    match handle.as_raw() {
        RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_) => Backend::X11,
        RawWindowHandle::Wayland(_) => Backend::Wayland,
        _ => Backend::Other,
    }
}

/// Whether this window can be positioned by the application at all.
///
/// `true` on X11. `false` on Wayland, where the compositor owns placement and
/// every `set_outer_position` call is silently dropped.
pub(crate) fn supports_positioning(window: &Window) -> bool {
    backend_of(window) == Backend::X11
}

/// Whether an X server (real X11 or XWayland) is reachable — i.e. `$DISPLAY`
/// is set to something non-empty. Used before any window exists, to pick the
/// backend when `integration.linux_backend = "auto"`.
pub(crate) fn x_display_available() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
}

/// Whether the *session* is Wayland, per the environment. This is about the
/// compositor, not about which backend terminale's own window uses: under
/// XWayland both are true at once, and that combination is exactly why the
/// X11 global key grab cannot work (see [`crate::portal_shortcuts`]).
pub(crate) fn session_is_wayland() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty()) {
        return true;
    }
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

/// Log — once per process — that a geometry operation was asked for on a
/// surface that cannot honour it, and how to fix it. Called from the window
/// positioning paths so a Wayland user gets one actionable line instead of
/// either silence or a flood.
pub(crate) fn warn_positioning_unsupported_once() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    tracing::warn!(
        "this window is a native Wayland surface, and Wayland does not let an \
         application place its own windows — Quake edge docking, the Snap \
         actions and window.startup_position cannot take effect. Set \
         `integration.linux_backend = \"x11\"` (Settings › Desktop integration) \
         and restart to run through XWayland, where they all work."
    );
}

// ── X11 (EWMH) property writes ───────────────────────────────────────────────

/// A cached X11 connection plus the atoms we write. Opened lazily on the first
/// EWMH write and kept for the process lifetime.
///
/// This is deliberately a *separate* connection from the one winit owns:
/// borrowing winit's would mean reaching into its internals and sharing a
/// non-reentrant Xlib display across call sites. A second connection costs one
/// socket and is completely standard X11 practice — property writes are
/// server-side state, so it does not matter which client performs them.
struct X11Conn {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    /// `_NET_WM_WINDOW_OPACITY` — whole-window alpha honoured by compositing WMs.
    opacity: u32,
    /// `_NET_WM_DESKTOP` — which workspace a window lives on (`!0` = all).
    desktop: u32,
    /// `_NET_WORKAREA` — the screen area left over once panels/docks have
    /// reserved their strut.
    workarea: u32,
    /// `_NET_CURRENT_DESKTOP` — indexes into `_NET_WORKAREA`, which holds one
    /// rect per virtual desktop.
    current_desktop: u32,
}

/// Lazily-opened shared X11 connection. `None` once an attempt has failed, so
/// a headless / Wayland-only session doesn't retry on every animation frame.
fn x11() -> Option<&'static X11Conn> {
    static CONN: OnceLock<Option<X11Conn>> = OnceLock::new();
    CONN.get_or_init(|| match open_x11() {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!(?e, "no X11 connection for EWMH window properties");
            None
        }
    })
    .as_ref()
}

fn open_x11() -> Result<X11Conn, Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;

    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .ok_or("X11 screen index out of range")?
        .root;
    let opacity = conn
        .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")?
        .reply()?
        .atom;
    let desktop = conn.intern_atom(false, b"_NET_WM_DESKTOP")?.reply()?.atom;
    let workarea = conn.intern_atom(false, b"_NET_WORKAREA")?.reply()?.atom;
    let current_desktop = conn
        .intern_atom(false, b"_NET_CURRENT_DESKTOP")?
        .reply()?
        .atom;
    Ok(X11Conn {
        conn,
        root,
        opacity,
        desktop,
        workarea,
        current_desktop,
    })
}

/// Read a CARDINAL array property from the root window.
fn root_cardinals(x: &X11Conn, atom: u32, max_items: u32) -> Option<Vec<u32>> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let reply = x
        .conn
        .get_property(false, x.root, atom, AtomEnum::CARDINAL, 0, max_items)
        .ok()?
        .reply()
        .ok()?;
    reply.value32().map(Iterator::collect)
}

/// The desktop's usable area in physical pixels — the whole virtual screen
/// minus whatever panels, docks and bars have reserved via their strut
/// (`_NET_WORKAREA`). `None` when there is no X connection or the window
/// manager doesn't publish the property.
///
/// EWMH stores one rect per virtual desktop, so the current desktop's entry is
/// the one that matters.
///
/// Note the shape of the data: `_NET_WORKAREA` is a single rect spanning **all**
/// monitors, not one per monitor, so callers intersect it with the monitor they
/// care about (see `dock_work_area`). That is as precise as EWMH gets.
pub(crate) fn work_area() -> Option<(i32, i32, u32, u32)> {
    let x = x11()?;
    let rects = root_cardinals(x, x.workarea, 4 * 64)?;
    let desktop = root_cardinals(x, x.current_desktop, 1)
        .and_then(|v| v.first().copied())
        .unwrap_or(0) as usize;
    // Fall back to the first rect when the current-desktop index is out of
    // range (some WMs publish a single rect regardless of desktop count).
    let base = if rects.len() >= (desktop + 1) * 4 {
        desktop * 4
    } else {
        0
    };
    let slice = rects.get(base..base + 4)?;
    let (w, h) = (slice[2], slice[3]);
    if w == 0 || h == 0 {
        return None;
    }
    Some((slice[0] as i32, slice[1] as i32, w, h))
}

/// Intersect a monitor rect with `other`, returning the overlap. `None` when
/// they don't overlap at all (a stale work area, a monitor hot-unplug), so the
/// caller can fall back to the untrimmed monitor rect rather than docking a
/// window into an empty rectangle.
///
/// Pure geometry — unit-tested without an X server.
#[must_use]
pub(crate) fn intersect_rect(
    a: (i32, i32, u32, u32),
    b: (i32, i32, u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    let left = ax.max(bx);
    let top = ay.max(by);
    let right = (ax.saturating_add_unsigned(aw)).min(bx.saturating_add_unsigned(bw));
    let bottom = (ay.saturating_add_unsigned(ah)).min(by.saturating_add_unsigned(bh));
    if right <= left || bottom <= top {
        return None;
    }
    #[allow(clippy::cast_sign_loss)]
    Some((left, top, (right - left) as u32, (bottom - top) as u32))
}

/// The X11 window id behind `window`, or `None` when it isn't an X11 surface.
fn x11_window_id(window: &Window) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        // `Xlib::window` is a `c_ulong` but X11 resource ids are 32-bit.
        #[allow(clippy::cast_possible_truncation)]
        RawWindowHandle::Xlib(h) => Some(h.window as u32),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// Set whole-window alpha via `_NET_WM_WINDOW_OPACITY` (0 = invisible,
/// 255 = fully opaque). This is what makes the Quake `Fade` animation visible
/// on Linux; it needs a compositing window manager (mutter, kwin, picom — i.e.
/// every modern desktop). No-op on Wayland or without an X connection.
///
/// At full opacity the property is *deleted* rather than set to the maximum, so
/// the compositor drops the window out of its translucency path entirely.
pub(crate) fn set_window_opacity(window: &Window, alpha: u8) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    let (Some(x), Some(win)) = (x11(), x11_window_id(window)) else {
        return;
    };
    let result = if alpha == u8::MAX {
        x.conn
            .delete_property(win, x.opacity)
            .map(x11rb::cookie::VoidCookie::ignore_error)
    } else {
        // EWMH encodes opacity as a CARDINAL spanning the full u32 range.
        let value = u32::try_from(u64::from(alpha) * u64::from(u32::MAX) / 255).unwrap_or(u32::MAX);
        x.conn
            .change_property32(
                PropMode::REPLACE,
                win,
                x.opacity,
                AtomEnum::CARDINAL,
                &[value],
            )
            .map(x11rb::cookie::VoidCookie::ignore_error)
    };
    if let Err(e) = result {
        tracing::debug!(?e, "could not set _NET_WM_WINDOW_OPACITY");
        return;
    }
    let _ = x.conn.flush();
}

/// Show `window` on every virtual desktop / workspace (or undo it) via
/// `_NET_WM_DESKTOP`. This is the Linux half of `quake.show_on_all_desktops`,
/// which used to be a documented no-op here.
///
/// A mapped window must be moved between desktops with a ClientMessage to the
/// root window (EWMH §_NET_WM_DESKTOP) rather than a direct property write, so
/// that is what we send; the property is also written directly so the value
/// survives on window managers that only read it at map time. Disabling moves
/// the window to the *current* desktop, which is what the user sees anyway.
pub(crate) fn set_on_all_desktops(window: &Window, enable: bool) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode,
    };
    use x11rb::wrapper::ConnectionExt as _;

    let (Some(x), Some(win)) = (x11(), x11_window_id(window)) else {
        return;
    };
    // 0xFFFF_FFFF is EWMH's "all desktops"; otherwise fall back to desktop 0
    // (the only value we can name without querying the current desktop, and the
    // one an un-pinned window sensibly returns to).
    let target: u32 = if enable { u32::MAX } else { 0 };

    if let Err(e) = x.conn.change_property32(
        PropMode::REPLACE,
        win,
        x.desktop,
        AtomEnum::CARDINAL,
        &[target],
    ) {
        tracing::debug!(?e, "could not write _NET_WM_DESKTOP");
        return;
    }
    // Source indication 1 = "normal application" per EWMH.
    let event = ClientMessageEvent::new(32, win, x.desktop, [target, 1, 0, 0, 0]);
    let send = x.conn.send_event(
        false,
        x.root,
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
        event,
    );
    match send {
        Ok(cookie) => cookie.ignore_error(),
        Err(e) => {
            tracing::debug!(?e, "could not send _NET_WM_DESKTOP client message");
            return;
        }
    }
    let _ = x.conn.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `x_display_available` must treat an empty `$DISPLAY` as absent — an
    /// empty value is what a stripped environment leaves behind, and asking
    /// winit for X11 there fails at event-loop build time.
    #[test]
    fn empty_display_is_not_available() {
        // The test process may legitimately have DISPLAY set; assert on the
        // documented rule rather than the ambient value.
        let raw = std::env::var_os("DISPLAY");
        let expected = raw.is_some_and(|d| !d.is_empty());
        assert_eq!(x_display_available(), expected);
    }

    /// The warning must be emitted at most once, so a Wayland user gets one
    /// actionable line instead of one per animation frame.
    #[test]
    fn positioning_warning_is_idempotent() {
        warn_positioning_unsupported_once();
        warn_positioning_unsupported_once();
    }

    /// The GNOME case this was written for: a 3-monitor layout whose union
    /// work area is trimmed by the 29 px top bar. The monitor under the bar
    /// must lose exactly that band, and nothing else.
    #[test]
    fn intersect_trims_the_panel_band() {
        let workarea = (0, 29, 6400, 1159);
        let primary = (1920, 0, 1920, 1080);
        assert_eq!(
            intersect_rect(primary, workarea),
            Some((1920, 29, 1920, 1051))
        );
    }

    /// A monitor that sits entirely below the panel band keeps its full rect.
    #[test]
    fn intersect_leaves_untouched_monitors_alone() {
        let workarea = (0, 29, 6400, 1159);
        let lower = (0, 108, 1920, 1080);
        assert_eq!(intersect_rect(lower, workarea), Some(lower));
    }

    /// A stale work area that no longer overlaps the monitor must report `None`
    /// so callers keep the untrimmed rect instead of docking into nothing.
    #[test]
    fn intersect_reports_no_overlap() {
        assert_eq!(intersect_rect((0, 0, 100, 100), (500, 500, 100, 100)), None);
        // Edge-touching rects share no area either.
        assert_eq!(intersect_rect((0, 0, 100, 100), (100, 0, 100, 100)), None);
    }
}
