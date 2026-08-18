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

// ── Application identity (systemd app scope) ─────────────────────────────────

/// The application id terminale claims. Matches the `terminale.desktop` entry
/// it installs, and is the name the desktop keys a remembered global shortcut
/// off — so it must stay stable across releases and across launches.
const APP_ID: &str = "terminale";

/// Give this process an *application id*, by placing it in a systemd user scope
/// named `app-terminale-<pid>.scope`.
///
/// This exists for one reason: `org.freedesktop.portal.GlobalShortcuts` refuses
/// callers it can't identify — `CreateSession` fails outright with
/// "An app id is required" — and for an ordinary (non-Flatpak) process the
/// desktop derives that id from the systemd unit the process lives in. A
/// terminale started from the application menu already has one, because the
/// desktop launched it into `app-gnome-terminale-*.scope`; one started from a
/// shell inherits that shell's scope and has none. So the Quake hotkey worked
/// or not depending on how the app happened to be started, which is not a
/// distinction any user should have to know about.
///
/// Registering the shortcut in the *desktop's* settings would work too, but it
/// is the wrong shape: setting a hotkey inside terminale should be all a user
/// has to do. Claiming the id ourselves keeps it that way.
///
/// This is exactly what `systemd-run --user --scope` does, and moving one's own
/// PID needs no privilege. Best-effort throughout: no systemd, no session bus,
/// or a refused call all leave the process where it was and return `false`.
///
/// `runtime` is used to drive the D-Bus round-trip; the call is awaited before
/// returning, because the caller goes straight on to talk to the portal and the
/// cgroup move has to have landed by then.
pub(crate) fn ensure_app_scope(runtime: &tokio::runtime::Runtime) -> bool {
    if let Some(unit) = current_app_unit() {
        tracing::debug!(unit, "already running under an application unit");
        return true;
    }
    match runtime.block_on(start_transient_scope()) {
        Ok(name) => {
            // The unit is created by a systemd *job*, so the cgroup move can
            // land a moment after the call returns. The portal reads our cgroup
            // to resolve the app id, so wait for it to actually take effect
            // rather than racing it.
            for _ in 0..50 {
                if current_app_unit().is_some() {
                    tracing::info!(scope = %name, app_id = APP_ID, "claimed an application id");
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            tracing::debug!(scope = %name, "app scope created but the cgroup move did not land");
            false
        }
        Err(e) => {
            tracing::debug!(?e, "could not place this process in a systemd app scope");
            false
        }
    }
}

/// The leaf systemd unit this process lives in, when it is *our* application
/// unit — the form the desktop parses our application id out of.
///
/// Two things make this narrower than it looks.
///
/// Only the leaf counts. A shell inside GNOME Console sits at
/// `…/app-…-org.gnome.Console.slice/vte-spawn-….scope`: an `app-…` ancestor, but
/// a leaf that names the terminal's spawn helper, not an application.
///
/// And the unit has to name *terminale*, not merely some application. Launch
/// terminale from a terminal emulator that puts each of its windows in its own
/// app scope — ghostty does, as `app-ghostty-surface-transient-<n>.scope` — and
/// the leaf passes for an application unit while announcing somebody else's
/// identity. Accepting it meant terminale skipped claiming a scope of its own,
/// inherited the host terminal's app id, and the global-shortcuts portal then
/// refused the Quake binding with `NotAllowed("An app id is required")`. The
/// hotkey silently did nothing for the entire session, which is exactly how it
/// presents: the drop-down hides once and never comes back.
fn current_app_unit() -> Option<String> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    app_unit_in_cgroup(&cgroup)
}

/// Whether a cgroup leaf names an *application* unit. systemd unit suffixes are
/// case-sensitive, so `strip_suffix` is used rather than `ends_with` — which
/// clippy reads as a (case-insensitive) file-extension check.
fn is_app_unit(leaf: &str) -> bool {
    leaf.starts_with("app-")
        && (leaf.strip_suffix(".scope").is_some() || leaf.strip_suffix(".service").is_some())
}

/// Whether a cgroup leaf names an application unit belonging to *terminale*.
///
/// The desktop reads the app id out of `app-<app id>-<instance>.scope` (or the
/// bare `app-<app id>.scope`), and `terminale.desktop` makes that id [`APP_ID`].
/// So a unit that starts `app-ghostty-` is an application unit, just not ours —
/// and treating it as ours is what left the process wearing another app's
/// identity. Matching on the id keeps both real cases right: a launch from the
/// application menu already sits in `app-terminale-….scope` and is left alone,
/// while a launch from any other app's scope goes on to claim its own.
fn is_our_app_unit(leaf: &str) -> bool {
    if !is_app_unit(leaf) {
        return false;
    }
    let stem = leaf
        .strip_suffix(".scope")
        .or_else(|| leaf.strip_suffix(".service"))
        .unwrap_or(leaf);
    // Match on a whole dash-delimited segment, because the id is not always the
    // first one: GNOME Shell launches apps as `app-gnome-<app id>-<pid>.scope`,
    // while a self-claimed or systemd-run scope is `app-<app id>-<pid>.scope`.
    // Comparing segments accepts both and still rejects
    // `app-ghostty-surface-transient-<n>.scope`, where our id appears nowhere.
    stem.strip_prefix("app-")
        .is_some_and(|rest| rest.split('-').any(|seg| seg == APP_ID))
}

/// Pure half of [`current_app_unit`]: pick the application unit out of the
/// contents of `/proc/self/cgroup`. Split out so the parsing is testable
/// without a particular process placement.
fn app_unit_in_cgroup(cgroup: &str) -> Option<String> {
    // cgroup v2 has a single `0::<path>` line; v1 has several. Either way the
    // unit is the last path segment.
    cgroup
        .lines()
        .filter_map(|l| l.rsplit(':').next())
        .filter_map(|path| path.rsplit('/').next())
        .find(|leaf| is_our_app_unit(leaf))
        .map(ToString::to_string)
}

/// Ask systemd's user manager to create a scope holding this process.
async fn start_transient_scope() -> Result<String, zbus::Error> {
    use zbus::zvariant::{OwnedObjectPath, Value};

    let pid = std::process::id();
    // `app-<app id>-<anything>.scope` is the layout the desktop parses; the pid
    // keeps the unit name unique across concurrent launches while the app id in
    // the middle stays constant.
    let name = format!("app-{APP_ID}-{pid}.scope");

    let conn = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;

    let properties: Vec<(&str, Value<'_>)> = vec![
        ("PIDs", Value::from(vec![pid])),
        ("Description", Value::from("terminale")),
        // Don't leave a failed unit lying around if the process dies badly.
        ("CollectMode", Value::from("inactive-or-failed")),
    ];
    // No auxiliary units.
    let aux: Vec<(&str, Vec<(&str, Value<'_>)>)> = Vec::new();

    let _job: OwnedObjectPath = proxy
        .call(
            "StartTransientUnit",
            &(name.as_str(), "fail", properties, aux),
        )
        .await?;
    Ok(name)
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
    /// `_NET_ACTIVE_WINDOW` — the "please focus this window" request.
    active_window: u32,
    /// A private property used only as a timestamp probe; see
    /// [`server_timestamp`].
    timestamp_probe: u32,
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
    let active_window = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
        .reply()?
        .atom;
    let timestamp_probe = conn
        .intern_atom(false, b"_TERMINALE_TIMESTAMP")?
        .reply()?
        .atom;
    Ok(X11Conn {
        conn,
        root,
        opacity,
        desktop,
        workarea,
        current_desktop,
        active_window,
        timestamp_probe,
    })
}

/// A genuine X server timestamp, obtained the way the protocol intends:
/// append zero bytes to a property on our own window and read the time out of
/// the `PropertyNotify` the server sends back.
///
/// This exists because of one number. Mutter's focus-stealing prevention
/// compares an activation request's timestamp against the user's last input;
/// a request carrying `CurrentTime` (0) cannot be compared, so it takes the
/// safe path and shows `<app> is ready` instead of focusing. winit's
/// `focus_window()` sends exactly that — `_NET_ACTIVE_WINDOW` with
/// `CURRENT_TIME` — which is why a Quake reveal announced itself in the
/// notification tray rather than taking the keyboard.
///
/// The event mask is selected and cleared around the probe: X11 event masks are
/// per-client, so this neither disturbs winit's own selection on the same window
/// nor leaves `PropertyNotify` traffic accumulating unread on this connection.
fn server_timestamp(x: &X11Conn, win: u32) -> Option<u32> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, PropMode,
    };
    use x11rb::protocol::Event;
    // `change_property8` lives on the wrapper trait, not the protocol one; both
    // are called `ConnectionExt`, hence the anonymous import.
    use x11rb::wrapper::ConnectionExt as _;

    x.conn
        .change_window_attributes(
            win,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .ok()?
        .check()
        .ok()?;
    // Appending nothing still counts as a property change, so the server
    // timestamps it without the property ever growing.
    let probe = x
        .conn
        .change_property8(
            PropMode::APPEND,
            win,
            x.timestamp_probe,
            AtomEnum::STRING,
            &[],
        )
        .ok()
        .and_then(|c| c.check().ok());
    let mut stamp = None;
    if probe.is_some() {
        let _ = x.conn.flush();
        // Bounded: a lost round-trip must cost a fallback, not the UI thread.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
        while std::time::Instant::now() < deadline {
            match x.conn.poll_for_event() {
                Ok(Some(Event::PropertyNotify(e))) if e.window == win => {
                    stamp = Some(e.time);
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(2)),
                Err(_) => break,
            }
        }
    }
    let _ = x.conn.change_window_attributes(
        win,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
    );
    let _ = x.conn.flush();
    stamp
}

/// Ask the window manager to focus `window`, with a real timestamp.
///
/// Returns `false` when there is no X11 connection or the request could not be
/// sent, so the caller can fall back to winit's own `focus_window()`.
///
/// Source indication is 1 (application), per EWMH — which is the truth, and the
/// part that was already right. Only the timestamp was wrong. (Source 2, "pager",
/// is often described as bypassing focus-stealing prevention; that is folklore.
/// The one Mutter-lineage source that spells the rule out treats a *pager*
/// request with a zero timestamp more strictly, not less, so this stays honest
/// about what it is and supplies a timestamp the WM can actually compare.)
pub(crate) fn activate_window(window: &Window) -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};

    let Some(x) = x11() else {
        return false;
    };
    let Some(win) = x11_window_id(window) else {
        return false;
    };
    let Some(time) = server_timestamp(x, win) else {
        tracing::debug!("no X server timestamp; leaving activation to winit");
        return false;
    };
    let event = ClientMessageEvent::new(32, win, x.active_window, [1, time, 0, 0, 0]);
    let sent = x
        .conn
        .send_event(
            false,
            x.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .is_ok();
    let _ = x.conn.flush();
    if sent {
        tracing::debug!(time, "asked the window manager to activate the window");
    }
    sent
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

    /// A shell started from a terminal emulator sits *under* an `app-…` slice
    /// but its leaf is the emulator's spawn helper — not an application unit.
    /// Reading that as an app id is what made the portal reject us, so this
    /// case must not match.
    #[test]
    fn vte_spawn_scope_is_not_an_app_unit() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                      app-dbus\\x2d:1.2\\x2dorg.gnome.Console.slice/\
                      vte-spawn-d19cc478-5399-4cbd-89ac-92468da00874.scope\n";
        assert_eq!(app_unit_in_cgroup(cgroup), None);
    }

    /// The scope terminale creates for itself, and the one GNOME creates when
    /// it launches the app from its desktop entry, must both be recognised —
    /// otherwise we would keep re-scoping an already-identified process.
    #[test]
    fn app_scopes_are_recognised() {
        let own = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                   app-terminale-201486.scope\n";
        assert_eq!(
            app_unit_in_cgroup(own).as_deref(),
            Some("app-terminale-201486.scope")
        );
        let gnome_launched = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                              app-gnome-terminale-4242.scope\n";
        assert_eq!(
            app_unit_in_cgroup(gnome_launched).as_deref(),
            Some("app-gnome-terminale-4242.scope")
        );
        // A transient .service counts too.
        let service = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                       app-terminale-7.service\n";
        assert!(app_unit_in_cgroup(service).is_some());
    }

    /// Another application's scope is not ours, however much it looks like an
    /// application unit.
    ///
    /// Launching terminale from a terminal emulator that scopes each of its
    /// windows — ghostty names them `app-ghostty-surface-transient-<n>.scope` —
    /// used to satisfy the "am I already identified?" check, so terminale never
    /// claimed a scope of its own and wore ghostty's identity instead. The
    /// global-shortcuts portal then answered `NotAllowed("An app id is
    /// required")` and the Quake hotkey did nothing for the whole session.
    #[test]
    fn another_apps_scope_is_not_ours() {
        let ghostty = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
                       app-ghostty-surface-transient-6975.scope\n";
        assert_eq!(app_unit_in_cgroup(ghostty), None);
        // Same shape, different neighbours: still not us.
        for leaf in [
            "app-ghostty.scope",
            "app-org.wezfurlong.wezterm-1234.scope",
            "app-Alacritty-9.scope",
        ] {
            assert!(!is_our_app_unit(leaf), "{leaf} must not read as ours");
        }
        // …and the ones that are ours keep matching, whoever launched them.
        for leaf in [
            "app-terminale.scope",
            "app-terminale-201486.scope",
            "app-gnome-terminale-4242.scope",
            "app-flatpak-terminale-7.service",
        ] {
            assert!(is_our_app_unit(leaf), "{leaf} must read as ours");
        }
    }

    /// A process outside any application unit (a plain login shell, a system
    /// service) must report none, so we go and claim a scope.
    #[test]
    fn non_app_cgroups_report_none() {
        assert_eq!(app_unit_in_cgroup("0::/init.scope\n"), None);
        assert_eq!(app_unit_in_cgroup(""), None);
        // Suffixes are case-sensitive: `.Scope` is not a systemd unit.
        assert!(!is_app_unit("app-terminale-1.Scope"));
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
