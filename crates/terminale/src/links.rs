//! Lightweight URL scanner used to find clickable links in the visible
//! terminal buffer when shells / CLIs don't emit OSC 8 markers (most of
//! them still don't).
//!
//! The built-in scanner is intentionally regex-free — the grammar we recognise
//! is small and the linear-time character scan is faster than firing up a
//! regex engine on every PTY chunk.
//!
//! User-configured [`terminale_config::HyperlinkRule`]s are compiled via the
//! `regex` crate and cached in `COMPILED_RULES`. Call
//! [`update_hyperlink_rules`] whenever the config changes.

use parking_lot::RwLock;
use regex::Regex;
use std::sync::OnceLock;

// ── Runtime hyperlink-rule cache ─────────────────────────────────────────────

/// Compiled form of one user hyperlink rule.
#[derive(Clone)]
struct CompiledRule {
    re: Regex,
}

/// Global cache of compiled user rules, updated via [`update_hyperlink_rules`].
fn compiled_rules() -> &'static RwLock<Vec<CompiledRule>> {
    static INSTANCE: OnceLock<RwLock<Vec<CompiledRule>>> = OnceLock::new();
    INSTANCE.get_or_init(|| RwLock::new(Vec::new()))
}

/// Replace the cached rule set with a freshly-compiled version of `rules`.
/// Rules whose regex fails to compile are skipped with a `warn!` log entry.
/// Call this whenever `config.terminal.hyperlink_rules` changes.
pub(crate) fn update_hyperlink_rules(rules: &[terminale_config::HyperlinkRule]) {
    let compiled: Vec<CompiledRule> = rules
        .iter()
        .filter_map(|r| {
            let pattern = r.regex.trim().to_string();
            if pattern.is_empty() {
                return None;
            }
            match Regex::new(&pattern) {
                Ok(re) => Some(CompiledRule { re }),
                Err(e) => {
                    tracing::warn!(
                        pattern = %r.regex,
                        error = %e,
                        "hyperlink_rule regex failed to compile — skipping"
                    );
                    None
                }
            }
        })
        .collect();
    *compiled_rules().write() = compiled;
}

// ── Built-in URL scanner ─────────────────────────────────────────────────────

/// URI schemes we treat as clickable. Anything beyond these stays plain
/// text — we don't want to turn arbitrary `foo://bar` into a hyperlink.
const SCHEMES: &[&str] = &["https://", "http://", "ftp://", "file://", "mailto:"];

/// One detected URL range — byte offsets into the source `text`.
#[derive(Debug, Clone)]
pub struct Match {
    /// Starting byte offset of the URL inside the scanned string.
    pub start: usize,
    /// One-past-the-last byte offset.
    pub end: usize,
    /// The matched URL, with trailing punctuation already trimmed.
    pub url: String,
}

/// Scan `text` left-to-right and return every URL match. The matched
/// ranges never overlap.
#[must_use]
pub fn scan(text: &str) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Try every scheme at this offset.
        let mut hit = None;
        for &s in SCHEMES {
            let sb = s.as_bytes();
            if i + sb.len() <= bytes.len() && &bytes[i..i + sb.len()] == sb {
                hit = Some(s.len());
                break;
            }
        }
        let Some(scheme_len) = hit else {
            i += 1;
            continue;
        };
        // Walk forward while the byte is part of a URL.
        let mut j = i + scheme_len;
        while j < bytes.len() && is_url_char(bytes[j]) {
            j += 1;
        }
        // Trim trailing punctuation that shells / docs commonly slap on a
        // URL but isn't actually part of it: `.,;:!?)>]}` and the closing
        // quote types.
        let mut k = j;
        while k > i + scheme_len {
            let last = bytes[k - 1];
            if matches!(
                last,
                b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b'>' | b']' | b'}' | b'\'' | b'"'
            ) {
                k -= 1;
            } else {
                break;
            }
        }
        // Require at least one char of "path" beyond the scheme.
        if k > i + scheme_len {
            // SAFETY: the source string was UTF-8 and our scheme detector
            // only matched ASCII bytes; the URL char predicate likewise
            // only accepts ASCII. So `i..k` is a valid UTF-8 slice.
            if let Ok(url) = std::str::from_utf8(&bytes[i..k]) {
                out.push(Match {
                    start: i,
                    end: k,
                    url: url.to_string(),
                });
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Scan `text` using the user-configured hyperlink rules stored in
/// [`compiled_rules()`]. Returns a deduplicated list of matches that **do not
/// overlap with** any match already in `existing_ranges` (byte pairs). Matches
/// from the compiled rules are appended after the built-in scan so the caller
/// can merge results without double-underlines.
///
/// When the compiled rule list is **empty** this function returns an empty
/// `Vec` — the caller is expected to fall back to [`scan`] for built-in
/// detection. This way adding rules augments (rather than replaces) built-in
/// detection.
#[must_use]
pub(crate) fn scan_with_rules(text: &str, existing_ranges: &[(usize, usize)]) -> Vec<Match> {
    let guard = compiled_rules().read();
    if guard.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Match> = Vec::new();
    for cr in guard.iter() {
        for m in cr.re.find_iter(text) {
            let (start, end) = (m.start(), m.end());
            if end <= start {
                continue;
            }
            // Skip if overlapping an already-claimed range.
            let overlaps_range = |(rs, re): (usize, usize)| start < re && rs < end;
            if existing_ranges.iter().copied().any(overlaps_range)
                || out.iter().any(|o: &Match| overlaps_range((o.start, o.end)))
            {
                continue;
            }
            out.push(Match {
                start,
                end,
                url: m.as_str().to_string(),
            });
        }
    }
    out
}

/// One detected filesystem path in the scanned text.
#[derive(Debug, Clone)]
pub struct PathMatch {
    /// Starting byte offset of the displayed token (after trimming
    /// surrounding quotes/brackets).
    pub start: usize,
    /// One-past-the-last byte offset of the displayed token. Includes a
    /// trailing `:line[:col]` suffix when present, so the whole
    /// compiler-style reference is clickable.
    pub end: usize,
    /// Resolved, existence-checked absolute path to open (the `:line:col`
    /// suffix is stripped here).
    pub path: std::path::PathBuf,
    /// Line number parsed from a `:line` / `:line:col` suffix, if any.
    pub line: Option<u32>,
    /// Column number parsed from a `:line:col` suffix, if any.
    pub column: Option<u32>,
}

/// Scan `text` for filesystem paths that actually exist on disk. Absolute
/// paths (and `~`-relative ones) are resolved without help; relative paths
/// are resolved against `cwd` when the shell has announced one (OSC 7).
///
/// A trailing `:line[:col]` (the convention compilers and grep use) is
/// recognised: it stays inside the clickable display range but is stripped
/// from the opened path. The on-disk existence check is what keeps this
/// precise — prose tokens that merely *look* path-ish never match.
#[must_use]
pub fn scan_paths(text: &str, cwd: Option<&std::path::Path>) -> Vec<PathMatch> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let tok_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if let Some(m) = resolve_path_token(text, tok_start, i, cwd) {
            out.push(m);
        }
    }
    out
}

/// Trim surrounding noise from `text[start..end]`, split off any
/// `:line[:col]` suffix, pre-filter, then resolve + existence-check.
fn resolve_path_token(
    text: &str,
    start: usize,
    end: usize,
    cwd: Option<&std::path::Path>,
) -> Option<PathMatch> {
    let bytes = text.as_bytes();
    // Trim leading openers.
    let mut ds = start;
    while ds < end && matches!(bytes[ds], b'(' | b'<' | b'[' | b'{' | b'\'' | b'"' | b'`') {
        ds += 1;
    }
    // Trim trailing closers / sentence punctuation. Colons and digits stay
    // so a `:line:col` suffix survives for the next step.
    let mut de = end;
    while de > ds
        && matches!(
            bytes[de - 1],
            b')' | b'>' | b']' | b'}' | b'\'' | b'"' | b'`' | b',' | b';' | b'!' | b'?'
        )
    {
        de -= 1;
    }
    if de <= ds {
        return None;
    }
    let display = &text[ds..de];

    // Strip a trailing `:line[:col]` (digits only) up to twice, capturing
    // the numbers. A drive colon like `C:\…` is safe: it's followed by a
    // separator, not digits. `nums` ends up innermost-first: `[col, line]`
    // for `file:line:col`, or `[line]` for `file:line`.
    let mut bare = display;
    let mut nums: Vec<u32> = Vec::new();
    for _ in 0..2 {
        if let Some(idx) = bare.rfind(':') {
            let (head, tail) = (&bare[..idx], &bare[idx + 1..]);
            if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) && !head.is_empty() {
                nums.push(tail.parse::<u32>().unwrap_or(0));
                bare = head;
                continue;
            }
        }
        break;
    }
    let (line, column) = match nums.as_slice() {
        [line] => (Some(*line), None),
        [col, line] => (Some(*line), Some(*col)),
        _ => (None, None),
    };
    // Also trim a trailing '.' the previous pass left (e.g. `foo.rs:1.`).
    let bare = bare.trim_end_matches('.');
    if bare.len() < 2 {
        return None;
    }

    // Cheap pre-filter: only stat tokens that look like paths, so a wall of
    // prose doesn't hammer the filesystem.
    if !looks_like_path(bare) {
        return None;
    }

    let resolved = resolve_existing(bare, cwd)?;
    Some(PathMatch {
        start: ds,
        end: de,
        path: resolved,
        line,
        column,
    })
}

/// Build an editor invocation from a `command` template, substituting
/// `{file}`, `{line}`, and `{column}` tokens. Returns `(program, args)`,
/// or `None` when the template is empty/blank. Missing line/column default
/// to `1` so editors that always expect them still work. Whitespace splits
/// the template into argv (simple, shell-free).
#[must_use]
pub fn build_editor_invocation(
    template: &str,
    file: &std::path::Path,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<(String, Vec<String>)> {
    let template = template.trim();
    if template.is_empty() {
        return None;
    }
    let file_s = file.to_string_lossy();
    let line_s = line.unwrap_or(1).to_string();
    let col_s = column.unwrap_or(1).to_string();
    let mut parts = template.split_whitespace().map(|tok| {
        tok.replace("{file}", &file_s)
            .replace("{line}", &line_s)
            .replace("{column}", &col_s)
    });
    let program = parts.next()?;
    let args: Vec<String> = parts.collect();
    Some((program, args))
}

/// Heuristic gate before the (more expensive) existence check.
fn looks_like_path(s: &str) -> bool {
    let has_sep = s.contains('/') || s.contains('\\');
    let is_home = s.starts_with('~');
    let is_drive = {
        let b = s.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/')
    };
    // A bare filename with an extension (`Cargo.toml`) is allowed too — the
    // existence check downstream keeps it honest.
    let has_ext = std::path::Path::new(s)
        .extension()
        .is_some_and(|e| (1..=8).contains(&e.len()));
    has_sep || is_home || is_drive || has_ext
}

/// Resolve `bare` to an absolute path that exists, or `None`.
fn resolve_existing(bare: &str, cwd: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let candidate: PathBuf =
        if let Some(rest) = bare.strip_prefix("~/").or_else(|| bare.strip_prefix("~\\")) {
            home_dir()?.join(rest)
        } else if bare == "~" {
            home_dir()?
        } else {
            let p = Path::new(bare);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd?.join(p)
            }
        };
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var).map(std::path::PathBuf::from)
}

#[inline]
fn is_url_char(b: u8) -> bool {
    matches!(
        b,
        b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'-' | b'.' | b'_' | b'~'
            | b'/' | b'?' | b'#' | b'[' | b']' | b'@'
            | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
            | b'*' | b'+' | b',' | b';' | b'='
            | b'%' | b':'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_basic_https() {
        let m = scan("Visit https://example.com today");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].url, "https://example.com");
    }

    #[test]
    fn trims_trailing_punctuation() {
        let m = scan("See https://example.com.");
        assert_eq!(m[0].url, "https://example.com");
    }

    #[test]
    fn ignores_unknown_scheme() {
        assert!(scan("ssh://foo@bar").is_empty());
    }

    #[test]
    fn finds_multiple() {
        let m = scan("https://a.com and http://b.io/path?q=1");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].url, "https://a.com");
        assert_eq!(m[1].url, "http://b.io/path?q=1");
    }

    /// Dev-server URLs have no dot in the host — they must still match.
    #[test]
    fn finds_localhost_with_and_without_port() {
        let m = scan("dev server at http://localhost ready");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].url, "http://localhost");

        let m = scan("listening on http://localhost:5173/app?hmr=1");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].url, "http://localhost:5173/app?hmr=1");
    }

    #[test]
    fn matches_mailto() {
        let m = scan("write to mailto:foo@bar.io");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].url, "mailto:foo@bar.io");
    }

    #[test]
    fn scan_paths_finds_existing_relative_and_absolute() {
        let dir = std::env::temp_dir().join(format!("terminale_links_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let file = dir.join("src").join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        // Relative path resolved against cwd, with a :line:col suffix.
        let line = "error at src/main.rs:42:10 here";
        let hits = scan_paths(line, Some(&dir));
        assert_eq!(hits.len(), 1, "expected exactly one path hit");
        assert_eq!(hits[0].path, file);
        // The :line:col suffix is parsed (line=42, col=10)…
        assert_eq!(hits[0].line, Some(42));
        assert_eq!(hits[0].column, Some(10));
        // …and the display range still covers the whole reference.
        assert_eq!(&line[hits[0].start..hits[0].end], "src/main.rs:42:10");

        // Absolute path needs no cwd; surrounding parens are trimmed.
        let abs = file.display().to_string();
        let line2 = format!("see ({abs})");
        let hits2 = scan_paths(&line2, None);
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].path, file);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editor_invocation_substitutes_tokens() {
        use std::path::Path;
        let file = Path::new("/proj/src/main.rs");
        // VS Code style.
        let (prog, args) =
            build_editor_invocation("code -g {file}:{line}:{column}", file, Some(42), Some(7))
                .unwrap();
        assert_eq!(prog, "code");
        assert_eq!(args, vec!["-g", "/proj/src/main.rs:42:7"]);
        // Vim style, column absent → defaults to 1.
        let (prog, args) =
            build_editor_invocation("vim +{line} {file}", file, Some(9), None).unwrap();
        assert_eq!(prog, "vim");
        assert_eq!(args, vec!["+9", "/proj/src/main.rs"]);
        // Empty template disables editor integration.
        assert!(build_editor_invocation("   ", file, Some(1), None).is_none());
    }

    #[test]
    fn scan_paths_rejects_nonexistent_and_prose() {
        let cwd = std::env::temp_dir();
        // Looks path-ish but doesn't exist → no match.
        assert!(scan_paths("open /definitely/not/here/xyzzy.rs please", Some(&cwd)).is_empty());
        // Plain prose with no separators / extensions → never stat'd.
        assert!(scan_paths("the quick brown fox jumps", Some(&cwd)).is_empty());
    }

    // ── scan_with_rules tests ──────────────────────────────────────────────────

    // Tests in this group mutate the process-global `compiled_rules()` cache.
    // Rust runs tests in parallel by default, so we serialise them with a
    // dedicated mutex to prevent races between set/clear calls.
    fn rules_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    /// Helper: install a fresh set of rules for testing.
    fn set_test_rules(rules: &[terminale_config::HyperlinkRule]) {
        update_hyperlink_rules(rules);
    }

    /// Helper: clear the global rule cache after each test.
    fn clear_rules() {
        update_hyperlink_rules(&[]);
    }

    #[test]
    fn scan_with_rules_empty_rule_list_returns_empty() {
        let _g = rules_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_rules();
        // With no compiled rules, should always return empty (fall back to
        // built-in scan).
        let m = scan_with_rules("Visit https://example.com today", &[]);
        assert!(m.is_empty(), "no rules → no rule matches");
    }

    #[test]
    fn scan_with_rules_matches_custom_pattern() {
        let _g = rules_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_test_rules(&[terminale_config::HyperlinkRule::new(
            r"\b[0-9a-f]{7,40}\b",
            "Git SHA",
        )]);
        let m = scan_with_rules("commit a1b2c3d4 is broken", &[]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].url, "a1b2c3d4");
        clear_rules();
    }

    #[test]
    fn scan_with_rules_does_not_overlap_existing_ranges() {
        let _g = rules_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_test_rules(&[terminale_config::HyperlinkRule::new(
            r"https?://\S+",
            "HTTP URL",
        )]);
        // The URL https://example.com is already claimed by `existing_ranges`.
        let existing = vec![(6usize, 27usize)]; // "https://example.com"
        let m = scan_with_rules("Visit https://example.com today", &existing);
        // Should produce no matches since the only match overlaps existing.
        assert!(m.is_empty(), "overlapping range must be skipped");
        clear_rules();
    }

    #[test]
    fn scan_with_rules_invalid_regex_is_skipped() {
        let _g = rules_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // An invalid regex must not panic — it gets skipped with a warning.
        set_test_rules(&[terminale_config::HyperlinkRule::new(
            r"[invalid(regex",
            "bad",
        )]);
        // compiled_rules() should be empty (compilation failed).
        let m = scan_with_rules("anything", &[]);
        assert!(m.is_empty());
        clear_rules();
    }

    #[test]
    fn update_hyperlink_rules_replaces_previous() {
        let _g = rules_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_test_rules(&[terminale_config::HyperlinkRule::new(r"\bfoo\b", "foo")]);
        let m1 = scan_with_rules("foo bar", &[]);
        assert_eq!(m1.len(), 1);

        // Replace with a different rule.
        set_test_rules(&[terminale_config::HyperlinkRule::new(r"\bbar\b", "bar")]);
        let m2 = scan_with_rules("foo bar", &[]);
        // "foo" no longer matches; "bar" does.
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0].url, "bar");
        clear_rules();
    }
}
