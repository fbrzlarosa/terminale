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
//! The wire stays deliberately simple — one newline-terminated request per
//! connection, one newline-terminated reply — so it is still debuggable with
//! `socat`/`nc`. Requests are JSON objects (see [`crate::control`]) except for
//! the two original bare words, `ping` and `toggle-quake`, which keep working
//! verbatim so nobody's keybinding breaks on upgrade.
//!
//! This module is only the transport and the `terminale ctl` client. What the
//! commands *mean*, and what they are allowed to do, lives in
//! [`crate::control`] — so a Windows named-pipe transport can be added here
//! later without touching the vocabulary.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

use crate::control::{ControlReply, ControlRequest};
use crate::UserEvent;

/// How long the client waits for the server to answer before giving up.
///
/// The Quake toggle is answered from a dedicated thread that never touches the
/// UI, so for that command anything beyond this means the socket is stale, not
/// that the app is busy. Query commands *do* hop to the UI thread, so they get
/// [`QUERY_TIMEOUT`] instead.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a client waits for a command that has to be served by the UI
/// thread. Longer than [`CLIENT_TIMEOUT`] because the answer queues behind
/// whatever the event loop is doing — a resize, a big paste, a slow frame.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the socket thread waits for the UI thread to hand back a reply
/// before telling the client the app is busy. Deliberately shorter than
/// [`QUERY_TIMEOUT`] so the client hears a real answer ("busy") rather than
/// timing out on silence.
const UI_TIMEOUT: Duration = Duration::from_secs(8);

/// A request handed to the UI thread, with the channel its reply goes back on.
///
/// Boxed inside [`UserEvent::Control`] to keep the event enum small: winit
/// clones/moves user events around, and every other variant is a handful of
/// bytes.
#[derive(Debug)]
pub(crate) struct ControlCall {
    /// What was asked.
    pub(crate) request: ControlRequest,
    /// Where the answer goes. The socket thread blocks on the matching
    /// receiver; a dropped sender surfaces as "the app is shutting down".
    pub(crate) reply: std::sync::mpsc::Sender<ControlReply>,
}

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
///
/// Everything except `ping` is answered by the UI thread: tabs, panes,
/// emulators and the renderer all belong to it, and reaching into them from here
/// would race the render loop. So this thread parses, forwards, and waits.
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
    // A client that spoke the old bare-word protocol gets the old bare-word
    // answer; anything else gets JSON.
    let legacy = crate::control::wants_legacy_reply(&line);

    match crate::control::parse_line(&line) {
        Err(e) => {
            tracing::debug!(error = %e, "rejected control request");
            answer(stream, legacy, &ControlReply::err(e));
            std::ops::ControlFlow::Continue(())
        }
        Ok(request) if !request.needs_ui_thread() => {
            answer(stream, legacy, &ControlReply::ok());
            std::ops::ControlFlow::Continue(())
        }
        Ok(request) => {
            let (tx, rx) = std::sync::mpsc::channel();
            let call = Box::new(ControlCall { request, reply: tx });
            if proxy.send_event(UserEvent::Control(call)).is_err() {
                // The event loop is gone — the app is shutting down.
                return std::ops::ControlFlow::Break(());
            }
            match rx.recv_timeout(UI_TIMEOUT) {
                Ok(reply) => {
                    answer(stream, legacy, &reply);
                    std::ops::ControlFlow::Continue(())
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    answer(
                        stream,
                        legacy,
                        &ControlReply::err("terminale did not answer in time (busy?)"),
                    );
                    std::ops::ControlFlow::Continue(())
                }
                // The sender was dropped without a reply: the event loop took
                // the event and then went away.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    std::ops::ControlFlow::Break(())
                }
            }
        }
    }
}

/// Write one reply line back to a client.
fn answer(stream: &UnixStream, legacy: bool, reply: &ControlReply) {
    let line = if legacy && reply.ok {
        // Exactly what the pre-JSON server sent, byte for byte — the
        // `--toggle-quake` client string-compares against it.
        "ok".to_string()
    } else if legacy {
        format!(
            "error: {}",
            reply.error.as_deref().unwrap_or("request refused")
        )
    } else {
        reply.to_line()
    };
    let mut writer = stream;
    let _ = writeln!(writer, "{line}");
    let _ = writer.flush();
}

// ── `terminale ctl …` ─────────────────────────────────────────────────────────

/// Send `request` to the running instance and return its reply.
///
/// # Errors
///
/// Returns a connection error when no instance is listening (usually
/// "terminale isn't running"), or an I/O error from the exchange.
pub(crate) fn send_request(request: &ControlRequest) -> std::io::Result<ControlReply> {
    let path = socket_path();
    let stream = UnixStream::connect(&path)?;
    stream.set_read_timeout(Some(QUERY_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    let payload = serde_json::to_string(request).map_err(std::io::Error::other)?;
    let mut writer = &stream;
    writeln!(writer, "{payload}")?;
    writer.flush()?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    serde_json::from_str(reply.trim_end()).map_err(std::io::Error::other)
}

/// The `terminale ctl` subcommands.
///
/// A thin, discoverable spelling of [`ControlRequest`] — `clap` gives each one
/// `--help`, so the API documents itself from the shell. Kept separate from the
/// wire enum so the two can diverge in ergonomics (short flags, positional
/// arguments) without changing the protocol.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum CtlCommand {
    /// Check that a terminale instance is listening.
    Ping,
    /// Print the running version and which control permissions are in effect.
    Version,
    /// Show or hide the Quake drop-down.
    ToggleQuake,
    /// List every dispatchable action with its label and key binding.
    ListActions,
    /// Run an action by name (see `list-actions`).
    Action {
        /// Action name, e.g. `SplitRight`. Case-insensitive.
        name: String,
    },
    /// List the tabs of every window.
    ListTabs,
    /// List the split panes of a tab.
    ListPanes {
        /// Tab index. Defaults to the active tab.
        #[arg(long)]
        tab: Option<usize>,
    },
    /// Print a pane's text.
    GetText {
        /// Tab index. Defaults to the active tab.
        #[arg(long)]
        tab: Option<usize>,
        /// Pane id. Defaults to the tab's focused pane.
        #[arg(long)]
        pane: Option<u32>,
        /// Include scrollback history, not just the visible screen.
        #[arg(long)]
        scrollback: bool,
    },
    /// Print the last command a pane ran, with its output and exit code.
    /// Requires shell integration (OSC 133).
    LastCommand {
        /// Tab index. Defaults to the active tab.
        #[arg(long)]
        tab: Option<usize>,
        /// Pane id. Defaults to the tab's focused pane.
        #[arg(long)]
        pane: Option<u32>,
        /// Cap on output lines (the tail is kept).
        #[arg(long)]
        max_lines: Option<usize>,
    },
    /// Type text at a pane's prompt. Does NOT press Enter unless `--submit` is
    /// passed and `integration.control_api.allow_submit` is enabled.
    SendText {
        /// The text to type.
        text: String,
        /// Tab index. Defaults to the active tab.
        #[arg(long)]
        tab: Option<usize>,
        /// Pane id. Defaults to the tab's focused pane.
        #[arg(long)]
        pane: Option<u32>,
        /// Also press Enter, actually running the command.
        #[arg(long)]
        submit: bool,
    },
    /// Send key presses, e.g. `ctrl+c`, `escape`, `"down down enter"`.
    SendKeys {
        /// Space-separated key specs.
        keys: String,
        /// Tab index. Defaults to the active tab.
        #[arg(long)]
        tab: Option<usize>,
        /// Pane id. Defaults to the tab's focused pane.
        #[arg(long)]
        pane: Option<u32>,
    },
    /// Write a PNG of the focused window to a file.
    Screenshot {
        /// Destination path. Relative paths are resolved against your working
        /// directory before being sent.
        path: PathBuf,
    },
}

impl CtlCommand {
    /// Translate the CLI shape into a wire request.
    fn to_request(&self) -> std::io::Result<ControlRequest> {
        Ok(match self {
            Self::Ping => ControlRequest::Ping,
            Self::Version => ControlRequest::Version,
            Self::ToggleQuake => ControlRequest::ToggleQuake,
            Self::ListActions => ControlRequest::ListActions,
            Self::Action { name } => ControlRequest::Action { name: name.clone() },
            Self::ListTabs => ControlRequest::ListTabs,
            Self::ListPanes { tab } => ControlRequest::ListPanes { tab: *tab },
            Self::GetText {
                tab,
                pane,
                scrollback,
            } => ControlRequest::GetText {
                tab: *tab,
                pane: *pane,
                scrollback: *scrollback,
            },
            Self::LastCommand {
                tab,
                pane,
                max_lines,
            } => ControlRequest::LastCommand {
                tab: *tab,
                pane: *pane,
                max_lines: *max_lines,
            },
            Self::SendText {
                text,
                tab,
                pane,
                submit,
            } => ControlRequest::SendText {
                text: text.clone(),
                tab: *tab,
                pane: *pane,
                submit: *submit,
            },
            Self::SendKeys { keys, tab, pane } => ControlRequest::SendKeys {
                keys: keys.clone(),
                tab: *tab,
                pane: *pane,
            },
            // The running instance has its own working directory, so a relative
            // path would land somewhere the caller did not mean. Resolve it here,
            // where "here" is still the caller's shell.
            Self::Screenshot { path } => ControlRequest::Screenshot {
                path: std::path::absolute(path)?,
            },
        })
    }
}

/// Run one `terminale ctl` subcommand and exit the process with its status.
///
/// Prints the reply's payload as pretty JSON on success (so it pipes into `jq`),
/// and the refusal on stderr with exit code 1 on failure. `get-text` is special
/// cased to print the text itself rather than a JSON string, because reading a
/// pane from a shell script is the common case.
pub(crate) fn run_ctl(cmd: &CtlCommand) -> ! {
    let request = match cmd.to_request() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("terminale ctl: {e}");
            std::process::exit(2);
        }
    };
    let reply = match send_request(&request) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "could not reach a running terminale on {}: {e}\n\
                 Start terminale first, or check that `integration.control_socket` is enabled.",
                socket_path().display()
            );
            std::process::exit(1);
        }
    };
    if !reply.ok {
        eprintln!(
            "terminale ctl: {}",
            reply.error.as_deref().unwrap_or("request failed")
        );
        std::process::exit(1);
    }

    // A screenshot is written by the render thread on its next frame, so the
    // file does not exist yet when the reply arrives. Wait for it, so the exit
    // status means what a script expects it to mean.
    if let ControlRequest::Screenshot { path } = &request {
        match wait_for_file(path) {
            Ok(()) => println!("{}", path.display()),
            Err(e) => {
                eprintln!("terminale ctl: {e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }

    match (&request, &reply.data) {
        (ControlRequest::GetText { .. }, Some(data)) => {
            if let Some(text) = data.get("text").and_then(serde_json::Value::as_str) {
                println!("{text}");
            }
        }
        (_, Some(data)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
            );
        }
        (_, None) => println!("ok"),
    }
    std::process::exit(0);
}

/// Block until `path` appears and stops growing, or give up.
///
/// The renderer writes the PNG in one go after the frame it captured is
/// presented, so "exists and is non-empty" is a sufficient signal; the size
/// check guards against reading a file mid-write on a slow disk.
fn wait_for_file(path: &std::path::Path) -> Result<(), String> {
    /// Total time to wait for the next frame to be captured and written. A
    /// hidden or fully-occluded window may not redraw promptly, so this is
    /// generous rather than snappy.
    const DEADLINE: Duration = Duration::from_secs(5);
    /// How often to look.
    const POLL: Duration = Duration::from_millis(50);

    let started = std::time::Instant::now();
    let mut last_len = 0u64;
    while started.elapsed() < DEADLINE {
        if let Ok(meta) = std::fs::metadata(path) {
            let len = meta.len();
            if len > 0 && len == last_len {
                return Ok(());
            }
            last_len = len;
        }
        std::thread::sleep(POLL);
    }
    Err(format!(
        "timed out waiting for {} — the window may not be redrawing, \
         or this GPU/surface cannot be captured (see the terminale log)",
        path.display()
    ))
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
