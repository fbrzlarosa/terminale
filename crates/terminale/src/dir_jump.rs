//! Directory-jump frecency store.
//!
//! Tracks directories visited via OSC 7 cwd reports and ranks them by a
//! combined frequency + recency score ("frecency"). The ranked list is
//! surfaced in the [`PaletteMode::DirectoryJump`] picker so the user can
//! fuzzy-search and jump the active shell to any tracked directory without
//! a third-party tool.
//!
//! # Frecency formula
//!
//! For each entry:
//!   `score = visit_count * recency_weight`
//!
//! where `recency_weight` is:
//!   * **4** — visited within the last hour
//!   * **2** — visited within the last day
//!   * **1** — visited within the last week
//!   * **0.5** — older than a week
//!
//! This mirrors the scoring used by many shell-based frecency tools and
//! gives a strong boost to recently-used directories while still surfacing
//! frequently-used old ones.
//!
//! # Persistence
//!
//! When `config.directory_jump.persist` is `true`, the store is written to
//! `<data_dir>/dir_history.toml` after every update. The file is a flat TOML
//! array; loading it at startup re-populates the in-memory store so the
//! history survives restarts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── On-disk entry ─────────────────────────────────────────────────────────────

/// A single directory entry in the persistent history file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct DirEntry {
    /// Canonical path string.
    pub path: String,
    /// Total number of visits recorded.
    pub visit_count: u64,
    /// Unix timestamp (seconds) of the most recent visit.
    pub last_seen_unix: i64,
}

impl DirEntry {
    /// Compute the frecency score used for ranking.
    ///
    /// Higher is better. Combines visit frequency with a recency multiplier
    /// that decays in four steps: last hour → last day → last week → older.
    pub(crate) fn frecency(&self, now_unix: i64) -> f64 {
        let age_secs = (now_unix - self.last_seen_unix).max(0);
        let recency_weight = if age_secs < 3_600 {
            4.0 // within the last hour
        } else if age_secs < 86_400 {
            2.0 // within the last day
        } else if age_secs < 604_800 {
            1.0 // within the last week
        } else {
            0.5 // older
        };
        self.visit_count as f64 * recency_weight
    }
}

// ── Disk format ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    dirs: Vec<DirEntry>,
}

// ── DirJumpStore ──────────────────────────────────────────────────────────────

/// In-memory frecency store for visited directories.
///
/// Constructed once on startup (optionally loading from disk), then updated
/// each time an OSC 7 cwd report arrives. Call [`DirJumpStore::ranked`] to get
/// the sorted list for the picker.
#[derive(Debug, Default)]
pub(crate) struct DirJumpStore {
    entries: Vec<DirEntry>,
}

impl DirJumpStore {
    /// Create an empty store.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Load the store from the on-disk history file at `path`.
    ///
    /// Returns an empty store on any I/O or parse error (non-fatal — history
    /// starts fresh).
    pub(crate) fn load(path: &std::path::Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::new();
        };
        let Ok(file) = toml::from_str::<HistoryFile>(&text) else {
            tracing::warn!(
                path = %path.display(),
                "dir_history.toml is malformed; starting with an empty store"
            );
            return Self::new();
        };
        Self {
            entries: file.dirs,
        }
    }

    /// Persist the current store to `path`.
    ///
    /// Creates parent directories as needed. Failures are logged at `warn`
    /// but never propagate to the caller — persistence is best-effort.
    pub(crate) fn save(&self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!(?e, path = %path.display(), "dir_history: could not create data dir");
                    return;
                }
            }
        }
        let file = HistoryFile {
            dirs: self.entries.clone(),
        };
        let text = match toml::to_string_pretty(&file) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(?e, "dir_history: serialization failed");
                return;
            }
        };
        if let Err(e) = std::fs::write(path, text) {
            tracing::warn!(?e, path = %path.display(), "dir_history: write failed");
        }
    }

    /// Record a visit to `path` at `now_unix` (seconds since epoch).
    ///
    /// If an entry for `path` already exists its `visit_count` and
    /// `last_seen_unix` are updated in place; otherwise a new entry is created.
    /// After the update, if the store exceeds `max_tracked` the entry with the
    /// lowest frecency score is evicted.
    ///
    /// Returns `true` when the store changed (so the caller knows to persist).
    pub(crate) fn record(&mut self, path: &str, now_unix: i64, max_tracked: usize) -> bool {
        if path.is_empty() {
            return false;
        }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
            // Dedup: update existing entry.
            let changed = entry.last_seen_unix != now_unix || entry.visit_count == 0;
            entry.visit_count += 1;
            entry.last_seen_unix = now_unix;
            // Evict only if we were already at capacity (no new entry added).
            self.evict_if_over(max_tracked, now_unix);
            return changed;
        }
        // New entry.
        self.entries.push(DirEntry {
            path: path.to_string(),
            visit_count: 1,
            last_seen_unix: now_unix,
        });
        self.evict_if_over(max_tracked, now_unix);
        true
    }

    /// Evict the entry with the lowest frecency score when the store exceeds
    /// `max_tracked`. A no-op when within capacity.
    fn evict_if_over(&mut self, max_tracked: usize, now_unix: i64) {
        while self.entries.len() > max_tracked {
            let min_idx = self
                .entries
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    a.frecency(now_unix)
                        .partial_cmp(&b.frecency(now_unix))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i);
            if let Some(i) = min_idx {
                self.entries.swap_remove(i);
            } else {
                break;
            }
        }
    }

    /// Return a sorted snapshot of the store (highest frecency first).
    ///
    /// The snapshot is a `Vec<String>` of path strings, ready to be used as
    /// palette candidates.
    pub(crate) fn ranked(&self, now_unix: i64) -> Vec<String> {
        let mut scored: Vec<(&DirEntry, f64)> = self
            .entries
            .iter()
            .map(|e| (e, e.frecency(now_unix)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(e, _)| e.path.clone()).collect()
    }

    /// Number of entries in the store (for tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

// ── Shell-safe cd payload ─────────────────────────────────────────────────────

/// Build the PTY payload that changes the shell's working directory to `path`.
///
/// The path is single-quoted for POSIX shells (sh, bash, zsh, fish). On
/// Windows PowerShell / cmd, single-quoting is also safe for `Set-Location` /
/// `cd`. Any embedded single-quote characters in the path are escaped by
/// ending the quoted string, inserting a literal `'`, and resuming the quote —
/// the standard POSIX single-quote escape idiom.
///
/// The payload ends with `\n` (Enter) so the command executes immediately.
///
/// # Examples
///
/// ```
/// // Normal path
/// assert_eq!(build_cd_payload("/home/user/projects"), "cd '/home/user/projects'\n");
/// // Path with a single-quote
/// assert_eq!(build_cd_payload("/it's/here"), "cd '/it'\\''s/here'\n");
/// ```
pub(crate) fn build_cd_payload(path: &str) -> String {
    // Single-quote the path, escaping any embedded single-quotes.
    let escaped = path.replace('\'', "'\\''");
    format!("cd '{escaped}'\n")
}

// ── dir_history_path convenience re-export ───────────────────────────────────

/// Convenience: the on-disk path for the history file (from `terminale_config::paths`).
pub(crate) fn history_path() -> Option<PathBuf> {
    terminale_config::paths::dir_history_path()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── frecency scoring ──────────────────────────────────────────────────────

    #[test]
    fn fresh_visit_within_hour_scores_4() {
        let e = DirEntry {
            path: "/tmp".into(),
            visit_count: 1,
            last_seen_unix: 1_000,
        };
        let score = e.frecency(1_000 + 100); // 100s ago — within the hour
        assert!((score - 4.0).abs() < 1e-9, "score={score}");
    }

    #[test]
    fn visit_within_day_scores_2() {
        let e = DirEntry {
            path: "/tmp".into(),
            visit_count: 1,
            last_seen_unix: 0,
        };
        let score = e.frecency(7200); // 2 hours ago — within the day but not the hour
        assert!((score - 2.0).abs() < 1e-9, "score={score}");
    }

    #[test]
    fn visit_within_week_scores_1() {
        let e = DirEntry {
            path: "/tmp".into(),
            visit_count: 1,
            last_seen_unix: 0,
        };
        let score = e.frecency(172_800); // 2 days ago
        assert!((score - 1.0).abs() < 1e-9, "score={score}");
    }

    #[test]
    fn old_visit_scores_half() {
        let e = DirEntry {
            path: "/tmp".into(),
            visit_count: 1,
            last_seen_unix: 0,
        };
        let score = e.frecency(700_000); // ~8 days ago
        assert!((score - 0.5).abs() < 1e-9, "score={score}");
    }

    #[test]
    fn high_count_recent_beats_low_count_recent() {
        let now = 1_000_000i64;
        let frequent = DirEntry {
            path: "/frequent".into(),
            visit_count: 20,
            last_seen_unix: now - 60, // 1 min ago — within hour
        };
        let rare = DirEntry {
            path: "/rare".into(),
            visit_count: 1,
            last_seen_unix: now - 60,
        };
        assert!(frequent.frecency(now) > rare.frecency(now));
    }

    #[test]
    fn recent_beats_old_same_count() {
        let now = 1_000_000i64;
        let recent = DirEntry {
            path: "/recent".into(),
            visit_count: 5,
            last_seen_unix: now - 60, // within hour
        };
        let old = DirEntry {
            path: "/old".into(),
            visit_count: 5,
            last_seen_unix: now - 700_000, // older than a week
        };
        assert!(recent.frecency(now) > old.frecency(now));
    }

    // ── store record / dedup / cap / evict ────────────────────────────────────

    #[test]
    fn record_inserts_new_entry() {
        let mut store = DirJumpStore::new();
        store.record("/home/user", 1000, 200);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn record_deduplicates_path() {
        let mut store = DirJumpStore::new();
        store.record("/home/user", 1000, 200);
        store.record("/home/user", 2000, 200);
        assert_eq!(store.len(), 1, "same path must not create a second entry");
        let entry = &store.entries[0];
        assert_eq!(entry.visit_count, 2);
        assert_eq!(entry.last_seen_unix, 2000);
    }

    #[test]
    fn record_caps_at_max_tracked() {
        let mut store = DirJumpStore::new();
        for i in 0..10 {
            store.record(&format!("/dir/{i}"), 1000 + i as i64, 5);
        }
        assert!(store.len() <= 5, "store must respect max_tracked={} but len={}", 5, store.len());
    }

    #[test]
    fn evict_removes_lowest_frecency() {
        let now = 1_000_000i64;
        let mut store = DirJumpStore::new();
        // Fill to capacity with fairly recent entries.
        for i in 0..5 {
            store.record(&format!("/dir/{i}"), now - 3600, 5);
        }
        // The oldest/least-frequent entry (/dir/0) should be a candidate for
        // eviction.  Add a sixth entry; one must be evicted.
        store.record("/dir/new", now - 60, 5);
        assert_eq!(store.len(), 5, "store must not exceed max_tracked after record");
    }

    #[test]
    fn ranked_is_sorted_highest_first() {
        let now = 1_000_000i64;
        let mut store = DirJumpStore::new();
        // Two visits to /frequent recently, one to /rare a week ago.
        store.record("/frequent", now - 60, 200);
        store.record("/frequent", now - 30, 200);
        store.record("/rare", now - 700_000, 200);
        let ranked = store.ranked(now);
        assert_eq!(ranked[0], "/frequent", "highest-frecency must come first");
    }

    // ── build_cd_payload ──────────────────────────────────────────────────────

    #[test]
    fn cd_payload_simple_path() {
        let p = build_cd_payload("/home/user/projects");
        assert_eq!(p, "cd '/home/user/projects'\n");
    }

    #[test]
    fn cd_payload_escapes_single_quote() {
        let p = build_cd_payload("/it's/here");
        assert_eq!(p, "cd '/it'\\''s/here'\n");
    }

    #[test]
    fn cd_payload_windows_path() {
        let p = build_cd_payload("C:\\Users\\dev");
        assert_eq!(p, "cd 'C:\\Users\\dev'\n");
    }

    #[test]
    fn cd_payload_ends_with_newline() {
        let p = build_cd_payload("/any/path");
        assert!(p.ends_with('\n'), "cd payload must end with newline");
    }

    #[test]
    fn cd_payload_path_with_spaces() {
        let p = build_cd_payload("/home/user/my projects");
        assert_eq!(p, "cd '/home/user/my projects'\n");
    }

    // ── persistence roundtrip ─────────────────────────────────────────────────

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir()
            .join(format!("terminale_dirjump_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dir_history.toml");

        let now = 1_000_000i64;
        let mut store = DirJumpStore::new();
        store.record("/home/user", now - 60, 200);
        store.record("/home/user/projects", now - 3600, 200);
        store.save(&path);

        let loaded = DirJumpStore::load(&path);
        assert_eq!(loaded.len(), 2, "loaded store must have 2 entries");
        let ranked = loaded.ranked(now);
        assert_eq!(ranked[0], "/home/user", "most-recent must rank first");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let store = DirJumpStore::load(std::path::Path::new("/nonexistent/dir_history.toml"));
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn config_roundtrip() {
        use terminale_config::DirectoryJumpConfig;
        let cfg = DirectoryJumpConfig {
            enabled: true,
            max_tracked: 100,
            persist: false,
        };
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrap {
            directory_jump: DirectoryJumpConfig,
        }
        let w = Wrap { directory_jump: cfg.clone() };
        let s = toml::to_string(&w).expect("serialize");
        let back: Wrap = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.directory_jump.enabled, cfg.enabled);
        assert_eq!(back.directory_jump.max_tracked, cfg.max_tracked);
        assert_eq!(back.directory_jump.persist, cfg.persist);
    }
}
