//! Session-restore and named-workspace support.
//!
//! # Data model
//!
//! A [`SavedWorkspace`] is a flat description of an entire multi-window
//! session (currently limited to one window's worth of tabs — multi-window
//! restore is noted as future work). Each [`SavedTab`] carries a recursive
//! [`SavedPaneTree`] that mirrors the live [`PaneNode`] structure, with
//! leaf nodes extended to hold the profile name, last working directory, and
//! display title.
//!
//! # On-disk format
//!
//! Named workspaces are written as individual TOML files under the OS-standard
//! config directory:
//!   `<config_dir>/workspaces/<name>.toml`
//!
//! The last session is auto-saved to:
//!   `<data_dir>/last_session.toml`
//!
//! Both files use the same [`SavedWorkspace`] schema.
//!
//! # Restore semantics
//!
//! Only the *layout* (tabs, split ratios) and, optionally, each pane's last
//! working directory (via OSC 7) are restored. Running processes are not
//! restored — each pane spawns a fresh shell.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Data types ────────────────────────────────────────────────────────────────

/// Direction of a binary split — serialised so saved workspaces are portable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SavedSplitDir {
    /// The divider runs horizontally; children stack top / bottom.
    Horizontal,
    /// The divider runs vertically; children sit left / right.
    Vertical,
}

/// A node in the saved pane tree. Mirrors the live `PaneNode` enum but
/// carries per-leaf metadata instead of runtime ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SavedPaneTree {
    /// A leaf — one shell pane.
    Leaf {
        /// Name of the profile to spawn (must match a `[profiles.profiles]`
        /// entry; falls back to the overall default when absent).
        profile: Option<String>,
        /// Last working directory, as reported by OSC 7. `None` when the
        /// shell never announced one.
        cwd: Option<String>,
        /// User-set display title (from the inline rename). `None` = automatic.
        title: Option<String>,
    },
    /// An internal split node.
    Split {
        direction: SavedSplitDir,
        /// Fraction `(0.0..1.0)` allocated to the `a` child.
        ratio: f32,
        a: Box<SavedPaneTree>,
        b: Box<SavedPaneTree>,
    },
}

/// One saved tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedTab {
    /// User-set tab title, if any.
    pub(crate) title: Option<String>,
    /// The pane layout tree for this tab.
    pub(crate) tree: SavedPaneTree,
    /// Tab-group id this tab belongs to, if any.
    #[serde(default)]
    pub(crate) group: Option<u32>,
}

/// One saved tab group definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedTabGroup {
    /// Stable id matching the per-tab `group` field.
    pub(crate) id: u32,
    /// Display name.
    pub(crate) name: String,
    /// Accent colour `[R, G, B]`.
    pub(crate) color: [u8; 3],
}

/// Root of a saved workspace — a list of saved tabs.
///
/// Multiple windows are not yet serialised; this struct is kept flat so
/// it's trivial to extend later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedWorkspace {
    /// Human-readable name (populated when saving; empty for `last_session`).
    #[serde(default)]
    pub(crate) name: String,
    /// The tabs in order. Index 0 corresponds to the leftmost tab.
    pub(crate) tabs: Vec<SavedTab>,
    /// Which tab was active at save time.
    #[serde(default)]
    pub(crate) active_tab: usize,
    /// Tab group definitions. Restored alongside per-tab `group` fields.
    #[serde(default)]
    pub(crate) tab_groups: Vec<SavedTabGroup>,
    /// The next group id to use after restore (so newly-created groups never
    /// collide with restored ones).
    #[serde(default)]
    pub(crate) next_group_id: u32,
}

// ── Capture: live state → SavedWorkspace ─────────────────────────────────────

/// Walk a live `PaneNode` tree (from `TabState`) and build its serialisable
/// counterpart. Leaf metadata is fetched via the closure `leaf_meta` which
/// takes a `PaneId` and returns `(profile, cwd, title)`.
pub(crate) fn capture_pane_tree<F>(
    node: &crate::PaneNode,
    leaf_meta: &F,
) -> SavedPaneTree
where
    F: Fn(crate::PaneId) -> (Option<String>, Option<String>, Option<String>),
{
    match node {
        crate::PaneNode::Leaf(id) => {
            let (profile, cwd, title) = leaf_meta(*id);
            SavedPaneTree::Leaf { profile, cwd, title }
        }
        crate::PaneNode::Split { direction, ratio, a, b } => {
            let saved_dir = match direction {
                crate::SplitDir::Horizontal => SavedSplitDir::Horizontal,
                crate::SplitDir::Vertical => SavedSplitDir::Vertical,
            };
            SavedPaneTree::Split {
                direction: saved_dir,
                ratio: *ratio,
                a: Box::new(capture_pane_tree(a, leaf_meta)),
                b: Box::new(capture_pane_tree(b, leaf_meta)),
            }
        }
    }
}

/// Capture the current state of a window's tabs + group registry into a
/// [`SavedWorkspace`]. Pass empty slices for `tab_groups` and `0` for
/// `next_group_id` when groups are not relevant.
#[allow(dead_code)]
pub(crate) fn capture_workspace(
    tabs: &[crate::TabState],
    active_tab: usize,
    name: &str,
    restore_working_dirs: bool,
) -> SavedWorkspace {
    capture_workspace_with_groups(tabs, active_tab, name, restore_working_dirs, &[], 0)
}

/// Extended variant that also serialises the tab-group registry.
pub(crate) fn capture_workspace_with_groups(
    tabs: &[crate::TabState],
    active_tab: usize,
    name: &str,
    restore_working_dirs: bool,
    tab_groups: &[crate::TabGroup],
    next_group_id: u32,
) -> SavedWorkspace {
    let saved_tabs: Vec<SavedTab> = tabs
        .iter()
        .map(|tab| {
            let meta = |id: crate::PaneId| -> (Option<String>, Option<String>, Option<String>) {
                let Some(pane) = tab.panes.get(&id) else {
                    return (None, None, None);
                };
                let profile = Some(pane.profile_name.clone());
                let cwd = if restore_working_dirs {
                    pane.emulator.lock().current_dir().map(std::string::ToString::to_string)
                } else {
                    None
                };
                let title = pane.user_title.clone();
                (profile, cwd, title)
            };
            SavedTab {
                title: tab.user_title.clone(),
                tree: capture_pane_tree(&tab.tree, &meta),
                group: tab.group,
            }
        })
        .collect();

    let saved_groups: Vec<SavedTabGroup> = tab_groups
        .iter()
        .map(|g| SavedTabGroup {
            id: g.id,
            name: g.name.clone(),
            color: g.color,
        })
        .collect();

    SavedWorkspace {
        name: name.to_string(),
        tabs: saved_tabs,
        active_tab: active_tab.min(tabs.len().saturating_sub(1)),
        tab_groups: saved_groups,
        next_group_id,
    }
}

// ── Restore plan ──────────────────────────────────────────────────────────────

/// One leaf node in the restore plan — the sequence of (profile, cwd, split
/// operations) the caller must execute to reconstruct a [`SavedPaneTree`].
#[derive(Debug, Clone)]
pub(crate) struct RestoreLeaf {
    pub(crate) profile: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) title: Option<String>,
}

/// One step in the restore plan produced by [`restore_plan_for_tree`].
#[derive(Debug, Clone)]
pub(crate) enum RestoreStep {
    /// Spawn the leaf described by the plan entry as the *initial* leaf
    /// (the tab already exists; this populates its first pane).
    InitLeaf(RestoreLeaf),
    /// Spawn a new leaf as a sibling of the currently-focused leaf.
    SplitLeaf {
        direction: SavedSplitDir,
        /// Which side the new leaf lives on (mirrors `side_b` in `split_in`).
        side_b: bool,
        ratio: f32,
        leaf: RestoreLeaf,
    },
}

/// Convert a [`SavedPaneTree`] into a flat sequence of [`RestoreStep`]s the
/// caller can execute one-by-one (pre-order traversal, `a` before `b`).
///
/// The first step is always [`RestoreStep::InitLeaf`] — it populates the
/// tab's initial lone leaf. Every subsequent step is a
/// [`RestoreStep::SplitLeaf`].
///
/// # Note
///
/// After calling `split_focused` for a `SplitLeaf` step the newly-spawned
/// pane is focused (side_b=true puts the new leaf as the focused `b`). We
/// therefore walk the tree so that the first leaf becomes the initial one
/// and every subsequent leaf is appended via `b`-side splits. The split
/// ratio is stored per-split so the geometry is restored faithfully.
pub(crate) fn restore_plan_for_tree(tree: &SavedPaneTree) -> Vec<RestoreStep> {
    let mut steps = Vec::new();
    collect_restore_steps(tree, &mut steps, true, crate::SplitDir::Vertical, 0.5);
    steps
}

fn collect_restore_steps(
    node: &SavedPaneTree,
    steps: &mut Vec<RestoreStep>,
    is_first: bool,
    _parent_dir: crate::SplitDir,
    _parent_ratio: f32,
) {
    match node {
        SavedPaneTree::Leaf { profile, cwd, title } => {
            let leaf = RestoreLeaf {
                profile: profile.clone(),
                cwd: cwd.clone(),
                title: title.clone(),
            };
            if is_first {
                steps.push(RestoreStep::InitLeaf(leaf));
            } else {
                // This arm is only reached when called directly on a
                // leaf that is the right-hand child of a split. The
                // split metadata is injected by the Split arm below.
                steps.push(RestoreStep::SplitLeaf {
                    direction: SavedSplitDir::Vertical,
                    side_b: true,
                    ratio: 0.5,
                    leaf,
                });
            }
        }
        SavedPaneTree::Split { direction, ratio, a, b } => {
            // Walk `a` subtree first, then `b`.
            collect_restore_steps_with_split(a, steps, is_first);
            // `b` is introduced by splitting from the result of `a`.
            inject_split_for_b(b, steps, *direction, *ratio);
        }
    }
}

/// Recursively collect steps for the `a` subtree (left/top side).
fn collect_restore_steps_with_split(
    node: &SavedPaneTree,
    steps: &mut Vec<RestoreStep>,
    is_first: bool,
) {
    match node {
        SavedPaneTree::Leaf { profile, cwd, title } => {
            let leaf = RestoreLeaf {
                profile: profile.clone(),
                cwd: cwd.clone(),
                title: title.clone(),
            };
            if is_first {
                steps.push(RestoreStep::InitLeaf(leaf));
            } else {
                steps.push(RestoreStep::SplitLeaf {
                    direction: SavedSplitDir::Vertical,
                    side_b: true,
                    ratio: 0.5,
                    leaf,
                });
            }
        }
        SavedPaneTree::Split { direction, ratio, a, b } => {
            collect_restore_steps_with_split(a, steps, is_first);
            inject_split_for_b(b, steps, *direction, *ratio);
        }
    }
}

/// Recursively inject split steps for the `b` (right/bottom) subtree,
/// using the parent split's direction and ratio.
fn inject_split_for_b(
    node: &SavedPaneTree,
    steps: &mut Vec<RestoreStep>,
    direction: SavedSplitDir,
    ratio: f32,
) {
    match node {
        SavedPaneTree::Leaf { profile, cwd, title } => {
            steps.push(RestoreStep::SplitLeaf {
                direction,
                side_b: true,
                ratio,
                leaf: RestoreLeaf {
                    profile: profile.clone(),
                    cwd: cwd.clone(),
                    title: title.clone(),
                },
            });
        }
        SavedPaneTree::Split { direction: child_dir, ratio: child_ratio, a, b } => {
            // The split itself introduces a new leaf on side_b=true with the
            // parent direction/ratio; the `a` subtree of this child split
            // is that leaf's position. We emit the split for `a` first, then `b`.
            inject_split_for_b(a, steps, direction, ratio);
            inject_split_for_b(b, steps, *child_dir, *child_ratio);
        }
    }
}

// ── Disk I/O ──────────────────────────────────────────────────────────────────

/// Write a [`SavedWorkspace`] to disk.
///
/// # Errors
///
/// Returns `Err` on TOML serialisation or filesystem error.
pub(crate) fn write_workspace(path: &Path, ws: &SavedWorkspace) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(ws)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// Read a [`SavedWorkspace`] from disk.
///
/// # Errors
///
/// Returns `Err` on filesystem or TOML parse error.
pub(crate) fn read_workspace(path: &Path) -> std::io::Result<SavedWorkspace> {
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// List all named workspaces in the user's workspaces directory.
///
/// Returns `Vec<(name, path)>` sorted alphabetically by name.
pub(crate) fn list_workspaces() -> Vec<(String, PathBuf)> {
    let Some(dir) = terminale_config::paths::workspaces_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? == "toml" {
                let name = p.file_stem()?.to_string_lossy().into_owned();
                Some((name, p))
            } else {
                None
            }
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Save the auto last-session snapshot to disk.
pub(crate) fn save_last_session(
    tabs: &[crate::TabState],
    active_tab: usize,
    restore_working_dirs: bool,
    tab_groups: &[crate::TabGroup],
    next_group_id: u32,
) {
    let Some(path) = terminale_config::paths::last_session_path() else {
        return;
    };
    let ws = capture_workspace_with_groups(
        tabs,
        active_tab,
        "",
        restore_working_dirs,
        tab_groups,
        next_group_id,
    );
    if let Err(e) = write_workspace(&path, &ws) {
        tracing::warn!(?e, "failed to save last session");
    }
}

/// Load the last-session snapshot from disk. Returns `None` if no file exists
/// or if parsing fails.
pub(crate) fn load_last_session() -> Option<SavedWorkspace> {
    let path = terminale_config::paths::last_session_path()?;
    match read_workspace(&path) {
        Ok(ws) => Some(ws),
        Err(e) => {
            tracing::warn!(?e, "failed to load last session");
            None
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(profile: &str, cwd: &str) -> SavedPaneTree {
        SavedPaneTree::Leaf {
            profile: Some(profile.into()),
            cwd: Some(cwd.into()),
            title: None,
        }
    }

    fn split(dir: SavedSplitDir, ratio: f32, a: SavedPaneTree, b: SavedPaneTree) -> SavedPaneTree {
        SavedPaneTree::Split {
            direction: dir,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    // ── Serialisation roundtrips ──────────────────────────────────────────────

    #[test]
    fn leaf_roundtrip() {
        let node = leaf("PowerShell", "C:/Users/test");
        let s = toml::to_string_pretty(&node).unwrap();
        let de: SavedPaneTree = toml::from_str(&s).unwrap();
        match de {
            SavedPaneTree::Leaf { profile, cwd, .. } => {
                assert_eq!(profile.as_deref(), Some("PowerShell"));
                assert_eq!(cwd.as_deref(), Some("C:/Users/test"));
            }
            _ => panic!("expected Leaf"),
        }
    }

    #[test]
    fn split_roundtrip() {
        let node = split(
            SavedSplitDir::Vertical,
            0.6,
            leaf("sh", "/home/a"),
            leaf("bash", "/home/b"),
        );
        let s = toml::to_string_pretty(&node).unwrap();
        let de: SavedPaneTree = toml::from_str(&s).unwrap();
        match de {
            SavedPaneTree::Split { direction, ratio, .. } => {
                assert_eq!(direction, SavedSplitDir::Vertical);
                assert!((ratio - 0.6).abs() < 1e-5);
            }
            _ => panic!("expected Split"),
        }
    }

    #[test]
    fn nested_split_roundtrip() {
        // Three-pane horizontal layout: left | (top / bottom)
        let tree = split(
            SavedSplitDir::Vertical,
            0.4,
            leaf("sh", "/a"),
            split(
                SavedSplitDir::Horizontal,
                0.5,
                leaf("bash", "/b"),
                leaf("zsh", "/c"),
            ),
        );
        let s = toml::to_string_pretty(&tree).unwrap();
        let de: SavedPaneTree = toml::from_str(&s).unwrap();
        // Re-serialise and compare text (normalised).
        let s2 = toml::to_string_pretty(&de).unwrap();
        assert_eq!(s, s2, "nested split roundtrip mismatch");
    }

    #[test]
    fn workspace_roundtrip() {
        let ws = SavedWorkspace {
            name: "my-work".into(),
            tabs: vec![
                SavedTab {
                    title: Some("main".into()),
                    tree: leaf("PowerShell", "C:/proj"),
                    group: None,
                },
                SavedTab {
                    title: None,
                    tree: split(
                        SavedSplitDir::Vertical,
                        0.5,
                        leaf("sh", "/a"),
                        leaf("sh", "/b"),
                    ),
                    group: None,
                },
            ],
            active_tab: 1,
            tab_groups: Vec::new(),
            next_group_id: 0,
        };
        let s = toml::to_string_pretty(&ws).unwrap();
        let de: SavedWorkspace = toml::from_str(&s).unwrap();
        assert_eq!(de.name, "my-work");
        assert_eq!(de.tabs.len(), 2);
        assert_eq!(de.active_tab, 1);
        assert_eq!(de.tabs[0].title.as_deref(), Some("main"));
    }

    // ── Tab-group serialisation ───────────────────────────────────────────────

    #[test]
    fn tab_group_definitions_roundtrip() {
        let ws = SavedWorkspace {
            name: "grouped".into(),
            tabs: vec![
                SavedTab { title: None, tree: leaf("sh", "/a"), group: Some(1) },
                SavedTab { title: None, tree: leaf("sh", "/b"), group: Some(2) },
                SavedTab { title: None, tree: leaf("sh", "/c"), group: Some(1) },
                SavedTab { title: None, tree: leaf("sh", "/d"), group: None },
            ],
            active_tab: 0,
            tab_groups: vec![
                SavedTabGroup { id: 1, name: "Build".into(), color: [0x4e, 0xa8, 0xff] },
                SavedTabGroup { id: 2, name: "Deploy".into(), color: [0x4e, 0xd4, 0x84] },
            ],
            next_group_id: 3,
        };
        let s = toml::to_string_pretty(&ws).unwrap();
        let de: SavedWorkspace = toml::from_str(&s).unwrap();
        assert_eq!(de.tab_groups.len(), 2);
        assert_eq!(de.tab_groups[0].id, 1);
        assert_eq!(de.tab_groups[0].name, "Build");
        assert_eq!(de.tab_groups[0].color, [0x4e, 0xa8, 0xff]);
        assert_eq!(de.tab_groups[1].id, 2);
        assert_eq!(de.tabs[0].group, Some(1));
        assert_eq!(de.tabs[1].group, Some(2));
        assert_eq!(de.tabs[2].group, Some(1));
        assert_eq!(de.tabs[3].group, None);
        assert_eq!(de.next_group_id, 3);
    }

    #[test]
    fn legacy_workspace_without_groups_loads_with_defaults() {
        // A serialised workspace without tab_groups / next_group_id fields must
        // still load cleanly (backwards compatibility).
        let toml_src = r#"
name = "legacy"
active_tab = 0

[[tabs]]
[tabs.tree]
type = "leaf"
profile = "sh"
cwd = "/home"
"#;
        let ws: SavedWorkspace = toml::from_str(toml_src).expect("legacy workspace must parse");
        assert!(ws.tab_groups.is_empty(), "absent tab_groups must default to empty");
        assert_eq!(ws.next_group_id, 0);
        assert_eq!(ws.tabs[0].group, None);
    }

    #[test]
    fn deleted_group_does_not_appear_in_saved_workspace() {
        // If a group is removed (id not in tab_groups), tabs that referenced it
        // should still round-trip — the id is just orphaned on restore.
        let ws = SavedWorkspace {
            name: "orphan".into(),
            tabs: vec![
                // Tab references group id 99 which is NOT in tab_groups.
                SavedTab { title: None, tree: leaf("sh", "/x"), group: Some(99) },
            ],
            active_tab: 0,
            tab_groups: Vec::new(), // group 99 was deleted
            next_group_id: 100,
        };
        let s = toml::to_string_pretty(&ws).unwrap();
        let de: SavedWorkspace = toml::from_str(&s).unwrap();
        // The orphaned id round-trips — callers handle the "no matching group" case.
        assert_eq!(de.tabs[0].group, Some(99));
        assert!(de.tab_groups.is_empty());
    }

    // ── Restore plan for a two-pane tab ──────────────────────────────────────

    #[test]
    fn restore_plan_single_leaf() {
        let tree = leaf("sh", "/home");
        let plan = restore_plan_for_tree(&tree);
        assert_eq!(plan.len(), 1);
        assert!(matches!(plan[0], RestoreStep::InitLeaf(_)));
    }

    #[test]
    fn restore_plan_vertical_split() {
        let tree = split(SavedSplitDir::Vertical, 0.4, leaf("sh", "/a"), leaf("bash", "/b"));
        let plan = restore_plan_for_tree(&tree);
        // Two panes: one init + one split.
        assert_eq!(plan.len(), 2);
        assert!(matches!(plan[0], RestoreStep::InitLeaf(_)));
        assert!(matches!(
            plan[1],
            RestoreStep::SplitLeaf { direction: SavedSplitDir::Vertical, side_b: true, .. }
        ));
        if let RestoreStep::SplitLeaf { ratio, .. } = plan[1] {
            assert!((ratio - 0.4).abs() < 1e-5);
        }
    }

    #[test]
    fn restore_plan_nested_split() {
        // left | (top / bottom) — 3 leaves
        let tree = split(
            SavedSplitDir::Vertical,
            0.4,
            leaf("sh", "/a"),
            split(
                SavedSplitDir::Horizontal,
                0.5,
                leaf("bash", "/b"),
                leaf("zsh", "/c"),
            ),
        );
        let plan = restore_plan_for_tree(&tree);
        // 3 leaves → 1 init + 2 splits
        assert_eq!(plan.len(), 3);
        assert!(matches!(plan[0], RestoreStep::InitLeaf(_)));
    }

    // ── Config roundtrip ──────────────────────────────────────────────────────

    #[test]
    fn restore_session_config_roundtrip() {
        use terminale_config::RestoreSession;
        use terminale_config::WindowConfig;
        let mut cfg = WindowConfig::default();
        // Default must be Off so we don't surprise users.
        assert_eq!(cfg.restore_session, RestoreSession::Off);
        cfg.restore_session = RestoreSession::LastSession;
        cfg.restore_working_dirs = false;
        let toml_text = toml::to_string_pretty(&cfg).unwrap();
        let de: WindowConfig = toml::from_str(&toml_text).unwrap();
        assert_eq!(de.restore_session, RestoreSession::LastSession);
        assert!(!de.restore_working_dirs);
    }
}
