//! Filesystem watcher for `config.toml`.
//!
//! When `window.auto_reload_config` is enabled, a background thread uses the
//! [`notify`] crate to watch the user's config file for modifications. Each
//! debounced change event posts a [`crate::UserEvent::ConfigChanged`] to the
//! winit event loop so the main thread can reload and live-apply the new
//! config without blocking.
//!
//! The watcher lifecycle mirrors the Quake-hotkey forwarder: it lives on a
//! dedicated thread and only communicates with the UI thread through the
//! existing `EventLoopProxy<UserEvent>`.

use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Quiet period a burst of filesystem events must be followed by before one
/// reload is forwarded to the UI thread.
///
/// A single save is never a single event: on Linux inotify reports the write,
/// the metadata update and the close separately, and editors that truncate +
/// rewrite (or write a temp file and rename it over the original) add more. The
/// debounce thread waits for `DEBOUNCE` of silence and then sends exactly one
/// signal, so one save is one reload — previously each event produced its own,
/// and a Settings save reloaded (and re-read the OS keychain) three times in a
/// row.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Start a filesystem watcher for `config_path`. Returns the live watcher
/// handle — the caller must keep it alive for as long as watching is wanted.
/// Dropping it unregisters the watch.
///
/// The watcher is gated on `enabled`: when `false` this function is a no-op
/// and returns `None`. The caller should call this again (and drop the old
/// handle) whenever `window.auto_reload_config` is toggled.
pub(crate) fn start(
    config_path: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
    enabled: bool,
) -> Option<RecommendedWatcher> {
    if !enabled {
        return None;
    }

    // notify v6 calls our closure on every event. The closure does no work
    // beyond filtering and a non-blocking hand-off: the debounce lives on its
    // own thread, which collapses a burst of events into a single reload.
    //
    // Doing it the other way round (sleeping inside the callback) is what the
    // watcher used to do, and it made things worse rather than better: notify
    // dispatches events serially, so N events became N sleeps AND N reloads,
    // spaced one debounce apart.
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let path_clone = config_path.clone();

    std::thread::Builder::new()
        .name("terminale-config-debounce".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                // Drain everything that arrives within a quiet period, so a
                // multi-event save collapses into one signal.
                while rx.recv_timeout(DEBOUNCE).is_ok() {}
                if proxy.send_event(UserEvent::ConfigChanged).is_err() {
                    // Event loop is gone — nothing left to notify.
                    break;
                }
            }
        })
        .ok();

    let watcher_result = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            match result {
                Ok(event) => {
                    use notify::EventKind;
                    // Only react to content-modifying events.
                    let is_modify = matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    );
                    if !is_modify {
                        return;
                    }
                    // Check if the event path matches (some OSes report the
                    // canonical path; we do a best-effort suffix check).
                    let relevant = event
                        .paths
                        .iter()
                        .any(|p| p == &path_clone || p.file_name() == path_clone.file_name());
                    if !relevant {
                        return;
                    }
                    // Never blocks: the debounce thread owns the timing.
                    let _ = tx.send(());
                }
                Err(e) => {
                    tracing::warn!(?e, "config watcher error");
                }
            }
        },
        NotifyConfig::default(),
    );

    match watcher_result {
        Ok(mut watcher) => {
            // Watch the parent directory rather than the file itself. Some
            // editors (vim, Emacs) write a temp file then rename it over the
            // original; watching the directory ensures we catch the rename
            // event even when the file inode changes.
            let watch_dir = config_path
                .parent()
                .map_or_else(|| config_path.clone(), std::path::Path::to_path_buf);
            if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                tracing::warn!(
                    ?e,
                    path = %watch_dir.display(),
                    "could not start config file watcher"
                );
                return None;
            }
            tracing::debug!(
                path = %config_path.display(),
                "config hot-reload watcher started"
            );
            Some(watcher)
        }
        Err(e) => {
            tracing::warn!(?e, "could not create config file watcher");
            None
        }
    }
}
