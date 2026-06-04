//! Session lifecycle: spawning a shell behind a pseudo-terminal and exposing
//! the I/O channels that higher layers consume.

use crate::error::{CoreError, CoreResult};
use bytes::Bytes;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtyPair, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;

/// Launch-time settings for a session. Mirrors `terminale_config::Profile`
/// without depending on that crate (keeps `terminale-core` framework-agnostic).
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// Executable path or PATH-resolvable name.
    pub command: String,
    /// CLI arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: HashMap<String, String>,
    /// Working directory; defaults to the parent process cwd.
    pub cwd: Option<PathBuf>,
}

impl SpawnSpec {
    /// Build a minimal spec that just launches `command` with no args.
    #[must_use]
    pub fn just(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
        }
    }
}

/// Stable identifier for a [`Session`] within a single process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(u64);

impl SessionId {
    /// Allocate a fresh, unique [`SessionId`].
    #[must_use]
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The raw identifier value.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Initial PTY size in cells (cols, rows) when no override is provided.
pub const DEFAULT_PTY_SIZE: (u16, u16) = (80, 24);

/// Hook fired by the PTY reader after each chunk lands in the output
/// channel. Use this to wake the host event loop so the new bytes get
/// rendered with minimum latency.
pub type DataNotifier = Arc<dyn Fn() + Send + Sync>;

/// Sends bytes to a remote backend's stdin. Used by [`Session::from_remote`]
/// to bridge a non-PTY transport (e.g. SSH) onto the same write surface a
/// PTY-backed session exposes.
pub type RemoteWriter = Arc<dyn Fn(&[u8]) -> CoreResult<()> + Send + Sync>;

/// Resizes a remote backend's PTY (server-side window change). Used by
/// [`Session::from_remote`].
pub type RemoteResizer = Arc<dyn Fn(u16, u16) -> CoreResult<()> + Send + Sync>;

/// How a [`Session`]'s I/O is fulfilled. The public `Session` API
/// (`write_input`, `resize`, `take_output`, `size`, `id`) is identical for
/// both, so higher layers (e.g. a tab) never need to know which one they
/// hold — that's what lets an SSH tab reuse the exact same plumbing.
enum Backend {
    /// Local shell behind a pseudo-terminal.
    Pty {
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        /// Child process handle — kept alive so `try_wait` can poll for the
        /// real OS exit code and `CloseOnCleanExit` can act on it.
        child: Arc<Mutex<Box<dyn Child + Send>>>,
    },
    /// A remote transport (e.g. SSH) bridged onto the session surface via
    /// closures that forward writes/resizes to the remote channel.
    Remote {
        write: RemoteWriter,
        resize: RemoteResizer,
    },
}

/// A live shell session, either backed by a local pseudo-terminal or by a
/// remote transport (SSH) bridged onto the same I/O surface.
///
/// For PTY sessions the master lives behind a mutex so resizes and writes can
/// come from any thread (e.g. winit event loop on the main thread, render
/// thread, etc.). A background reader thread copies PTY output into an `mpsc`
/// channel. Remote sessions instead forward writes/resizes through closures
/// and hand back a receiver fed by the transport's own pump task.
pub struct Session {
    id: SessionId,
    cols: u16,
    rows: u16,
    backend: Backend,
    output_rx: Option<mpsc::UnboundedReceiver<Bytes>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.backend {
            Backend::Pty { .. } => "pty",
            Backend::Remote { .. } => "remote",
        };
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("backend", &kind)
            .finish()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // PTY teardown can block indefinitely, so it must NEVER run on the
        // calling thread (typically the winit event loop). On Windows,
        // closing the ConPTY pseudo-console blocks until the console host
        // (OpenConsole.exe) drains and exits — and the host stays alive while
        // ANY process in the child's tree keeps a handle to the pseudo
        // console. `kill()` only terminates the direct child (the shell), so
        // a tab running e.g. a dev server kept the host alive and the master
        // drop froze the whole event loop until Windows reported the app as
        // hung and killed it (WER `AppHangXProcB1` on OpenConsole.exe).
        //
        // So: swap the backend out for an inert placeholder and hand the
        // real teardown (child kill + master/pseudo-console close) to a
        // detached reaper thread. The worst case is now a leaked background
        // thread + console host instead of a dead app.
        //
        // For remote (SSH) backends there is no local child to kill and no
        // pseudo-console to close — dropping the closures is trivially cheap,
        // so those tear down inline.
        let backend = std::mem::replace(
            &mut self.backend,
            Backend::Remote {
                write: Arc::new(|_: &[u8]| Ok(())),
                resize: Arc::new(|_, _| Ok(())),
            },
        );
        if let Backend::Pty { child, .. } = &backend {
            let child = Arc::clone(child);
            let spawned = thread::Builder::new()
                .name("terminale-pty-reaper".into())
                .spawn(move || {
                    // Blocking lock is fine here — we're off the UI thread.
                    // Killing the child makes the reader thread's pending
                    // `read()` return EOF, which lets the pseudo-console
                    // close cleanly when `backend` (the master) drops below.
                    let _ = child.lock().kill();
                    // Reap the child before closing the pseudo-console:
                    // TerminateProcess is asynchronous, and closing the
                    // console while the client is still dying is what leaves
                    // wedged console-host processes spinning at 100% CPU.
                    // Blocking is fine on this thread; kill() above makes the
                    // wait bounded in practice.
                    let _ = child.lock().wait();
                    drop(backend);
                });
            if let Err(e) = spawned {
                // Reaper thread failed to spawn (resource exhaustion). The
                // backend was moved into the closure which never ran, so it
                // drops right here on the calling thread — same behaviour as
                // before this fix, and strictly better than leaking the PTY.
                tracing::warn!(?e, "pty reaper thread spawn failed; tearing down inline");
            }
        }
    }
}

impl Session {
    /// Spawn the platform's default shell behind a PTY.
    ///
    /// On Windows this is `pwsh.exe` (falling back to `powershell.exe`).
    /// On Unix it picks `$SHELL`, falling back to `/bin/bash`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Pty`] when the underlying PTY system cannot
    /// allocate a master/slave pair, or when the child shell fails to spawn.
    pub fn spawn_default(cols: u16, rows: u16) -> CoreResult<Self> {
        let shell = default_shell();
        Self::spawn(&shell, cols, rows)
    }

    /// Spawn the given shell path behind a PTY (no extra args / env).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Pty`] when the underlying PTY system cannot
    /// allocate a master/slave pair, or when the child shell fails to spawn.
    pub fn spawn(shell: &str, cols: u16, rows: u16) -> CoreResult<Self> {
        Self::spawn_with(&SpawnSpec::just(shell), cols, rows)
    }

    /// Same as [`Self::spawn_with`] but the reader thread invokes `notifier()`
    /// after every chunk it pushes to the output channel. Use this to wake
    /// the host event loop and keep input echo latency low.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Pty`] when the underlying PTY system cannot
    /// allocate a master/slave pair, or when the child shell fails to spawn.
    pub fn spawn_with_notifier(
        spec: &SpawnSpec,
        cols: u16,
        rows: u16,
        notifier: DataNotifier,
    ) -> CoreResult<Self> {
        Self::spawn_inner(spec, cols, rows, Some(notifier))
    }

    /// Spawn a shell described by a `SpawnSpec` (command + args + env + cwd).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Pty`] when the underlying PTY system cannot
    /// allocate a master/slave pair, or when the child shell fails to spawn.
    pub fn spawn_with(spec: &SpawnSpec, cols: u16, rows: u16) -> CoreResult<Self> {
        Self::spawn_inner(spec, cols, rows, None)
    }

    fn spawn_inner(
        spec: &SpawnSpec,
        cols: u16,
        rows: u16,
        notifier: Option<DataNotifier>,
    ) -> CoreResult<Self> {
        let pty_system = native_pty_system();
        let PtyPair { master, slave } = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| CoreError::Pty(format!("openpty failed: {e}")))?;

        let mut cmd = CommandBuilder::new(&spec.command);
        for arg in &spec.args {
            cmd.arg(arg);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        // Set terminfo-friendly env so apps know our capabilities.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = slave
            .spawn_command(cmd)
            .map_err(|e| CoreError::Pty(format!("spawn `{}` failed: {e}", spec.command)))?;
        drop(slave); // we keep only the master end open

        let writer = master
            .take_writer()
            .map_err(|e| CoreError::Pty(format!("take_writer failed: {e}")))?;
        let mut reader = master
            .try_clone_reader()
            .map_err(|e| CoreError::Pty(format!("try_clone_reader failed: {e}")))?;

        let (output_tx, output_rx) = mpsc::unbounded_channel::<Bytes>();
        thread::Builder::new()
            .name("terminale-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            tracing::debug!("pty reader: EOF");
                            break;
                        }
                        Ok(n) => {
                            if output_tx.send(Bytes::copy_from_slice(&buf[..n])).is_err() {
                                tracing::debug!("pty reader: receiver dropped");
                                break;
                            }
                            if let Some(n) = notifier.as_ref() {
                                n();
                            }
                        }
                        Err(e) => {
                            tracing::warn!(?e, "pty read error");
                            break;
                        }
                    }
                }
            })
            .map_err(|e| CoreError::Pty(format!("failed to spawn reader thread: {e}")))?;

        Ok(Self {
            id: SessionId::new(),
            cols,
            rows,
            backend: Backend::Pty {
                master: Arc::new(Mutex::new(master)),
                writer: Arc::new(Mutex::new(writer)),
                child: Arc::new(Mutex::new(child)),
            },
            output_rx: Some(output_rx),
        })
    }

    /// Wrap an already-connected remote transport (e.g. an SSH channel) as a
    /// [`Session`], so a tab can drive it through the exact same
    /// `write_input` / `resize` / `take_output` surface a PTY session uses.
    ///
    /// `output_rx` is the receiver the transport's pump task feeds remote
    /// bytes into; `write` forwards keystrokes/paste to the remote stdin and
    /// `resize` issues a server-side window change. Both closures run
    /// synchronously and should themselves be cheap (the SSH wrapper just
    /// pushes onto an unbounded channel drained by its own Tokio task).
    #[must_use]
    pub fn from_remote(
        cols: u16,
        rows: u16,
        output_rx: mpsc::UnboundedReceiver<Bytes>,
        write: RemoteWriter,
        resize: RemoteResizer,
    ) -> Self {
        Self {
            id: SessionId::new(),
            cols,
            rows,
            backend: Backend::Remote { write, resize },
            output_rx: Some(output_rx),
        }
    }

    /// Stable identifier for this session.
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// `true` when this session is backed by a remote transport (SSH)
    /// rather than a local PTY. Callers use this to avoid assuming the
    /// local OS/shell apply to what's running in the session.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        matches!(self.backend, Backend::Remote { .. })
    }

    /// Current PTY size in (cols, rows).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Take the output channel. Callable once per session.
    ///
    /// Subsequent callers get `None`. The returned receiver is the only way
    /// to drain PTY output; if dropped, the reader thread exits.
    pub fn take_output(&mut self) -> Option<mpsc::UnboundedReceiver<Bytes>> {
        self.output_rx.take()
    }

    /// Resize the session (notifies the running shell via SIGWINCH / ConPTY
    /// for a PTY, or a server-side window change for a remote session).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Pty`] when a PTY backend rejects the new size, or
    /// [`CoreError::Remote`] when a remote backend's channel has closed.
    pub fn resize(&mut self, cols: u16, rows: u16) -> CoreResult<()> {
        self.cols = cols;
        self.rows = rows;
        let result = match &self.backend {
            Backend::Pty { master, .. } => master
                .lock()
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| CoreError::Pty(format!("resize failed: {e}"))),
            Backend::Remote { resize, .. } => resize(cols, rows),
        };
        tracing::debug!(cols, rows, ok = result.is_ok(), "session resize");
        result
    }

    /// Send bytes to the shell stdin (typed keys, paste, etc.).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Io`] when writing to a PTY master fails (typically
    /// because the child has exited and closed its end of the PTY), or
    /// [`CoreError::Remote`] when a remote backend's channel has closed.
    pub fn write_input(&self, data: &[u8]) -> CoreResult<()> {
        match &self.backend {
            Backend::Pty { writer, .. } => {
                let mut w = writer.lock();
                w.write_all(data)?;
                w.flush()?;
                Ok(())
            }
            Backend::Remote { write, .. } => write(data),
        }
    }

    /// Poll for the child process's exit status without blocking.
    ///
    /// Returns `Some(code)` when the child has exited and the OS reported a
    /// numeric exit code (Windows / Unix exit(N)). Returns `None` when the
    /// child is still running, the session is a remote backend, or the
    /// platform did not provide a numeric code (e.g. killed by signal on
    /// Unix — those are treated the same as an unknown status).
    ///
    /// This is safe to call multiple times; the child handle's `try_wait`
    /// is non-destructive (the handle stays valid after the call).
    #[must_use]
    pub fn try_exit_status(&self) -> Option<i32> {
        match &self.backend {
            Backend::Pty { child, .. } => child
                .lock()
                .try_wait()
                .ok()
                .flatten()
                .map(|s| s.exit_code() as i32),
            Backend::Remote { .. } => None,
        }
    }

    /// OS process id of the local shell child, if this is a PTY session and the
    /// child is still known. `None` for remote (SSH) sessions or once the
    /// child handle can no longer report a pid. Useful for querying the
    /// shell's working directory from the OS as a restore fallback.
    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        match &self.backend {
            Backend::Pty { child, .. } => child.lock().process_id(),
            Backend::Remote { .. } => None,
        }
    }
}

fn default_shell() -> String {
    if cfg!(windows) {
        // pwsh (PowerShell 7) if available, else legacy powershell.
        which("pwsh.exe")
            .or_else(|| which("powershell.exe"))
            .unwrap_or_else(|| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

fn which(exe: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn default_shell_resolves_to_something() {
        let shell = default_shell();
        assert!(!shell.is_empty());
    }

    /// Regression: dropping a freshly-spawned PTY session must not dead-lock.
    ///
    /// On Windows, closing the ConPTY pseudo-console blocks until the reader
    /// thread's pending blocking `read()` returns; while the child is alive and
    /// silent it never does, so the [`Drop`] impl kills the child first. If that
    /// regresses, the drop hangs — we run it on a worker thread and fail via a
    /// timeout instead of hanging the whole test binary.
    #[test]
    fn dropping_freshly_spawned_session_does_not_hang() {
        use std::sync::mpsc;
        use std::time::Duration;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(session) = Session::spawn_default(80, 24) {
                drop(session); // must return promptly, not dead-lock
            }
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(10)).is_ok(),
            "spawning then dropping a Session dead-locked (ConPTY teardown)"
        );
    }

    /// Regression for the tab-close hang: `Session::drop` must return
    /// (nearly) immediately on the calling thread — the blocking ConPTY
    /// teardown happens on the detached reaper thread. Before this fix the
    /// drop ran `ClosePseudoConsole` inline, which blocks until the console
    /// host exits; with any descendant process still attached that meant the
    /// event loop froze and Windows killed the app as hung.
    #[test]
    fn session_drop_returns_promptly_on_calling_thread() {
        use std::time::{Duration, Instant};
        let Ok(session) = Session::spawn_default(80, 24) else {
            return; // no shell available in this environment — skip
        };
        let start = Instant::now();
        drop(session);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Session::drop blocked the calling thread for {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn try_exit_status_returns_none_for_remote_session() {
        // Remote sessions have no OS child handle, so try_exit_status must
        // always return None regardless of the session state.
        let (tx, rx) = mpsc::unbounded_channel::<Bytes>();
        drop(tx); // immediately disconnected — simulates a closed channel
        let write: RemoteWriter = Arc::new(|_: &[u8]| Ok(()));
        let resize: RemoteResizer = Arc::new(|_, _| Ok(()));
        let session = Session::from_remote(80, 24, rx, write, resize);
        assert_eq!(
            session.try_exit_status(),
            None,
            "remote sessions must always return None from try_exit_status"
        );
    }

    #[test]
    fn remote_backend_bridges_io() {
        use std::sync::atomic::{AtomicU16, AtomicUsize};

        let (tx, rx) = mpsc::unbounded_channel::<Bytes>();
        let written = Arc::new(Mutex::new(Vec::<u8>::new()));
        let resized = Arc::new((AtomicU16::new(0), AtomicU16::new(0)));
        let write_calls = Arc::new(AtomicUsize::new(0));

        let w = Arc::clone(&written);
        let wc = Arc::clone(&write_calls);
        let write: RemoteWriter = Arc::new(move |data: &[u8]| {
            w.lock().extend_from_slice(data);
            wc.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let r = Arc::clone(&resized);
        let resize: RemoteResizer = Arc::new(move |cols: u16, rows: u16| {
            r.0.store(cols, Ordering::Relaxed);
            r.1.store(rows, Ordering::Relaxed);
            Ok(())
        });

        let mut session = Session::from_remote(80, 24, rx, write, resize);
        assert_eq!(session.size(), (80, 24));

        // write_input forwards to the remote writer.
        session.write_input(b"ls -la\n").unwrap();
        assert_eq!(&*written.lock(), b"ls -la\n");
        assert_eq!(write_calls.load(Ordering::Relaxed), 1);

        // resize forwards cols/rows and updates the cached size.
        session.resize(120, 40).unwrap();
        assert_eq!(session.size(), (120, 40));
        assert_eq!(resized.0.load(Ordering::Relaxed), 120);
        assert_eq!(resized.1.load(Ordering::Relaxed), 40);

        // The output receiver handed in is exactly the one returned.
        tx.send(Bytes::from_static(b"hello")).unwrap();
        let mut out = session.take_output().expect("remote session has output");
        assert_eq!(out.try_recv().unwrap(), Bytes::from_static(b"hello"));
    }
}
