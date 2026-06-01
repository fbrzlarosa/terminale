//! Label-hint quick-select mode and pane-select label mode.
//!
//! ## Quick-select
//! Scans the visible rows + scrollback for regex matches (URLs, file paths,
//! git SHAs, IPv4 addresses, hex colours, UUIDs) and overlays short keyboard
//! labels on each match. The user types label characters; on a full match the
//! matched text is copied to the clipboard and the mode exits. Esc cancels.
//!
//! ## Pane-select
//! Assigns one label per open pane and waits for the user to press that label
//! key; the matching pane receives focus. Esc cancels. Uses the same alphabet
//! and label-assignment logic as quick-select.
//!
//! All logic here is pure (no I/O, no rendering). Integration wiring lives in
//! `main.rs`.

// ── Pattern matching ─────────────────────────────────────────────────────────

/// Default regex patterns for quick-select, as `&str` slices.
/// Callers can override via `QuickSelectConfig::patterns`.
pub const DEFAULT_PATTERNS: &[&str] = &[
    // URLs (http / https / ftp / file / mailto already covered by links.rs,
    // but we list a broad URL pattern so quick-select can work standalone).
    r"https?://[^\s\x00-\x1f\x7f]{2,}",
    r"ftp://[^\s\x00-\x1f\x7f]{2,}",
    r"file://[^\s\x00-\x1f\x7f]{2,}",
    // Git SHA (7-40 hex chars, word-bounded).
    r"\b[0-9a-fA-F]{7,40}\b",
    // IPv4
    r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
    // Hex colours (#rgb / #rrggbb)
    r"#[0-9a-fA-F]{3}(?:[0-9a-fA-F]{3})?\b",
    // UUIDs
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
    // Absolute / home-relative paths that look like filesystem paths.
    // (Existence-check is too expensive here; links.rs does that for
    // Ctrl+click. Quick-select is intentionally less strict.)
    r"(?:~|/)[^\s\x00-\x1f\x7f]{2,}",
    // Windows-style paths.
    r"[A-Za-z]:\\[^\s\x00-\x1f\x7f]{2,}",
];

/// Default label alphabet (home-row-biased): home-row first for the fastest
/// single-key hits, then extending outward.
pub const DEFAULT_ALPHABET: &str = "asdfjklqwerzxcvghtybnuiopm";

// ── One scanned match ────────────────────────────────────────────────────────

/// A single match found by the scanner, with its source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QsMatch {
    /// 0-based row index in the buffer slice passed to `scan`.
    pub row: usize,
    /// 0-based byte offset of the match start within that row's text.
    pub col_start: usize,
    /// 0-based byte offset one past the match end.
    pub col_end: usize,
    /// The matched text, ready for clipboard.
    pub text: String,
}

// ── Scanning ─────────────────────────────────────────────────────────────────

/// Scan a slice of row strings using the provided compiled patterns.
///
/// Returns matches in row-major order. Overlapping matches are skipped
/// (the first match at any position wins). Each match carries its row index
/// and byte offsets so the renderer can position label badges.
///
/// # Arguments
/// * `rows`     — The text content of each visible/scrollback row.
/// * `patterns` — Pre-compiled `Regex` objects. Pass the output of
///   [`compile_patterns`] or your own list.
#[must_use]
pub fn scan(rows: &[&str], patterns: &[regex::Regex]) -> Vec<QsMatch> {
    let mut out: Vec<QsMatch> = Vec::new();
    for (row_idx, row_text) in rows.iter().enumerate() {
        // Collect all (start, end, text) from all patterns, then deduplicate
        // by keeping only the first match at each start position.
        let mut row_matches: Vec<(usize, usize, &str)> = Vec::new();
        for re in patterns {
            for m in re.find_iter(row_text) {
                row_matches.push((m.start(), m.end(), m.as_str()));
            }
        }
        // Sort by start offset, then by length descending so longer matches
        // (UUIDs vs. SHAs) win when they start at the same position.
        row_matches.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

        let mut cursor = 0usize;
        for (start, end, text) in row_matches {
            if start < cursor {
                // Overlaps a previously accepted match — skip.
                continue;
            }
            if text.is_empty() {
                continue;
            }
            out.push(QsMatch {
                row: row_idx,
                col_start: start,
                col_end: end,
                text: text.to_string(),
            });
            cursor = end;
        }
    }
    out
}

/// Compile a list of pattern strings into `Regex` objects.
///
/// Invalid patterns are silently skipped (the caller may choose to log a
/// warning). Use [`validate_patterns`] to surface errors for settings UI.
#[must_use]
pub fn compile_patterns(patterns: &[String]) -> Vec<regex::Regex> {
    patterns
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect()
}

/// Compile the default patterns.
#[must_use]
pub fn default_compiled_patterns() -> Vec<regex::Regex> {
    DEFAULT_PATTERNS
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect()
}

/// Validate a list of pattern strings, returning the first error (if any).
///
/// Returns `None` when all patterns compile successfully.
#[must_use]
pub fn validate_patterns(patterns: &[String]) -> Option<String> {
    for p in patterns {
        if let Err(e) = regex::Regex::new(p) {
            return Some(format!("invalid pattern {p:?}: {e}"));
        }
    }
    None
}

/// Validate an alphabet string: must be non-empty and all characters must be
/// unique. Returns `None` on success, a human-readable message on failure.
#[must_use]
pub fn validate_alphabet(alphabet: &str) -> Option<String> {
    if alphabet.is_empty() {
        return Some("alphabet must not be empty".into());
    }
    let chars: Vec<char> = alphabet.chars().collect();
    let mut seen = std::collections::HashSet::new();
    for c in &chars {
        if !seen.insert(c) {
            return Some(format!("alphabet contains duplicate character '{c}'"));
        }
    }
    None
}

// ── Label assignment ─────────────────────────────────────────────────────────

/// Generate `n` unique labels from `alphabet`, shortest first.
///
/// * When `n ≤ alphabet.len()` each label is one character.
/// * When `n > alphabet.len()` two-character labels are used for any excess.
/// * All labels within this call are unique.
///
/// Labels are assigned in stable order (alphabet-index order) so the visual
/// distribution is deterministic.
#[must_use]
pub fn assign_labels(n: usize, alphabet: &str) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    let chars: Vec<char> = alphabet.chars().collect();
    let a = chars.len();
    if a == 0 {
        return Vec::new();
    }

    let mut labels = Vec::with_capacity(n);

    if n <= a {
        // All single-char labels.
        for ch in chars.iter().take(n) {
            labels.push(ch.to_string());
        }
    } else {
        // Single-char labels for the first `a` entries, two-char for the rest.
        // To keep all labels unique we use: 1-char labels = chars[0..a],
        // 2-char labels = chars[i] + chars[j] skipping any pair whose
        // concatenation collides with a 1-char label (impossible: 2-char ≠
        // 1-char) or a prior 2-char label (guaranteed unique by enumeration).
        for &ch in &chars {
            labels.push(ch.to_string());
        }
        let remaining = n - a;
        let mut count = 0;
        'outer: for &c1 in &chars {
            for &c2 in &chars {
                if count >= remaining {
                    break 'outer;
                }
                labels.push(format!("{c1}{c2}"));
                count += 1;
            }
        }
    }

    labels
}

// ── QuickSelectState ─────────────────────────────────────────────────────────

/// Result of processing one typed character in quick-select mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QsResult {
    /// More characters needed.
    Pending,
    /// A match was uniquely identified; carry its `QsMatch`.
    Hit(QsMatch),
    /// The typed prefix no longer matches any label — no matches remain.
    Miss,
    /// The user pressed Escape; the caller should exit the mode.
    Cancelled,
}

/// Active quick-select session state.
#[derive(Debug, Clone)]
pub struct QuickSelectState {
    /// All matches found in the buffer, in display order.
    pub matches: Vec<QsMatch>,
    /// Labels parallel to `matches`.
    pub labels: Vec<String>,
    /// Characters the user has typed so far (the current prefix).
    prefix: String,
}

impl QuickSelectState {
    /// Create a new session from a set of buffer rows and compiled patterns.
    /// Uses `alphabet` to assign labels.
    #[must_use]
    pub fn new(rows: &[&str], patterns: &[regex::Regex], alphabet: &str) -> Self {
        let matches = scan(rows, patterns);
        let n = matches.len();
        let labels = assign_labels(n, alphabet);
        Self {
            matches,
            labels,
            prefix: String::new(),
        }
    }

    /// Process one typed character and return the result.
    ///
    /// * If the character is `\x1b` (Escape) → `Cancelled`.
    /// * If the typed prefix narrows to exactly one match → `Hit`.
    /// * If no labels start with the new prefix → `Miss`.
    /// * Otherwise → `Pending`.
    pub fn type_char(&mut self, c: char) -> QsResult {
        if c == '\x1b' {
            return QsResult::Cancelled;
        }
        self.prefix.push(c);
        let candidates: Vec<usize> = self
            .labels
            .iter()
            .enumerate()
            .filter(|(_, label)| label.starts_with(self.prefix.as_str()))
            .map(|(i, _)| i)
            .collect();
        match candidates.as_slice() {
            [] => QsResult::Miss,
            [idx] => {
                // Exact match only when the prefix fully equals the label
                // (not just a prefix of a longer label).
                if self.labels[*idx] == self.prefix {
                    QsResult::Hit(self.matches[*idx].clone())
                } else {
                    QsResult::Pending
                }
            }
            _ => QsResult::Pending,
        }
    }

    /// Current typed prefix.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Iterator over `(match, label, remaining_suffix)` tuples for the
    /// currently-visible matches (those whose label starts with the typed
    /// prefix). `remaining_suffix` is the part of the label still to type
    /// (i.e. `label[prefix.len()..]`).
    pub fn matches_with_labels(&self) -> impl Iterator<Item = (&QsMatch, &str, &str)> {
        self.matches
            .iter()
            .zip(self.labels.iter())
            .filter_map(|(m, label)| {
                if label.starts_with(self.prefix.as_str()) {
                    let suffix = &label[self.prefix.len()..];
                    Some((m, label.as_str(), suffix))
                } else {
                    None
                }
            })
    }

    /// True when no matches exist (empty buffer, or all patterns failed to match).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
}

// ── PaneSelectState ──────────────────────────────────────────────────────────

/// One pane entry in a pane-select session.
#[derive(Debug, Clone)]
pub struct PaneEntry {
    /// Stable pane id from the main tab tree.
    pub pane_id: u32,
    /// Human-readable pane title (tab profile name / custom title).
    /// Populated but currently only consumed by future tooltip/preview code.
    #[allow(dead_code)]
    pub title: String,
    /// Badge label (single or two-char string from the alphabet).
    pub label: String,
}

/// Active pane-select session state.
#[derive(Debug, Clone)]
pub struct PaneSelectState {
    /// All pane entries for the active tab, in display order.
    pub entries: Vec<PaneEntry>,
    /// Characters the user has typed so far.
    prefix: String,
}

impl PaneSelectState {
    /// Create a new pane-select session.
    ///
    /// `panes` is a slice of `(pane_id, title)` pairs from the active tab.
    #[must_use]
    pub fn new(panes: &[(u32, String)], alphabet: &str) -> Self {
        let n = panes.len();
        let labels = assign_labels(n, alphabet);
        let entries = panes
            .iter()
            .zip(labels)
            .map(|((id, title), label)| PaneEntry {
                pane_id: *id,
                title: title.clone(),
                label,
            })
            .collect();
        Self {
            entries,
            prefix: String::new(),
        }
    }

    /// Process one typed character.
    ///
    /// Returns `Some(pane_id)` when a pane is uniquely identified,
    /// `None` when more input is needed (or Escape was pressed — the
    /// caller should check whether the result is the sentinel `u32::MAX`
    /// which signals cancellation).
    ///
    /// Convention:
    /// * `'\x1b'` → `Some(u32::MAX)` (cancelled)
    /// * unique pane resolved → `Some(pane_id)`
    /// * still pending → `None`
    pub fn type_char(&mut self, c: char) -> Option<u32> {
        if c == '\x1b' {
            return Some(u32::MAX); // sentinel for "cancelled"
        }
        self.prefix.push(c);
        let candidates: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.label.starts_with(self.prefix.as_str()))
            .map(|(i, _)| i)
            .collect();
        match candidates.as_slice() {
            [] => Some(u32::MAX), // nothing matches → cancel
            [idx] => {
                if self.entries[*idx].label == self.prefix {
                    Some(self.entries[*idx].pane_id)
                } else {
                    None // prefix of a longer label, keep going
                }
            }
            _ => None, // multiple candidates, keep going
        }
    }

    /// Current prefix.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Visible entries filtered by current prefix.
    pub fn visible_entries(&self) -> impl Iterator<Item = &PaneEntry> {
        self.entries
            .iter()
            .filter(|e| e.label.starts_with(self.prefix.as_str()))
    }
}

// ── Overlay badge descriptor ─────────────────────────────────────────────────

/// A single badge to draw in the quick-select overlay. The renderer receives
/// a `Vec<OverlayBadge>` each frame when quick-select mode is active and
/// draws small highlighted label chips at the specified cell positions.
#[derive(Debug, Clone)]
pub struct OverlayBadge {
    /// Grid column (0-based) where the badge should be anchored.
    pub col: u16,
    /// Grid row (0-based, in viewport coordinates) where the badge sits.
    pub row: u16,
    /// The full label text (e.g. `"a"`, `"sf"`).
    pub label: String,
    /// The already-typed prefix of this label — displayed dim / filled.
    pub typed_prefix: String,
    /// Whether this badge is for the currently-matched (highlighted) entry.
    pub highlighted: bool,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── assign_labels ────────────────────────────────────────────────────────

    #[test]
    fn assign_labels_single_chars_for_small_n() {
        let labels = assign_labels(3, "asdfjkl");
        assert_eq!(labels, vec!["a", "s", "d"]);
        // All unique.
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn assign_labels_two_char_for_large_n() {
        // alphabet of 4 chars, request 20 → need 2-char labels for overflow.
        let labels = assign_labels(20, "abcd");
        assert_eq!(labels.len(), 20, "must produce exactly 20 labels");
        // All unique.
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "all labels must be unique");
        // First 4 are 1-char.
        for label in labels.iter().take(4) {
            assert_eq!(label.chars().count(), 1, "first 4 must be 1-char: {label}");
        }
        // Remainder are 2-char.
        for label in labels.iter().skip(4) {
            assert_eq!(label.chars().count(), 2, "overflow must be 2-char: {label}");
        }
    }

    #[test]
    fn assign_labels_empty_n() {
        assert!(assign_labels(0, "abc").is_empty());
    }

    #[test]
    fn assign_labels_empty_alphabet() {
        assert!(assign_labels(5, "").is_empty());
    }

    // ── URL detection ────────────────────────────────────────────────────────

    #[test]
    fn scan_detects_url() {
        let patterns = default_compiled_patterns();
        let rows = &["Visit https://example.com today"];
        let matches = scan(rows, &patterns);
        assert!(
            matches.iter().any(|m| m.text.starts_with("https://")),
            "should find https URL: {matches:?}"
        );
    }

    #[test]
    fn scan_detects_ipv4() {
        let patterns = default_compiled_patterns();
        let rows = &["server at 192.168.1.1 port 22"];
        let matches = scan(rows, &patterns);
        assert!(
            matches.iter().any(|m| m.text == "192.168.1.1"),
            "should find IPv4: {matches:?}"
        );
    }

    #[test]
    fn scan_detects_git_hash() {
        let patterns = default_compiled_patterns();
        let rows = &["commit abc1234def5678 was merged"];
        let matches = scan(rows, &patterns);
        assert!(
            matches
                .iter()
                .any(|m| m.text == "abc1234def5678"),
            "should find git hash: {matches:?}"
        );
    }

    #[test]
    fn scan_detects_path() {
        let patterns = default_compiled_patterns();
        let rows = &["error in /usr/local/bin/foo.sh"];
        let matches = scan(rows, &patterns);
        assert!(
            matches.iter().any(|m| m.text.starts_with('/') || m.text.starts_with('~')),
            "should find absolute path: {matches:?}"
        );
    }

    #[test]
    fn scan_no_overlap() {
        // Two adjacent tokens — should each get a separate match and never overlap.
        let patterns = default_compiled_patterns();
        let rows = &["192.168.0.1 https://foo.com"];
        let matches = scan(rows, &patterns);
        // Verify no overlapping ranges.
        for i in 0..matches.len() {
            for j in (i + 1)..matches.len() {
                if matches[i].row == matches[j].row {
                    assert!(
                        matches[i].col_end <= matches[j].col_start
                            || matches[j].col_end <= matches[i].col_start,
                        "overlapping matches: {:?} and {:?}",
                        matches[i],
                        matches[j]
                    );
                }
            }
        }
    }

    // ── type_char prefix narrowing ───────────────────────────────────────────

    #[test]
    fn type_char_narrows_to_hit() {
        let patterns = default_compiled_patterns();
        let rows = &["https://a.com and https://b.com"];
        let mut state = QuickSelectState::new(rows, &patterns, "abcdef");

        // There should be at least 2 matches.
        assert!(state.matches.len() >= 2, "need ≥2 matches for this test");

        // The first label is "a", second is "b" (with the default alphabet).
        // Typing the first label char should eventually hit.
        let first_label = state.labels[0].clone();
        let mut result = QsResult::Pending;
        for c in first_label.chars() {
            result = state.type_char(c);
        }
        assert!(
            matches!(result, QsResult::Hit(_)),
            "typing a complete label must resolve to Hit: {result:?}"
        );
    }

    #[test]
    fn type_char_escape_cancels() {
        let patterns = default_compiled_patterns();
        let rows = &["https://example.com"];
        let mut state = QuickSelectState::new(rows, &patterns, "asdfjkl");
        let result = state.type_char('\x1b');
        assert_eq!(result, QsResult::Cancelled);
    }

    #[test]
    fn type_char_miss_on_bad_key() {
        let patterns = default_compiled_patterns();
        let rows = &["https://example.com"];
        let mut state = QuickSelectState::new(rows, &patterns, "asdfjkl");
        // First char that matches no label.
        let result = state.type_char('Z');
        assert_eq!(result, QsResult::Miss, "non-alphabet key must be Miss");
    }

    // ── validate_patterns / validate_alphabet ────────────────────────────────

    #[test]
    fn validate_alphabet_rejects_empty() {
        assert!(validate_alphabet("").is_some());
    }

    #[test]
    fn validate_alphabet_rejects_duplicates() {
        assert!(validate_alphabet("aab").is_some());
    }

    #[test]
    fn validate_alphabet_accepts_valid() {
        assert!(validate_alphabet(DEFAULT_ALPHABET).is_none());
    }

    #[test]
    fn validate_patterns_rejects_bad_regex() {
        let bad = vec!["[invalid".to_string()];
        assert!(validate_patterns(&bad).is_some());
    }

    #[test]
    fn validate_patterns_accepts_defaults() {
        let defaults: Vec<String> = DEFAULT_PATTERNS.iter().map(|&s| s.to_string()).collect();
        assert!(
            validate_patterns(&defaults).is_none(),
            "default patterns must all compile"
        );
    }

    // ── Config roundtrip (uses terminale_config types, tested separately) ────

    #[test]
    fn quick_select_default_keybinds() {
        let sc = terminale_config::ShortcutsConfig::default();
        // quick_select must have a non-empty default binding.
        assert!(
            !sc.quick_select.is_empty(),
            "quick_select shortcut must have a default binding"
        );
        // pane_select is intentionally unbound by default.
        // (No assertion — empty is fine.)
    }

    #[test]
    fn quick_select_config_defaults_validate() {
        terminale_config::Config::default()
            .validate()
            .expect("default config with quick_select must validate");
    }

    #[test]
    fn quick_select_config_roundtrip() {
        let cfg = terminale_config::QuickSelectConfig::default();
        // Use serde_json for a round-trip check (available in the terminale
        // crate's dev/test environment via serde_json workspace dependency).
        let json = serde_json::to_string(&cfg).expect("should serialise");
        let back: terminale_config::QuickSelectConfig =
            serde_json::from_str(&json).expect("should deserialise");
        assert_eq!(
            back.alphabet, cfg.alphabet,
            "alphabet must survive roundtrip"
        );
        assert_eq!(
            back.patterns.len(),
            cfg.patterns.len(),
            "patterns count must survive roundtrip"
        );
    }

    // ── visible_entries / matches_with_labels badge production ──────────────

    /// `visible_entries` narrows the pane list to entries whose label starts
    /// with the current prefix. After typing one character the count should
    /// drop (only entries whose label starts with that character remain).
    #[test]
    fn pane_select_visible_entries_filters_by_prefix() {
        let panes: Vec<(u32, String)> = vec![
            (1, "shell".into()),
            (2, "editor".into()),
            (3, "logs".into()),
        ];
        let mut ps = PaneSelectState::new(&panes, "asdfjkl");
        // Before typing: all 3 entries are visible.
        assert_eq!(ps.visible_entries().count(), 3, "all entries before any input");
        // Type the first label character ('a' for pane 1).
        ps.type_char('a');
        // After typing 'a', only entries whose label starts with 'a' remain.
        let visible: Vec<_> = ps.visible_entries().collect();
        assert!(
            visible.iter().all(|e| e.label.starts_with('a')),
            "all visible entries must have label starting with 'a': {visible:?}"
        );
    }

    /// `matches_with_labels` returns `(match, label, remaining_suffix)` triples
    /// for every match whose label still starts with the current typed prefix.
    /// Typing the first char of the first label should reduce the visible set
    /// and the remaining suffix should be one character shorter.
    #[test]
    fn quick_select_matches_with_labels_returns_correct_suffix() {
        let patterns = default_compiled_patterns();
        let rows = &[
            "see https://alpha.example.com and https://beta.example.com here",
        ];
        let mut qs = QuickSelectState::new(rows, &patterns, "abcdef");
        // Two URLs → two matches → two labels ("a", "b").
        assert!(qs.matches.len() >= 2, "need ≥2 URL matches for this test");

        // Before typing: all matches are visible, remaining == full label.
        let before: Vec<_> = qs.matches_with_labels().collect();
        assert_eq!(before.len(), qs.matches.len(), "all visible before typing");
        for (_, label, suffix) in &before {
            assert_eq!(*suffix, *label, "suffix must equal full label before typing");
        }

        // Type the first label character.
        let first_char = qs.labels[0].chars().next().unwrap();
        qs.type_char(first_char);

        let after: Vec<_> = qs.matches_with_labels().collect();
        // At least one entry whose remaining suffix is 1 shorter than the label.
        assert!(
            after.iter().any(|(_, label, suffix)| {
                label.starts_with(first_char) && suffix.len() + 1 == label.len()
            }),
            "remaining suffix must be label minus typed prefix: {after:?}"
        );
    }

    /// `QuickSelectState::is_empty` returns `true` when the buffer contains
    /// no matches for the given patterns.
    #[test]
    fn quick_select_is_empty_on_no_matches() {
        let patterns = default_compiled_patterns();
        let rows = &["hello world, no URLs here"];
        // Limit to URL-only patterns to guarantee no matches.
        let url_only: Vec<regex::Regex> =
            vec![regex::Regex::new(r"https?://\S+").unwrap()];
        let qs = QuickSelectState::new(rows, &url_only, "asdfjkl");
        assert!(qs.is_empty(), "no matches → is_empty must be true");

        // A scan with actual URL content must NOT be empty.
        let rows2 = &["visit https://example.com today"];
        let qs2 = QuickSelectState::new(rows2, &patterns, "asdfjkl");
        assert!(!qs2.is_empty(), "URL present → is_empty must be false");
    }

    /// Overlay-badge roundtrip: constructing `OverlayBadge` and reading its
    /// `label` field correctly derives the `remaining` suffix.
    #[test]
    fn overlay_badge_remaining_matches_label_suffix() {
        let badge = OverlayBadge {
            col: 5,
            row: 2,
            label: "sf".to_string(),
            typed_prefix: "s".to_string(),
            highlighted: false,
        };
        // `remaining` should be the part of `label` after `typed_prefix`.
        let remaining = &badge.label[badge.typed_prefix.len()..];
        assert_eq!(remaining, "f", "remaining must be the untyped tail of label");
    }
}
