//! Pure, testable status-bar segment rendering.
//!
//! This module knows nothing about GPU or egui — it just converts a
//! [`StatusContext`] + a list of [`terminale_config::StatusSegment`]s into
//! a pair of `String`s (left side and right side) that the renderer paints
//! into the status-bar strip.

use std::collections::HashMap;
use std::path::Path;
use terminale_config::StatusSegment;

/// All the runtime data a segment can draw from.
pub struct StatusContext<'a> {
    /// Latest OSC-7 working directory, or `None` if not announced yet.
    pub cwd: Option<&'a Path>,
    /// Active profile name (from `state.config.profiles.default` / resolved profile).
    pub profile_name: &'a str,
    /// 1-based index of the active tab.
    pub tab_index: usize,
    /// Total number of tabs.
    pub tab_count: usize,
    /// OSC 1337 `SetUserVar` map for the active pane's emulator.
    pub user_vars: &'a HashMap<String, String>,
    /// Wall-clock time to use for `Clock` segments (captured once per refresh).
    pub now: chrono::DateTime<chrono::Local>,
}

/// Shorten a filesystem path for display in the status bar.
///
/// Strategy:
/// - If the path starts with the user's home directory, replace that prefix
///   with `~`.
/// - Then keep only the last two components (or fewer if the path is short).
///
/// Examples:
/// - `/home/user/projects/foo/bar` → `~/projects/foo/bar` (if home =
///   `/home/user`) → `foo/bar`
/// - `/etc/nginx` → `/etc/nginx`
/// - `/etc` → `/etc`
#[must_use]
pub fn shorten_cwd(path: &Path) -> String {
    // Replace home prefix with `~`.
    let as_str = path.to_string_lossy();
    let home = dirs_home();
    let normalised = if let Some(h) = &home {
        let h_str = h.to_string_lossy();
        if as_str.starts_with(h_str.as_ref()) {
            let rest = &as_str[h_str.len()..];
            // `rest` is either empty (we ARE at home) or starts with a separator.
            if rest.is_empty() {
                "~".to_string()
            } else {
                format!("~{rest}")
            }
        } else {
            as_str.into_owned()
        }
    } else {
        as_str.into_owned()
    };

    // Keep only the last two path components.
    // Split on both `/` and `\` so Windows paths work without the `Path` API.
    let parts: Vec<&str> = normalised
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();

    match parts.as_slice() {
        [] => normalised,
        // Single component: if the normalised path starts with `~` or `/` keep the prefix.
        [single] => {
            if normalised.starts_with('~') {
                format!("~/{single}")
            } else if normalised.starts_with('/') {
                format!("/{single}")
            } else {
                (*single).to_string()
            }
        }
        [.., a, b] => {
            // If the original normalised had a `~` prefix and the reconstruction
            // would lose it, restore it.
            if normalised.starts_with('~') && *a != "~" {
                format!("{a}/{b}")
            } else if normalised.starts_with('~') {
                format!("~/{b}")
            } else {
                format!("{a}/{b}")
            }
        }
    }
}

/// Render one segment to a `String`.
#[must_use]
pub fn render_segment(seg: &StatusSegment, ctx: &StatusContext<'_>) -> String {
    match seg {
        StatusSegment::Cwd => {
            if let Some(path) = ctx.cwd {
                shorten_cwd(path)
            } else {
                String::new()
            }
        }
        StatusSegment::Clock { format } => ctx.now.format(format).to_string(),
        StatusSegment::Profile => ctx.profile_name.to_string(),
        StatusSegment::TabIndex => format!("{}/{}", ctx.tab_index, ctx.tab_count),
        StatusSegment::UserVar { name } => ctx
            .user_vars
            .get(name.as_str())
            .cloned()
            .unwrap_or_default(),
        StatusSegment::Literal { text } => text.clone(),
        StatusSegment::Spacer => String::new(),
    }
}

/// Compose a list of segments into a single `String` by joining the rendered
/// values with a single space.  Empty segments (e.g. missing user-var) are
/// skipped so no double-spaces appear.
#[must_use]
pub fn compose(segments: &[StatusSegment], ctx: &StatusContext<'_>) -> String {
    let parts: Vec<String> = segments
        .iter()
        .map(|s| render_segment(s, ctx))
        .filter(|s| !s.is_empty())
        .collect();
    parts.join("  ")
}

// ── platform home-dir helper ────────────────────────────────────────────────

fn dirs_home() -> Option<std::path::PathBuf> {
    // Use the `HOME` / `USERPROFILE` env vars directly — avoids pulling in a
    // new crate just for home-dir detection in this pure module.
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(std::path::PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn make_ctx<'a>(
        cwd: Option<&'a Path>,
        user_vars: &'a HashMap<String, String>,
    ) -> StatusContext<'a> {
        StatusContext {
            cwd,
            profile_name: "PowerShell",
            tab_index: 2,
            tab_count: 5,
            user_vars,
            now: chrono::Local
                .with_ymd_and_hms(2026, 5, 30, 14, 5, 9)
                .unwrap(),
        }
    }

    #[test]
    fn cwd_segment_none_is_empty() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        assert_eq!(
            render_segment(&StatusSegment::Cwd, &ctx),
            "",
            "missing cwd must render as empty"
        );
    }

    #[test]
    fn cwd_segment_short_path() {
        let vars = HashMap::new();
        let path = PathBuf::from("/etc");
        let ctx = make_ctx(Some(&path), &vars);
        let s = render_segment(&StatusSegment::Cwd, &ctx);
        // Must not be empty and should not be absurdly long.
        assert!(!s.is_empty());
    }

    #[test]
    fn clock_formats_correctly() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        let seg = StatusSegment::Clock {
            format: "%H:%M".into(),
        };
        assert_eq!(
            render_segment(&seg, &ctx),
            "14:05",
            "clock must format with zero-padding"
        );
    }

    #[test]
    fn clock_custom_format() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        let seg = StatusSegment::Clock {
            format: "%Y-%m-%d".into(),
        };
        assert_eq!(render_segment(&seg, &ctx), "2026-05-30");
    }

    #[test]
    fn profile_segment() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        assert_eq!(render_segment(&StatusSegment::Profile, &ctx), "PowerShell");
    }

    #[test]
    fn tab_index_segment() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        assert_eq!(
            render_segment(&StatusSegment::TabIndex, &ctx),
            "2/5",
            "tab index must be `index/total`"
        );
    }

    #[test]
    fn user_var_present() {
        let mut vars = HashMap::new();
        vars.insert("git_branch".into(), "main".into());
        let ctx = make_ctx(None, &vars);
        let seg = StatusSegment::UserVar {
            name: "git_branch".into(),
        };
        assert_eq!(render_segment(&seg, &ctx), "main");
    }

    #[test]
    fn user_var_missing_is_empty() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        let seg = StatusSegment::UserVar {
            name: "nonexistent".into(),
        };
        assert_eq!(
            render_segment(&seg, &ctx),
            "",
            "missing user-var must render as empty"
        );
    }

    #[test]
    fn literal_segment() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        let seg = StatusSegment::Literal { text: " | ".into() };
        assert_eq!(render_segment(&seg, &ctx), " | ");
    }

    #[test]
    fn spacer_is_empty() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        assert_eq!(render_segment(&StatusSegment::Spacer, &ctx), "");
    }

    #[test]
    fn compose_skips_empty_segments() {
        let mut vars = HashMap::new();
        vars.insert("present".into(), "value".into());
        let ctx = make_ctx(None, &vars);
        let segments = vec![
            // Cwd is None so this renders empty.
            StatusSegment::Cwd,
            // UserVar with missing name renders empty.
            StatusSegment::UserVar {
                name: "missing".into(),
            },
            StatusSegment::Literal {
                text: "hello".into(),
            },
        ];
        let s = compose(&segments, &ctx);
        // Only non-empty segment is the literal.
        assert_eq!(s, "hello", "empty segments must be skipped");
    }

    #[test]
    fn compose_joins_with_two_spaces() {
        let vars = HashMap::new();
        let ctx = make_ctx(None, &vars);
        let segments = vec![
            StatusSegment::Literal { text: "A".into() },
            StatusSegment::Literal { text: "B".into() },
        ];
        assert_eq!(compose(&segments, &ctx), "A  B");
    }

    #[test]
    fn shorten_cwd_deep_unix_path() {
        let path = PathBuf::from("/a/b/c/d");
        let s = shorten_cwd(&path);
        // Should show at most the last two components.
        assert!(s.contains("c/d") || s.contains("c\\d"), "got: {s}");
    }

    #[test]
    fn shorten_cwd_root() {
        let path = PathBuf::from("/");
        let s = shorten_cwd(&path);
        // Root should not panic and should not be empty.
        assert!(!s.is_empty(), "root must return something");
    }

    #[test]
    fn shorten_cwd_two_components() {
        let path = PathBuf::from("/etc/nginx");
        let s = shorten_cwd(&path);
        assert!(
            s.contains("etc") && s.contains("nginx"),
            "two-component path: got {s}"
        );
    }
}
