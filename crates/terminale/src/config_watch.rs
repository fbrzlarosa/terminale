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

/// Small debounce added inside the event callback: wait this many milliseconds
/// after a filesystem event before forwarding the reload signal to the UI
/// thread. Smooths out bursts (e.g. editors that truncate + rewrite a file in
/// two rapid operations) without adding significant latency for normal saves.
const CALLBACK_DEBOUNCE_MS: u64 = 150;

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

    // notify v6 uses a channel-based API. We create a watcher that calls a
    // closure on each event batch. The closure debounces by sleeping briefly
    // then sending a single wake signal — a cheap but effective strategy for
    // the typical "editor saves once" case.
    let proxy_clone = proxy.clone();
    let path_clone = config_path.clone();

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
                    // Brief debounce: wait a moment before signalling the UI
                    // thread so bursts of rapid filesystem events (e.g. an
                    // editor that truncates then rewrites) coalesce into one
                    // reload.
                    std::thread::sleep(Duration::from_millis(CALLBACK_DEBOUNCE_MS));
                    if proxy_clone.send_event(UserEvent::ConfigChanged).is_err() {
                        // Event loop is gone — nothing left to notify.
                    }
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
