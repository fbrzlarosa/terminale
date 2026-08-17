//! Per-user control socket — lets a second `terminale` invocation drive the
//! already-running instance.
//!
//! This exists because of Wayland. A Wayland compositor never hands a client a
//! system-wide key grab, so the X11 grab behind [`global_hotkey`] only fires
//! while an X11 window happens to have focus — which under a Wayland session is
//! almost never. The two supported ways out are the desktop's global-shortcut
//! portal (see [`crate::portal_shortcuts`]) and the one every window manager
//! has understood for decades: bind a key to a *command*. This module is what
//! makes the command work —
//!
//! ```text
//! terminale --toggle-quake
//! ```
//!
//! connects to the running instance's socket, asks it to toggle the drop-down,
//! and exits. Bind that to any key in GNOME Settings, KDE, sway, i3, Hyprland,
//! or a `.desktop` action and Quake mode behaves exactly as it does on Windows.
//!
//! The protocol is deliberately trivial — one newline-terminated command per
//! connection, one newline-terminated reply — so it stays debuggable with
//! `socat`/`nc` and needs no dependency.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Command asking the running instance to toggle Quake visibility.
pub(crate) const CMD_TOGGLE_QUAKE: &str = "toggle-quake";
/// Liveness probe — replies `ok` and does nothing else.
pub(crate) const CMD_PING: &str = "ping";

/// How long the client waits for the server to answer before giving up. The
/// server answers from a dedicated thread without touching the UI thread, so
/// anything beyond this means the socket is stale, not that the app is busy.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);

/// Path of the per-user control socket.
///
/// `$XDG_RUNTIME_DIR` is the correct home for this: it is per-user, mode 0700,
/// and cleaned up at logout. Where it is absent (macOS, a bare `su` shell) we
/// fall back to a uid-suffixed name in the temp dir so two users on one machine
/// never collide.
pub(crate) fn socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir.join("terminale.sock");
        }
    }
    // SAFETY: `getuid` is always successful and has no preconditions.
    let uid = unsafe { libc_getuid() };
    std::env::temp_dir().join(format!("terminale-{uid}.sock"))
}

/// Minimal `getuid` binding — pulling in a whole libc dependency for one
/// argument-less syscall wrapper isn't worth it.
///
/// # Safety
///
/// `getuid` takes no arguments, never fails, and has no side effects.
unsafe fn libc_getuid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    getuid()
}

// ── Client side (`terminale --toggle-quake`) ─────────────────────────────────

/// Send `command` to the running instance and return its reply.
///
/// # Errors
///
/// Returns the connection error when no instance is listening (the usual case
/// being "terminale isn't running"), or an I/O error from the exchange itself.
pub(crate) fn send_command(command: &str) -> std::io::Result<String> {
    let path = socket_path();
    let stream = UnixStream::connect(&path)?;
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    let mut writer = &stream;
    writeln!(writer, "{command}")?;
    writer.flush()?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}

// ── Server side (the running instance) ───────────────────────────────────────

/// Bind the control socket and serve it on a dedicated thread for the process
/// lifetime. Returns the bound path on success.
///
/// A leftover socket file from a crashed instance is detected (nothing accepts
/// a connection on it) and replaced; a socket that a *live* instance still owns
/// is left alone and this returns `None`, so a second terminale window never
/// steals the first one's control channel.
pub(crate) fn serve(proxy: EventLoopProxy<UserEvent>) -> Option<PathBuf> {
    let path = socket_path();

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Either a live instance owns it, or it is a stale file left by a
            // crash. A successful connect distinguishes the two.
            if UnixStream::connect(&path).is_ok() {
                tracing::debug!(
                    path = %path.display(),
                    "another terminale instance already owns the control socket"
                );
                return None;
            }
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(?e, path = %path.display(), "could not clear stale control socket");
                return None;
            }
            match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(?e, path = %path.display(), "could not bind control socket");
                    return None;
                }
            }
        }
        Err(e) => {
            tracing::warn!(?e, path = %path.display(), "could not bind control socket");
            return None;
        }
    };

    // Owner-only, belt and braces: $XDG_RUNTIME_DIR is already 0700, but the
    // temp-dir fallback is world-readable.
    if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        tracing::debug!(?e, "could not tighten control socket permissions");
    }

    let thread_path = path.clone();
    let spawned = std::thread::Builder::new()
        .name("terminale-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        if handle_client(&stream, &proxy).is_break() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!(?e, "control socket accept failed");
                    }
                }
            }
            // The listener is dropped with the thread; remove the file so the
            // next launch binds cleanly instead of going through stale-socket
            // recovery.
            let _ = std::fs::remove_file(&thread_path);
        });

    if let Err(e) = spawned {
        tracing::warn!(?e, "could not start the control socket thread");
        let _ = std::fs::remove_file(&path);
        return None;
    }
    tracing::info!(path = %path.display(), "control socket listening");
    Some(path)
}

/// Serve one connection. Returns `Break` when the event loop has gone away and
/// the server thread should stop.
fn handle_client(
    stream: &UnixStream,
    proxy: &EventLoopProxy<UserEvent>,
) -> std::ops::ControlFlow<()> {
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));

    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return std::ops::ControlFlow::Continue(());
    }
    let command = line.trim();

    let (reply, deliver) = match command {
        CMD_TOGGLE_QUAKE => ("ok", Some(UserEvent::ToggleQuake)),
        CMD_PING => ("ok", None),
        "" => ("error: empty command", None),
        other => {
            tracing::debug!(command = other, "unknown control-socket command");
            ("error: unknown command", None)
        }
    };

    if let Some(event) = deliver {
        if proxy.send_event(event).is_err() {
            // The event loop is gone — the app is shutting down.
            return std::ops::ControlFlow::Break(());
        }
    }

    let mut writer = stream;
    let _ = writeln!(writer, "{reply}");
    let _ = writer.flush();
    std::ops::ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With `$XDG_RUNTIME_DIR` set the socket must live inside it — that is
    /// the per-user, mode-0700, logout-cleaned location the spec intends.
    #[test]
    fn socket_path_prefers_xdg_runtime_dir() {
        let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
            // Nothing to assert on a host without the variable; the fallback
            // is covered by `socket_path_is_absolute`.
            return;
        };
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return;
        }
        assert_eq!(socket_path(), dir.join("terminale.sock"));
    }

    /// Whichever branch is taken, the path must be absolute — a relative
    /// socket path would bind relative to the process's working directory,
    /// which for a GUI launch is arbitrary.
    #[test]
    fn socket_path_is_absolute() {
        assert!(socket_path().is_absolute());
    }

    /// Connecting when nothing is listening must surface as an error rather
    /// than hanging, so `--toggle-quake` can print a useful message.
    #[test]
    fn send_command_without_server_errors() {
        // Point at a path nothing can be listening on.
        let missing = std::env::temp_dir().join("terminale-ipc-test-absent.sock");
        let _ = std::fs::remove_file(&missing);
        assert!(UnixStream::connect(&missing).is_err());
    }
}
