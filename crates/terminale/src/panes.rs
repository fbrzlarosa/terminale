//! Pane tree geometry, divider hit-testing, split/close/zoom/resize operations,
//! and directional pane focus.

use crate::{
    DividerPath, LocalDividerSpec, LocalPaneSpec, PaneDirection, PaneId, PaneNode, RunningState,
    SplitDir, TabState,
};
use std::sync::Arc;

/// Result of searching for a resizable split: the path to the `Split` node
/// (usable with `set_split_ratio_at` / `split_ratio_at` / `walk_to_node_rect`)
/// and a flag indicating whether the focused pane is in the `a` subtree of
/// that node (`focused_in_a = true`) or the `b` subtree (`false`).
pub(crate) struct ResizeSplitResult {
    /// Path to the `Split` node itself (no trailing child step). `None` when
    /// the target leaf was found but no matching split has been identified yet
    /// (used as a sentinel while bubbling up the recursion).
    pub(crate) path: Option<DividerPath>,
    /// `true` when the focused pane lives under the `a` child of this split.
    pub(crate) focused_in_a: bool,
}

// ── split_in / collapse_close / first_leaf_of ─────────────────────────────────

/// Replace the leaf `target` in `node` with a `Split` of `target` and a
/// new leaf carrying `new_id`. `side_b` decides which side the new leaf
/// lives on (`true` puts the new pane on the b-side = right/bottom).
pub(crate) fn split_in(
    node: PaneNode,
    target: PaneId,
    direction: SplitDir,
    new_id: PaneId,
    side_b: bool,
) -> PaneNode {
    graft_in(node, target, direction, PaneNode::Leaf(new_id), side_b)
}

/// Replace the leaf `target` in `node` with a `Split` of `target` and an
/// arbitrary `subtree`. `side_b` decides which side the subtree lives on
/// (`true` puts it on the b-side = right/bottom). Generalisation of
/// [`split_in`] — grafting a whole pane tree is what powers "merge tab
/// into split". When `target` is not found the tree is returned unchanged
/// (and the subtree is dropped — callers must guard target existence when
/// the subtree carries panes they care about).
pub(crate) fn graft_in(
    node: PaneNode,
    target: PaneId,
    direction: SplitDir,
    subtree: PaneNode,
    side_b: bool,
) -> PaneNode {
    // Thread the subtree through an Option so the recursion can MOVE it
    // into the (single) graft point without cloning pane ids around.
    fn go(
        node: PaneNode,
        target: PaneId,
        direction: SplitDir,
        subtree: &mut Option<PaneNode>,
        side_b: bool,
    ) -> PaneNode {
        match node {
            PaneNode::Leaf(id) if id == target => {
                let Some(sub) = subtree.take() else {
                    return PaneNode::Leaf(id);
                };
                let (a, b) = if side_b {
                    (Box::new(PaneNode::Leaf(target)), Box::new(sub))
                } else {
                    (Box::new(sub), Box::new(PaneNode::Leaf(target)))
                };
                PaneNode::Split {
                    direction,
                    ratio: 0.5,
                    a,
                    b,
                }
            }
            PaneNode::Leaf(_) => node,
            PaneNode::Split {
                direction: d,
                ratio,
                a,
                b,
            } => {
                let a = Box::new(go(*a, target, direction, subtree, side_b));
                let b = Box::new(go(*b, target, direction, subtree, side_b));
                PaneNode::Split {
                    direction: d,
                    ratio,
                    a,
                    b,
                }
            }
        }
    }
    let mut sub = Some(subtree);
    go(node, target, direction, &mut sub, side_b)
}

/// Rewrite every leaf id in `node` through `map`. Ids missing from the map
/// are left unchanged (callers build a complete map, so that's defensive).
/// Used when grafting a foreign pane tree whose ids must be re-allocated
/// into the destination tab's id space.
pub(crate) fn remap_leaf_ids(
    node: PaneNode,
    map: &std::collections::HashMap<PaneId, PaneId>,
) -> PaneNode {
    match node {
        PaneNode::Leaf(id) => PaneNode::Leaf(map.get(&id).copied().unwrap_or(id)),
        PaneNode::Split {
            direction,
            ratio,
            a,
            b,
        } => PaneNode::Split {
            direction,
            ratio,
            a: Box::new(remap_leaf_ids(*a, map)),
            b: Box::new(remap_leaf_ids(*b, map)),
        },
    }
}

// ── Drop-side geometry (drag a tab/pane onto a pane body) ───────────────────

/// Which half of a drop-target pane the dragged item will occupy when a
/// tab / pane is dropped onto a terminal body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropSide {
    /// Left half — vertical split, dragged item on the a-side.
    Left,
    /// Right half — vertical split, dragged item on the b-side.
    Right,
    /// Top half — horizontal split, dragged item on the a-side.
    Top,
    /// Bottom half — horizontal split, dragged item on the b-side.
    Bottom,
}

impl DropSide {
    /// The `(direction, side_b)` pair to feed the split/graft helpers so
    /// the dragged item lands on this side of the target pane.
    pub(crate) fn split(self) -> (SplitDir, bool) {
        match self {
            Self::Left => (SplitDir::Vertical, false),
            Self::Right => (SplitDir::Vertical, true),
            Self::Top => (SplitDir::Horizontal, false),
            Self::Bottom => (SplitDir::Horizontal, true),
        }
    }

    /// The half of `rect` (physical px, `(x, y, w, h)`) this side covers —
    /// used to draw the drop-zone highlight during the drag.
    pub(crate) fn half_rect(self, rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = rect;
        match self {
            Self::Left => (x, y, w / 2.0, h),
            Self::Right => (x + w / 2.0, y, w / 2.0, h),
            Self::Top => (x, y, w, h / 2.0),
            Self::Bottom => (x, y + h / 2.0, w, h / 2.0),
        }
    }
}

/// Which [`DropSide`] of `rect` the point `(x, y)` falls toward: the offset
/// from the rect centre is normalised per axis and the dominant axis wins,
/// so the rect is effectively cut into four triangles by its diagonals —
/// the standard editor drop-zone feel (VS Code, Zed).
pub(crate) fn drop_side_for(rect: (f32, f32, f32, f32), x: f32, y: f32) -> DropSide {
    let (rx, ry, rw, rh) = rect;
    let nx = ((x - rx) / rw.max(1.0)).clamp(0.0, 1.0) - 0.5;
    let ny = ((y - ry) / rh.max(1.0)).clamp(0.0, 1.0) - 0.5;
    if nx.abs() >= ny.abs() {
        if nx < 0.0 {
            DropSide::Left
        } else {
            DropSide::Right
        }
    } else if ny < 0.0 {
        DropSide::Top
    } else {
        DropSide::Bottom
    }
}

/// Walk `node` removing the leaf `target` — when its parent `Split` is
/// found, the parent is replaced by the sibling subtree. Returns
/// `(new_tree, true)` when the target was found and removed,
/// `(unchanged_tree, false)` otherwise.
pub(crate) fn collapse_close(node: PaneNode, target: PaneId) -> (PaneNode, bool) {
    match node {
        PaneNode::Leaf(_) => (node, false),
        PaneNode::Split {
            direction,
            ratio,
            a,
            b,
        } => {
            // Direct hit on either child? Collapse the parent.
            if matches!(*a, PaneNode::Leaf(id) if id == target) {
                return (*b, true);
            }
            if matches!(*b, PaneNode::Leaf(id) if id == target) {
                return (*a, true);
            }
            // Recurse into both subtrees.
            let (new_a, found_a) = collapse_close(*a, target);
            if found_a {
                return (
                    PaneNode::Split {
                        direction,
                        ratio,
                        a: Box::new(new_a),
                        b,
                    },
                    true,
                );
            }
            let (new_b, found_b) = collapse_close(*b, target);
            (
                PaneNode::Split {
                    direction,
                    ratio,
                    a: Box::new(new_a),
                    b: Box::new(new_b),
                },
                found_b,
            )
        }
    }
}

/// First leaf id encountered in a depth-first walk — used to pick the
/// next focused pane after a close.
pub(crate) fn first_leaf_of(node: &PaneNode) -> Option<PaneId> {
    match node {
        PaneNode::Leaf(id) => Some(*id),
        PaneNode::Split { a, .. } => first_leaf_of(a),
    }
}

// ── count_leaves ──────────────────────────────────────────────────────────────

/// Count the number of leaf nodes in a pane tree.
///
/// A single-leaf tab returns `1`. A tree with one split returns `2`.
/// Deeper nesting sums both children recursively.
pub(crate) fn count_leaves(node: &PaneNode) -> usize {
    match node {
        PaneNode::Leaf(_) => 1,
        PaneNode::Split { a, b, .. } => count_leaves(a) + count_leaves(b),
    }
}

// ── detach_leaf ───────────────────────────────────────────────────────────────

/// Remove pane `pane_id` from `tab`'s tree and return the detached [`crate::Pane`].
///
/// Mirrors the first half of [`crate::TabState::close_focused`] but works on
/// an *arbitrary* leaf rather than the focused one, and **returns** the removed
/// pane so the caller can reattach it elsewhere.
///
/// # Preconditions
/// * `count_leaves(&tab.tree) > 1` — callers must guard this (a lone leaf
///   cannot be detached; the whole tab would be empty).
/// * `pane_id` is a leaf in `tab.tree` — if not found, `None` is returned and
///   `tab` is left unchanged.
///
/// Side-effects on success:
/// * `tab.zoomed_pane` is cleared (un-zoom before detach, consistent with
///   the close flow).
/// * The focused leaf is updated to the first remaining leaf so the tab does
///   not hold a stale focus reference.
pub(crate) fn detach_leaf(tab: &mut crate::TabState, pane_id: PaneId) -> Option<crate::Pane> {
    if matches!(tab.tree, PaneNode::Leaf(_)) {
        // Lone leaf — cannot detach; caller must guard count_leaves > 1.
        return None;
    }
    // Un-zoom: a zoom on the departing pane (or any pane) leaves the layout
    // in an inconsistent state; clear it unconditionally.
    tab.zoomed_pane = None;

    // Swap out the tree, collapse around `pane_id`, swap back.
    let owned = std::mem::replace(&mut tab.tree, PaneNode::Leaf(pane_id));
    let (new_tree, found) = collapse_close(owned, pane_id);
    tab.tree = new_tree;

    if !found {
        return None;
    }

    let pane = tab.panes.remove(&pane_id)?;
    tab.focused = first_leaf_of(&tab.tree).unwrap_or(pane_id);
    Some(pane)
}

// ── pane_label / compose_tab_label / short_cwd / tab_label ───────────────────

/// A program-announced OSC title worth displaying, or `None` for "noise":
/// an empty title, or the shell's own `…\powershell.exe` path that ConPTY
/// sets (ugly as a label). Real titles ("vim main.rs", "ssh host") survive.
pub(crate) fn useful_program_title(t: &str) -> Option<&str> {
    let t = t.trim();
    if t.is_empty() || t.to_ascii_lowercase().ends_with(".exe") {
        None
    } else {
        Some(t)
    }
}

/// Pure tab-label composition (split out for testing). Priority: an explicit
/// user-set name (rename) wins over everything; then a program-announced OSC
/// title; otherwise `profile — cwd`; falls back to the profile name. Long
/// titles are truncated; a crashed tab is flagged.
pub(crate) fn compose_tab_label(
    user_title: Option<&str>,
    profile_name: &str,
    custom_title: Option<&str>,
    cwd_short: Option<&str>,
    crashed: bool,
) -> String {
    let truncate = |t: &str| -> String {
        if t.chars().count() > 40 {
            let head: String = t.chars().take(39).collect();
            format!("{head}…")
        } else {
            t.to_string()
        }
    };
    let user = user_title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(truncate);
    let base = match user {
        Some(u) => u,
        None => match custom_title.and_then(useful_program_title) {
            Some(t) => truncate(t),
            None => match cwd_short {
                Some(short) if !short.is_empty() => format!("{profile_name} — {short}"),
                _ => profile_name.to_string(),
            },
        },
    };
    if crashed {
        format!("⚠ {base} (crashed)")
    } else {
        base
    }
}

/// Last path segment of a cwd, with `~` substitution for the home dir.
pub(crate) fn short_cwd(path: &str) -> String {
    // Replace HOME with ~ where applicable.
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok());
    let display = match home {
        Some(h) if path.starts_with(&h) => {
            let stripped = &path[h.len()..];
            if stripped.is_empty() {
                "~".to_string()
            } else {
                format!("~{}", stripped.replace('\\', "/"))
            }
        }
        _ => path.replace('\\', "/"),
    };
    // Trim to the last 2 segments for screen real estate.
    let parts: Vec<&str> = display.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => display,
        [last] => (*last).to_string(),
        [.., a, b] => format!("{a}/{b}"),
    }
}

/// Display label for a tab — prefixes a warning glyph when the tab has
/// crashed so users can spot dead tabs at a glance. When the shell has
/// announced a cwd via OSC 7 we append the trailing path component so
/// the user can tell tabs apart at a glance ("PowerShell — repo").
pub(crate) fn tab_label(tab: &TabState) -> String {
    let cwd = tab.emulator.lock().current_dir().map(short_cwd);
    compose_tab_label(
        tab.user_title.as_deref(),
        &tab.profile_name,
        tab.custom_title.as_deref(),
        cwd.as_deref(),
        tab.crashed,
    )
}

/// Build a display label for an individual split pane. Mirrors
/// [`compose_tab_label`] semantics: prefers a program-announced title
/// (filtered through [`useful_program_title`]); otherwise
/// `profile_name — short_cwd`; truncated at 40 chars with an ellipsis.
/// No crash marker — crashes are shown at the tab-pill level.
pub(crate) fn pane_label(pane: &crate::Pane) -> String {
    let cwd = pane.emulator.lock().current_dir().map(short_cwd);
    compose_tab_label(
        pane.user_title.as_deref(),
        &pane.profile_name,
        pane.custom_title.as_deref(),
        cwd.as_deref(),
        false, // no crash prefix in pane headers
    )
}

// ── pane_specs_for_tab / walk_pane_tree ───────────────────────────────────────

/// Flatten the active tab's pane tree into the list of leaves the
/// renderer draws — one [`LocalPaneSpec`] per leaf, with its physical-px
/// sub-rect computed from the tree's `Split` ratios. Header rects are
/// carved out when `state.show_pane_headers` is `true` and the tab has
/// more than one leaf.
pub(crate) fn pane_specs_for_tab(state: &RunningState, tab: &TabState) -> Vec<LocalPaneSpec> {
    let surface = state.window.inner_size();
    let scale = state.window.scale_factor() as f32;
    let top_pad_px = state.renderer.body_top_px();
    let bottom_px = state.renderer.body_bottom_px(surface.height);
    // Account for a vertical tab strip on the left or right: body_left_px /
    // body_right_px include the strip width so the terminal grid does not
    // overlap it.
    let left_px = state.renderer.body_left_px();
    let right_px = state.renderer.body_right_px(surface.width);
    let body_rect = (
        left_px,
        top_pad_px,
        (right_px - left_px).max(0.0),
        (bottom_px - top_pad_px).max(0.0),
    );
    let cell_h_px = state.renderer.cell_height() * scale;
    let header_h_px = terminale_render::PANE_HEADER_HEIGHT * scale;

    // Pane-zoom: when a pane is zoomed, synthesise a single spec that fills
    // the whole body rect — no dividers, no other panes visible.
    if let Some(zoom_id) = tab.zoomed_pane {
        if let Some(pane) = tab.panes.get(&zoom_id) {
            let title = pane_label(pane);
            let mut out = vec![LocalPaneSpec {
                pane_id: zoom_id,
                rect_px: body_rect,
                header_rect_px: None,
                title,
                emulator: Arc::clone(&pane.emulator),
                scroll_lines: pane.scroll_lines,
                focused: true,
            }];
            // Rename overlay still applies in zoom mode.
            if let Some(rename) = &state.renaming {
                let targets_pane =
                    matches!(rename.target, crate::RenameTarget::Pane(pid) if pid == zoom_id);
                if targets_pane {
                    if let Some(s) = out.first_mut() {
                        s.title = format!("{}|", rename.buffer);
                    }
                }
            }
            return out;
        }
    }

    let leaves = count_leaves(&tab.tree);
    let with_headers = leaves > 1 && state.show_pane_headers;
    let mut out = Vec::new();
    walk_pane_tree(
        &tab.tree,
        body_rect,
        tab,
        with_headers,
        header_h_px,
        cell_h_px,
        &mut out,
    );
    // Mark the focused leaf — there's always exactly one in the tree.
    for spec in &mut out {
        spec.focused = spec.pane_id == tab.focused;
    }
    // While renaming a specific pane, substitute the live buffer + caret
    // into that pane's header title so the user sees their edit in-place.
    // Rename takes priority over the spinner — do this before the spinner pass.
    let rename_pane_id: Option<crate::PaneId> = state.renaming.as_ref().and_then(|r| {
        if let crate::RenameTarget::Pane(pid) = r.target {
            Some(pid)
        } else {
            None
        }
    });
    if let Some(pid) = rename_pane_id {
        for spec in &mut out {
            if spec.pane_id == pid {
                spec.title = format!("{}|", state.renaming.as_ref().unwrap().buffer);
            }
        }
    }
    // Prepend the busy-spinner frame to each pane header when the spinner is
    // enabled and headers are visible (i.e. `with_headers` was true). Skip
    // panes being renamed — their live-buffer title takes precedence.
    if with_headers && state.tab_activity_spinner {
        let spinner = crate::SPINNER_FRAMES[state.spinner_frame % crate::SPINNER_FRAMES.len()];
        for spec in &mut out {
            // Don't overlay the spinner onto a live-rename buffer.
            if rename_pane_id == Some(spec.pane_id) {
                continue;
            }
            if let Some(pane) = tab.panes.get(&spec.pane_id) {
                if crate::pane_is_busy(pane) {
                    spec.title = format!("{spinner}  {}", spec.title);
                }
            }
        }
    }
    out
}

/// Recursive helper for [`pane_specs_for_tab`]: walks `node` allocating
/// `rect_px` to its leaves according to each `Split`'s direction +
/// ratio. Vertical splits divide horizontally (left / right), horizontal
/// splits divide vertically (top / bottom) — matching the user-facing
/// orientation of the divider.
///
/// When `with_headers` is `true` and the leaf rect is tall enough, carves
/// `header_h_px` off the top of each leaf into `header_rect_px` so the
/// grid only occupies the area below the strip.
pub(crate) fn walk_pane_tree(
    node: &PaneNode,
    rect: (f32, f32, f32, f32),
    tab: &TabState,
    with_headers: bool,
    header_h_px: f32,
    cell_h_px: f32,
    out: &mut Vec<LocalPaneSpec>,
) {
    let (x, y, w, h) = rect;
    match node {
        PaneNode::Leaf(id) => {
            if let Some(pane) = tab.panes.get(id) {
                // Carve a header band off the top when enabled and the
                // remaining grid height fits at least one row.
                let (header_rect_px, grid_rect) =
                    if with_headers && h > header_h_px + cell_h_px.max(1.0) {
                        (
                            Some((x, y, w, header_h_px)),
                            (x, y + header_h_px, w, h - header_h_px),
                        )
                    } else {
                        (None, rect)
                    };
                let title = pane_label(pane);
                out.push(LocalPaneSpec {
                    pane_id: *id,
                    rect_px: grid_rect,
                    header_rect_px,
                    title,
                    emulator: Arc::clone(&pane.emulator),
                    scroll_lines: pane.scroll_lines,
                    focused: false, // filled in by the caller
                });
            }
        }
        PaneNode::Split {
            direction,
            ratio,
            a,
            b,
        } => {
            let r = ratio.clamp(0.05, 0.95);
            match direction {
                SplitDir::Vertical => {
                    // Children sit left / right of a vertical divider.
                    let aw = (w * r).floor();
                    let bw = (w - aw).max(0.0);
                    walk_pane_tree(
                        a,
                        (x, y, aw, h),
                        tab,
                        with_headers,
                        header_h_px,
                        cell_h_px,
                        out,
                    );
                    walk_pane_tree(
                        b,
                        (x + aw, y, bw, h),
                        tab,
                        with_headers,
                        header_h_px,
                        cell_h_px,
                        out,
                    );
                }
                SplitDir::Horizontal => {
                    // Children stack top / bottom of a horizontal divider.
                    let ah = (h * r).floor();
                    let bh = (h - ah).max(0.0);
                    walk_pane_tree(
                        a,
                        (x, y, w, ah),
                        tab,
                        with_headers,
                        header_h_px,
                        cell_h_px,
                        out,
                    );
                    walk_pane_tree(
                        b,
                        (x, y + ah, w, bh),
                        tab,
                        with_headers,
                        header_h_px,
                        cell_h_px,
                        out,
                    );
                }
            }
        }
    }
}

// ── focus_pane_under_cursor / pane_header_close_at / pane_header_at ──────────

/// Click-to-focus pane: hit-test the pointer (physical px in surface space)
/// against the active tab's pane sub-rects and, if it lands inside a
/// non-focused pane's sub-rect, mark that pane as the new focused one.
pub(crate) fn focus_pane_under_cursor(state: &mut RunningState, pointer_phys: (f32, f32)) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let specs = pane_specs_for_tab(state, tab);
    let mut target: Option<PaneId> = None;
    for spec in &specs {
        let (x, y, w, h) = spec.rect_px;
        if pointer_phys.0 >= x
            && pointer_phys.0 < x + w
            && pointer_phys.1 >= y
            && pointer_phys.1 < y + h
        {
            target = Some(spec.pane_id);
            break;
        }
    }
    if let Some(id) = target {
        if let Some(tab) = state.tabs.get_mut(state.active_tab) {
            if tab.focused != id {
                tab.focused = id;
                state.pending_hook_pane_focus.push(id);
                state.window.request_redraw();
            }
        }
    }
}

/// Return the pane id whose header close-X glyph box contains `pointer_phys`,
/// or `None`. Geometry mirrors the renderer: close box is 16 × logical px
/// right-aligned with 4 px right inset, vertically at 3 px inside the strip.
pub(crate) fn pane_header_close_at(
    state: &RunningState,
    pointer_phys: (f32, f32),
) -> Option<PaneId> {
    let tab = state.tabs.get(state.active_tab)?;
    let specs = pane_specs_for_tab(state, tab);
    let scale = state.window.scale_factor() as f32;
    let close_box_w = 16.0 * scale;
    let close_box_h = 16.0 * scale;
    let close_inset_r = 4.0 * scale;
    let close_inset_y = 3.0 * scale;
    for spec in &specs {
        let (hx, hy, hw, hh) = spec.header_rect_px?;
        let bx = hx + hw - close_inset_r - close_box_w;
        let by = hy + close_inset_y;
        // Ensure the close box actually fits inside the header.
        if close_box_w >= hw || close_box_h >= hh {
            continue;
        }
        if pointer_phys.0 >= bx
            && pointer_phys.0 < bx + close_box_w
            && pointer_phys.1 >= by
            && pointer_phys.1 < by + close_box_h
        {
            return Some(spec.pane_id);
        }
    }
    None
}

/// Return the pane id whose header strip (excluding the close-X box) contains
/// `pointer_phys`, or `None`. Used to focus-on-header-click.
pub(crate) fn pane_header_at(state: &RunningState, pointer_phys: (f32, f32)) -> Option<PaneId> {
    let tab = state.tabs.get(state.active_tab)?;
    let specs = pane_specs_for_tab(state, tab);
    for spec in &specs {
        let (hx, hy, hw, hh) = spec.header_rect_px?;
        if pointer_phys.0 >= hx
            && pointer_phys.0 < hx + hw
            && pointer_phys.1 >= hy
            && pointer_phys.1 < hy + hh
        {
            return Some(spec.pane_id);
        }
    }
    None
}

// ── walk_divider_tree / divider_specs_for_tab / hit_test_divider ─────────────

/// Recursive companion to [`walk_pane_tree`]: walks `node` allocating a
/// [`LocalDividerSpec`] at every `Split`. The visible stroke is centred
/// on the geometric boundary at the configured thickness; the hit-test
/// rect grows by `grab_pad_px` on each side along the perpendicular axis
/// so the user doesn't need pixel precision to grab a divider.
pub(crate) fn walk_divider_tree(
    node: &PaneNode,
    rect: (f32, f32, f32, f32),
    path: &mut Vec<bool>,
    thickness_px: f32,
    grab_pad_px: f32,
    out: &mut Vec<LocalDividerSpec>,
) {
    let (x, y, w, h) = rect;
    match node {
        PaneNode::Leaf(_) => {}
        PaneNode::Split {
            direction,
            ratio,
            a,
            b,
        } => {
            let r = ratio.clamp(0.05, 0.95);
            let half_thick = thickness_px / 2.0;
            let half_band = half_thick + grab_pad_px;
            match direction {
                SplitDir::Vertical => {
                    let aw = (w * r).floor();
                    let boundary_x = x + aw;
                    let visible = (boundary_x - half_thick, y, thickness_px, h);
                    let hit = (boundary_x - half_band, y, half_band * 2.0, h);
                    out.push(LocalDividerSpec {
                        path: path.clone(),
                        axis: SplitDir::Vertical,
                        rect_px: hit,
                        visible_rect_px: visible,
                    });
                    let bw = (w - aw).max(0.0);
                    path.push(false);
                    walk_divider_tree(a, (x, y, aw, h), path, thickness_px, grab_pad_px, out);
                    path.pop();
                    path.push(true);
                    walk_divider_tree(b, (x + aw, y, bw, h), path, thickness_px, grab_pad_px, out);
                    path.pop();
                }
                SplitDir::Horizontal => {
                    let ah = (h * r).floor();
                    let boundary_y = y + ah;
                    let visible = (x, boundary_y - half_thick, w, thickness_px);
                    let hit = (x, boundary_y - half_band, w, half_band * 2.0);
                    out.push(LocalDividerSpec {
                        path: path.clone(),
                        axis: SplitDir::Horizontal,
                        rect_px: hit,
                        visible_rect_px: visible,
                    });
                    let bh = (h - ah).max(0.0);
                    path.push(false);
                    walk_divider_tree(a, (x, y, w, ah), path, thickness_px, grab_pad_px, out);
                    path.pop();
                    path.push(true);
                    walk_divider_tree(b, (x, y + ah, w, bh), path, thickness_px, grab_pad_px, out);
                    path.pop();
                }
            }
        }
    }
}

/// Flatten the active tab's pane tree into the list of divider hit-targets,
/// one [`LocalDividerSpec`] per `Split` node.
pub(crate) fn divider_specs_for_tab(state: &RunningState, tab: &TabState) -> Vec<LocalDividerSpec> {
    let surface = state.window.inner_size();
    let top_pad_px = state.renderer.body_top_px();
    let bottom_px = state.renderer.body_bottom_px(surface.height);
    let body_rect = (
        0.0,
        top_pad_px,
        surface.width as f32,
        (bottom_px - top_pad_px).max(0.0),
    );
    let mut out = Vec::new();
    let mut path = Vec::new();
    walk_divider_tree(
        &tab.tree,
        body_rect,
        &mut path,
        state.divider_thickness_px,
        state.divider_grab_padding_px,
        &mut out,
    );
    out
}

/// Hit-test a pointer (physical px in surface space) against the divider
/// specs returned by [`divider_specs_for_tab`]. Returns the (path, axis)
/// of the first containing rect found, iterating in **reverse** so that
/// deeper splits — which are emitted later by the walker — win over an
/// outer split whose grab band happens to overlap a sibling's.
pub(crate) fn hit_test_divider(
    specs: &[LocalDividerSpec],
    pointer_phys: (f32, f32),
) -> Option<(DividerPath, SplitDir)> {
    for spec in specs.iter().rev() {
        let (x, y, w, h) = spec.rect_px;
        if pointer_phys.0 >= x
            && pointer_phys.0 < x + w
            && pointer_phys.1 >= y
            && pointer_phys.1 < y + h
        {
            return Some((spec.path.clone(), spec.axis));
        }
    }
    None
}

// ── split_node_mut_at_path / set_split_ratio_at / split_ratio_at ─────────────

/// Walk `path` from the root, descending into `a` (`false`) or `b` (`true`)
/// at each step. Returns the `Split` node reached at the end, or `None`
/// if the path no longer resolves.
pub(crate) fn split_node_mut_at_path<'a>(
    tree: &'a mut PaneNode,
    path: &[bool],
) -> Option<&'a mut PaneNode> {
    let mut node = tree;
    for &step in path {
        match node {
            PaneNode::Leaf(_) => return None,
            PaneNode::Split { a, b, .. } => {
                node = if step { b.as_mut() } else { a.as_mut() };
            }
        }
    }
    // The final node must itself be a `Split` — a divider can only live
    // on a Split node. If we landed on a Leaf, the tree shape changed
    // and the path is stale.
    if matches!(node, PaneNode::Split { .. }) {
        Some(node)
    } else {
        None
    }
}

/// Clamp `new_ratio` to the same range [`walk_pane_tree`] uses and mutate
/// `Split.ratio` at the node addressed by `path`. Returns `true` when the
/// path resolved and the mutation took effect; `false` when the path is
/// stale (tree mutated since capture).
pub(crate) fn set_split_ratio_at(tree: &mut PaneNode, path: &[bool], new_ratio: f32) -> bool {
    let Some(node) = split_node_mut_at_path(tree, path) else {
        return false;
    };
    if let PaneNode::Split { ratio, .. } = node {
        *ratio = new_ratio.clamp(0.05, 0.95);
        true
    } else {
        false
    }
}

/// Re-derive the physical-px rect of the `Split` node at `path` by
/// re-walking the tree alongside the body-rect logic. Returns `None` if
/// the path no longer resolves.
pub(crate) fn parent_rect_for_divider(
    state: &RunningState,
    tab: &TabState,
    path: &DividerPath,
) -> Option<(f32, f32, f32, f32)> {
    let surface = state.window.inner_size();
    let top_pad_px = state.renderer.body_top_px();
    let bottom_px = state.renderer.body_bottom_px(surface.height);
    let body_rect = (
        0.0,
        top_pad_px,
        surface.width as f32,
        (bottom_px - top_pad_px).max(0.0),
    );
    walk_to_node_rect(&tab.tree, body_rect, path)
}

/// Internal: walk the tree alongside its layout math and return the rect
/// of the node at `path`.
pub(crate) fn walk_to_node_rect(
    node: &PaneNode,
    rect: (f32, f32, f32, f32),
    path: &[bool],
) -> Option<(f32, f32, f32, f32)> {
    if path.is_empty() {
        return Some(rect);
    }
    let (x, y, w, h) = rect;
    match node {
        PaneNode::Leaf(_) => None,
        PaneNode::Split {
            direction,
            ratio,
            a,
            b,
        } => {
            let r = ratio.clamp(0.05, 0.95);
            let (a_rect, b_rect) = match direction {
                SplitDir::Vertical => {
                    let aw = (w * r).floor();
                    let bw = (w - aw).max(0.0);
                    ((x, y, aw, h), (x + aw, y, bw, h))
                }
                SplitDir::Horizontal => {
                    let ah = (h * r).floor();
                    let bh = (h - ah).max(0.0);
                    ((x, y, w, ah), (x, y + ah, w, bh))
                }
            };
            let (step, rest) = path.split_first()?;
            if *step {
                walk_to_node_rect(b, b_rect, rest)
            } else {
                walk_to_node_rect(a, a_rect, rest)
            }
        }
    }
}

/// Look up the current ratio at the `Split` addressed by `path`. Returns
/// `None` when the path is stale.
pub(crate) fn split_ratio_at(tree: &PaneNode, path: &[bool]) -> Option<f32> {
    let mut node = tree;
    for &step in path {
        match node {
            PaneNode::Leaf(_) => return None,
            PaneNode::Split { a, b, .. } => {
                node = if step { b.as_ref() } else { a.as_ref() };
            }
        }
    }
    if let PaneNode::Split { ratio, .. } = node {
        Some(*ratio)
    } else {
        None
    }
}

// ── cursor_icon_for_divider / update_divider_drag / update_divider_hover ─────

/// Map a divider axis to the cursor icon the user should see while
/// hovering / dragging it.
pub(crate) fn cursor_icon_for_divider(axis: SplitDir) -> winit::window::CursorIcon {
    match axis {
        SplitDir::Vertical => winit::window::CursorIcon::EwResize,
        SplitDir::Horizontal => winit::window::CursorIcon::NsResize,
    }
}

/// While a divider drag is in flight, recompute the `Split.ratio` from the
/// current pointer position and apply it.
pub(crate) fn update_divider_drag(state: &mut RunningState, pointer_phys: (f32, f32)) {
    let Some(drag) = state.pending_divider_drag.clone() else {
        return;
    };
    let (px, py, pw, ph) = drag.parent_rect_px;
    let new_ratio = match drag.axis {
        SplitDir::Vertical => {
            if pw <= 0.0 {
                drag.start_ratio
            } else {
                let local_x = pointer_phys.0 - px;
                (local_x / pw).clamp(0.05, 0.95)
            }
        }
        SplitDir::Horizontal => {
            if ph <= 0.0 {
                drag.start_ratio
            } else {
                let local_y = pointer_phys.1 - py;
                (local_y / ph).clamp(0.05, 0.95)
            }
        }
    };
    let Some(tab) = state.tabs.get_mut(state.active_tab) else {
        state.pending_divider_drag = None;
        return;
    };
    let applied = set_split_ratio_at(&mut tab.tree, &drag.path, new_ratio);
    if !applied {
        // Path stale — abort cleanly.
        state.pending_divider_drag = None;
        state.hovered_divider = None;
        state.window.set_cursor(winit::window::CursorIcon::Default);
        return;
    }
    if state.live_pane_resize {
        resize_active_tab_panes(state);
    }
    state.window.request_redraw();
}

/// Finalise a divider drag on left-release.
pub(crate) fn finish_divider_drag(state: &mut RunningState) {
    state.pending_divider_drag = None;
    resize_active_tab_panes(state);
    state.window.request_redraw();
}

/// Update the hovered-divider state for the current pointer position.
/// Returns `true` when a divider was hovered.
pub(crate) fn update_divider_hover(state: &mut RunningState, pointer_phys: (f32, f32)) -> bool {
    let specs = state
        .tabs
        .get(state.active_tab)
        .map(|tab| divider_specs_for_tab(state, tab))
        .unwrap_or_default();
    let hit = hit_test_divider(&specs, pointer_phys);
    let changed = match (&state.hovered_divider, &hit) {
        (None, None) => false,
        (Some(a), Some(b)) => a != b,
        _ => true,
    };
    if changed {
        if let Some((_, axis)) = &hit {
            state.window.set_cursor(cursor_icon_for_divider(*axis));
        } else {
            state.window.set_cursor(winit::window::CursorIcon::Default);
        }
    }
    state.hovered_divider = hit;
    state.hovered_divider.is_some()
}

// ── resize_active_tab_panes ───────────────────────────────────────────────────

/// Resize each pane in the active tab to fit its computed sub-rect
/// after a tree change (split, close, divider drag in a future phase).
pub(crate) fn resize_active_tab_panes(state: &mut RunningState) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let specs = pane_specs_for_tab(state, tab);
    // Collect (pane_id, cols, rows) then apply — can't mutate state
    // while iterating its tabs / panes immutably.
    let updates: Vec<(PaneId, u16, u16)> = specs
        .iter()
        .map(|s| {
            let (_, _, w, h) = s.rect_px;
            // Pane sub-rects are chrome-free (built from the body area), so
            // convert without re-subtracting the chrome offsets — same fix
            // as in resize_all_tabs.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (cols, rows) = state
                .renderer
                .rect_to_cells(w.max(1.0) as u32, h.max(1.0) as u32);
            (s.pane_id, cols, rows)
        })
        .collect();
    let Some(tab) = state.tabs.get_mut(state.active_tab) else {
        return;
    };
    for (id, cols, rows) in updates {
        if let Some(pane) = tab.panes.get_mut(&id) {
            // Same-size guard — see resize_all_tabs for the rationale.
            if pane.cols == cols && pane.rows == rows {
                continue;
            }
            // Emulator FIRST — grid must be at new size before PTY notifies
            // the shell (same rationale as in resize_all_tabs).
            pane.emulator.lock().resize(cols, rows);
            pane.session.resize(cols, rows).ok();
            pane.cols = cols;
            pane.rows = rows;
        }
    }
}

// ── resolved_divider_color ────────────────────────────────────────────────────

/// Pick the RGB the divider stroke should use this frame: an explicit
/// override from `appearance.divider_color`, or a neutral fallback derived
/// from the renderer's background.
pub(crate) fn resolved_divider_color(state: &RunningState) -> [u8; 3] {
    if let Some(rgb) = state.divider_color {
        return rgb;
    }
    let bg = terminale_render::BACKGROUND_RGB;
    let lighten = |c: u8| -> u8 {
        // Move 15% of the way toward white. Saturates at 255.
        let bumped = u16::from(c) + ((u16::from(255 - c) * 38) / 255);
        bumped.min(255) as u8
    };
    [lighten(bg[0]), lighten(bg[1]), lighten(bg[2])]
}

// ── split_focused_pane / close_focused_pane ───────────────────────────────────

/// Spawn a sibling pane and splice it into the focused tab's pane tree.
pub(crate) fn split_focused_pane(state: &mut RunningState, direction: SplitDir, side_b: bool) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let size = state.window.inner_size();
    let initial = (terminale_term::DEFAULT_COLS, terminale_term::DEFAULT_ROWS);
    // Inherit cwd from the focused pane like new_tab does.
    let inherited_cwd: Option<std::path::PathBuf> = tab
        .emulator
        .lock()
        .current_dir()
        .map(std::path::PathBuf::from);
    let profile: Option<terminale_config::Profile> =
        inherited_cwd.map(|cwd| terminale_config::Profile {
            name: tab.profile_name.clone(),
            command: String::new(),
            args: Vec::new(),
            env: Default::default(),
            cwd: Some(cwd),
            icon: tab.icon.clone(),
        });
    let new_pane = crate::spawn_pane(
        profile.as_ref(),
        None,
        &state.renderer,
        initial,
        size.width,
        size.height,
        state.proxy.clone(),
        state.scrollback_lines,
        state.shell_integration,
    );
    // Capture the program label for the session_start hook before the pane
    // is moved into the tab (the profile name was used to build it).
    let program = profile
        .as_ref()
        .map_or_else(|| "shell".to_string(), |p| p.name.clone());
    // Match the new pane's emulator palette to the tab's theme.
    new_pane.emulator.lock().set_palette(state.palette);
    let new_pane_id = if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        // Un-zoom before splitting: a zoom state on an about-to-be-split pane
        // would produce a confusing layout (the zoom rect would cover both
        // new panes). Reset it so the normal tree layout takes over.
        tab.zoomed_pane = None;
        Some(tab.split_focused(direction, new_pane, side_b))
    } else {
        None
    };
    // Enqueue session_start for the freshly-spawned pane.
    if let Some(id) = new_pane_id {
        state.pending_hook_session_start.push((id, program));
        // Focus moved to the new pane — notify plugins.
        state.pending_hook_pane_focus.push(id);
    }
    // Resize all panes in the tab to fit their new sub-rects so the
    // PTY + emulator dimensions match what the renderer will draw.
    resize_active_tab_panes(state);
    state.renderer.set_selection(None);
    state.window.request_redraw();
}

/// Close the focused pane in the active tab. If the tab is a single
/// leaf, falls through to the regular tab-close flow.
pub(crate) fn close_focused_pane(state: &mut RunningState) {
    // Un-zoom first so the layout is consistent after the close.
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        tab.zoomed_pane = None;
    }
    let drop_pane = state
        .tabs
        .get_mut(state.active_tab)
        .and_then(TabState::close_focused);
    if drop_pane.is_some() {
        // A pane was collapsed up — resize everything to fit and
        // repaint.
        resize_active_tab_panes(state);
        state.renderer.set_selection(None);
        state.window.request_redraw();
    } else {
        // Tree was a single leaf — close the whole tab.
        crate::request_close_tab(state, state.active_tab);
    }
}

// ── active_tab_pane_rects / pick_adjacent_pane / focus_pane_in_direction ─────

/// Collect the physical-px rect of every leaf in the active tab into a flat
/// `Vec<(PaneId, rect)>`.
pub(crate) fn active_tab_pane_rects(state: &RunningState) -> Vec<(PaneId, (f32, f32, f32, f32))> {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return Vec::new();
    };
    let specs = pane_specs_for_tab(state, tab);
    specs.into_iter().map(|s| (s.pane_id, s.rect_px)).collect()
}

// ── pane-aware mouse hit-testing ──────────────────────────────────────────────

/// Pure inverse of the renderer's per-pane cell placement. The renderer puts
/// a pane's cell `(col, row)` at `(rect.x + pad_px + col·cw, rect.y + row·ch)`
/// (see `render_panes` / `queue_extra_pane` in `terminale-render`), so this
/// maps a physical-pixel position back to the cell, given the pane's grid
/// origin and cell metrics — all in physical px.
///
/// `clamp_leading = false` (strict): positions above/left of the first cell
/// return `None` — used for hit-testing "is the pointer on the grid?".
/// `clamp_leading = true`: those positions clamp to col/row 0 — used for
/// selection drags that stray outside the pane. Trailing overflow always
/// clamps to the last cell (xterm-style: clicks in the right/bottom padding
/// hit the nearest edge cell).
#[must_use]
pub(crate) fn cell_from_pane_origin(
    pos_px: (f32, f32),
    origin_px: (f32, f32),
    pad_px: f32,
    cell_w_px: f32,
    cell_h_px: f32,
    cols: u16,
    rows: u16,
    clamp_leading: bool,
) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 || cell_w_px <= 0.0 || cell_h_px <= 0.0 {
        return None;
    }
    let ux = pos_px.0 - origin_px.0 - pad_px;
    let uy = pos_px.1 - origin_px.1;
    if !clamp_leading && (ux < 0.0 || uy < 0.0) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let col = (ux.max(0.0) / cell_w_px).floor() as i64;
    #[allow(clippy::cast_possible_truncation)]
    let row = (uy.max(0.0) / cell_h_px).floor() as i64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((
        col.clamp(0, i64::from(cols) - 1) as u16,
        row.clamp(0, i64::from(rows) - 1) as u16,
    ))
}

/// Map a **physical-pixel** window position to the pane whose grid rect
/// contains it, returning the pane id and the **pane-local** cell.
///
/// This is the pane-aware replacement for the renderer's window-global
/// `cell_at_pixel`, which knows nothing about split layouts: with two or
/// more panes the window-global cell index matches no pane's grid, which
/// silently broke selection, link hover/click, and mouse reporting inside
/// splits. Positions over a pane header, the tab bar, a divider, or outside
/// every pane return `None`.
pub(crate) fn pane_cell_at_pixel(
    state: &RunningState,
    pos_px: (f32, f32),
) -> Option<(PaneId, u16, u16)> {
    let tab = state.tabs.get(state.active_tab)?;
    let scale = state.window.scale_factor() as f32;
    let cw = state.renderer.cell_width() * scale;
    let ch = state.renderer.cell_height() * scale;
    let pad = state.renderer.padding() * scale;
    for (id, (rx, ry, rw, rh)) in active_tab_pane_rects(state) {
        if pos_px.0 < rx || pos_px.1 < ry || pos_px.0 >= rx + rw || pos_px.1 >= ry + rh {
            continue;
        }
        let pane = tab.panes.get(&id)?;
        return cell_from_pane_origin(pos_px, (rx, ry), pad, cw, ch, pane.cols, pane.rows, false)
            .map(|(c, r)| (id, c, r));
    }
    None
}

/// Like [`pane_cell_at_pixel`] but **forgiving on the leading edges**: a
/// position inside a pane's rect that falls in the padding strip left of
/// column 0 (or above row 0) clamps to the first cell instead of missing.
///
/// This is the right hit-test for STARTING a selection: users habitually
/// press a few pixels left of the text they want and drag across. In a
/// split, that padding strip sits in the middle of the window (right next
/// to the divider), so the strict test silently swallowed those drags —
/// the press never armed and "selection didn't work".
pub(crate) fn pane_cell_at_pixel_clamped(
    state: &RunningState,
    pos_px: (f32, f32),
) -> Option<(PaneId, u16, u16)> {
    let tab = state.tabs.get(state.active_tab)?;
    let scale = state.window.scale_factor() as f32;
    let cw = state.renderer.cell_width() * scale;
    let ch = state.renderer.cell_height() * scale;
    let pad = state.renderer.padding() * scale;
    for (id, (rx, ry, rw, rh)) in active_tab_pane_rects(state) {
        if pos_px.0 < rx || pos_px.1 < ry || pos_px.0 >= rx + rw || pos_px.1 >= ry + rh {
            continue;
        }
        let pane = tab.panes.get(&id)?;
        return cell_from_pane_origin(pos_px, (rx, ry), pad, cw, ch, pane.cols, pane.rows, true)
            .map(|(c, r)| (id, c, r));
    }
    None
}

/// `true` when the pointer is over the **focused** pane's grid. Used to gate
/// chrome that the renderer can only draw in the focused pane's frame (e.g.
/// hover link underlines).
pub(crate) fn pointer_over_focused_pane(state: &RunningState, pos_px: (f32, f32)) -> bool {
    let focused = state
        .tabs
        .get(state.active_tab)
        .map(|t| t.focused)
        .unwrap_or_default();
    pane_cell_at_pixel(state, pos_px).is_some_and(|(id, _, _)| id == focused)
}

/// Like [`pane_cell_at_pixel`] but pinned to the **focused** pane and clamped
/// into its grid: positions outside the pane rect map to the nearest edge
/// cell. Selection drags and SGR mouse-drag reporting always target the
/// focused pane, even while the pointer strays over a neighbour pane or the
/// window chrome — same semantics as dragging outside a single-pane window.
pub(crate) fn focused_pane_cell_clamped(
    state: &RunningState,
    pos_px: (f32, f32),
) -> Option<(u16, u16)> {
    let tab = state.tabs.get(state.active_tab)?;
    let focused = tab.focused;
    let (_, (rx, ry, _, _)) = active_tab_pane_rects(state)
        .into_iter()
        .find(|(id, _)| *id == focused)?;
    let pane = tab.panes.get(&focused)?;
    let scale = state.window.scale_factor() as f32;
    let cw = state.renderer.cell_width() * scale;
    let ch = state.renderer.cell_height() * scale;
    let pad = state.renderer.padding() * scale;
    cell_from_pane_origin(pos_px, (rx, ry), pad, cw, ch, pane.cols, pane.rows, true)
}

/// Geometry helper for directional pane focus. Given the focused pane's rect
/// and a direction, pick the nearest other pane in that direction.
#[must_use]
pub(crate) fn pick_adjacent_pane(
    focused_rect: (f32, f32, f32, f32),
    candidates: &[(u32, (f32, f32, f32, f32))],
    direction: PaneDirection,
) -> Option<u32> {
    let (fx, fy, fw, fh) = focused_rect;
    // Trailing edges + perpendicular centre of the focused pane.
    let f_right = fx + fw;
    let f_bottom = fy + fh;
    let f_cx = fx + fw / 2.0;
    let f_cy = fy + fh / 2.0;

    // Overlap tolerance: two panes are "adjacent" in the perpendicular axis
    // when their extents overlap by at least this many pixels.
    const OVERLAP_TOL: f32 = 4.0;
    // Edge adjacency tolerance: how far (px) the candidate's leading edge may
    // be from the focused pane's trailing edge for us to consider them
    // "touching".
    const EDGE_TOL: f32 = 8.0;

    let mut best_id: Option<u32> = None;
    let mut best_dist = f32::MAX;

    for &(id, (cx, cy, cw, ch)) in candidates {
        let c_right = cx + cw;
        let c_bottom = cy + ch;
        let c_cx = cx + cw / 2.0;
        let c_cy = cy + ch / 2.0;

        let in_dir = match direction {
            PaneDirection::Right => {
                cx >= f_right - EDGE_TOL
                    && cx > fx
                    && c_bottom > fy + OVERLAP_TOL
                    && cy < f_bottom - OVERLAP_TOL
            }
            PaneDirection::Left => {
                c_right <= fx + EDGE_TOL
                    && c_right < f_right
                    && c_bottom > fy + OVERLAP_TOL
                    && cy < f_bottom - OVERLAP_TOL
            }
            PaneDirection::Down => {
                cy >= f_bottom - EDGE_TOL
                    && cy > fy
                    && c_right > fx + OVERLAP_TOL
                    && cx < f_right - OVERLAP_TOL
            }
            PaneDirection::Up => {
                c_bottom <= fy + EDGE_TOL
                    && c_bottom < f_bottom
                    && c_right > fx + OVERLAP_TOL
                    && cx < f_right - OVERLAP_TOL
            }
        };
        if !in_dir {
            continue;
        }
        // Primary key: primary-axis distance (closest edge first).
        // Secondary key: perpendicular-axis centre offset (prefer aligned).
        let primary_dist = match direction {
            PaneDirection::Right => cx - f_right,
            PaneDirection::Left => fx - c_right,
            PaneDirection::Down => cy - f_bottom,
            PaneDirection::Up => fy - c_bottom,
        }
        .max(0.0);
        let perp_dist = match direction {
            PaneDirection::Left | PaneDirection::Right => (c_cy - f_cy).abs(),
            PaneDirection::Up | PaneDirection::Down => (c_cx - f_cx).abs(),
        };
        // Combined score: primary axis dominates, perpendicular breaks ties.
        let score = primary_dist * 10_000.0 + perp_dist;
        if score < best_dist {
            best_dist = score;
            best_id = Some(id);
        }
    }
    best_id
}

/// Move focus to the nearest pane in `direction` relative to the focused pane.
pub(crate) fn focus_pane_in_direction(state: &mut RunningState, direction: PaneDirection) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    // If pane is zoomed, un-zoom first before navigating (or just navigate
    // within the virtual single-pane — which is a no-op since there's only
    // one visible). We leave zoom active and return early: the user must
    // toggle zoom off first.
    if tab.zoomed_pane.is_some() {
        return;
    }
    let focused_id = tab.focused;
    let rects = active_tab_pane_rects(state);
    let Some(&(_, focused_rect)) = rects.iter().find(|(id, _)| *id == focused_id) else {
        return;
    };
    // Candidates: all panes except the focused one.
    let candidates: Vec<(u32, (f32, f32, f32, f32))> = rects
        .into_iter()
        .filter(|(id, _)| *id != focused_id)
        .collect();
    let Some(target_id) = pick_adjacent_pane(focused_rect, &candidates, direction) else {
        return;
    };
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        if tab.focused != target_id {
            tab.focused = target_id;
            state.pending_hook_pane_focus.push(target_id);
            state.window.request_redraw();
        }
    }
}

// ── swap_leaves / rotate_panes / rotate_panes_back ──────────────────────────

/// Collect the leaf pane-ids from `node` in depth-first (left-to-right,
/// top-to-bottom) order into `out`.
pub(crate) fn collect_leaves(node: &PaneNode, out: &mut Vec<PaneId>) {
    match node {
        PaneNode::Leaf(id) => out.push(*id),
        PaneNode::Split { a, b, .. } => {
            collect_leaves(a, out);
            collect_leaves(b, out);
        }
    }
}

/// Replace every leaf `PaneNode::Leaf(id)` in `node` whose id equals
/// `old` with `new`. Structural walk; the tree shape is unchanged.
fn replace_leaf_id(node: &mut PaneNode, old: PaneId, new: PaneId) {
    match node {
        PaneNode::Leaf(id) => {
            if *id == old {
                *id = new;
            }
        }
        PaneNode::Split { a, b, .. } => {
            replace_leaf_id(a, old, new);
            replace_leaf_id(b, old, new);
        }
    }
}

/// Swap the positions of two leaf pane-ids `a` and `b` in `tree`.
/// The tree structure (split directions / ratios) is preserved; only
/// the leaf values are exchanged. Safe when either id is absent —
/// the absent side is left unchanged.
pub(crate) fn swap_leaves(tree: &mut PaneNode, a: PaneId, b: PaneId) {
    if a == b {
        return;
    }
    // Use a sentinel that is guaranteed to be absent from the tree.
    // We use u32::MAX as a temporary placeholder.
    const SENTINEL: PaneId = u32::MAX;
    // a → sentinel, b → a, sentinel → b.
    replace_leaf_id(tree, a, SENTINEL);
    replace_leaf_id(tree, b, a);
    replace_leaf_id(tree, SENTINEL, b);
}

/// Rotate the leaf pane-ids of `tree` forward by one step.
///
/// Leaf order is defined by the depth-first traversal used in
/// [`collect_leaves`].  The id at position 0 moves to the last
/// position, every other id shifts one index forward (position 1→0,
/// 2→1, …). This keeps the split tree shape identical and only
/// reassigns which physical pane sits in each slot.
///
/// A tree with a single leaf is a no-op.
pub(crate) fn rotate_panes(tree: &mut PaneNode) {
    let mut leaves = Vec::new();
    collect_leaves(tree, &mut leaves);
    if leaves.len() < 2 {
        return;
    }
    // Forward rotation: the id in slot 0 moves to the last slot.
    // In terms of swaps on the tree: swap slot[0] with slot[1], then
    // slot[1] with slot[2], …, until slot[n-2] with slot[n-1].
    // After these (n-1) adjacent swaps the original slot[0] id has
    // bubbled all the way to slot[n-1].
    //
    // We track the "current id in slot i" using a mirror array so we
    // can always call swap_leaves with the correct ids.
    let n = leaves.len();
    let mut cur = leaves.clone();
    for i in 0..n - 1 {
        if cur[i] != cur[i + 1] {
            swap_leaves(tree, cur[i], cur[i + 1]);
        }
        cur.swap(i, i + 1);
    }
}

/// Rotate the leaf pane-ids of `tree` backward by one step (inverse of
/// [`rotate_panes`]).  The id at the last position moves to position 0,
/// every other id shifts one index backward.
pub(crate) fn rotate_panes_back(tree: &mut PaneNode) {
    let mut leaves = Vec::new();
    collect_leaves(tree, &mut leaves);
    if leaves.len() < 2 {
        return;
    }
    // Backward rotation: the id in slot[n-1] moves to slot[0].
    // In terms of swaps: swap slot[n-1] with slot[n-2], then
    // slot[n-2] with slot[n-3], …, until slot[1] with slot[0].
    let n = leaves.len();
    let mut cur = leaves.clone();
    for i in (1..n).rev() {
        if cur[i] != cur[i - 1] {
            swap_leaves(tree, cur[i], cur[i - 1]);
        }
        cur.swap(i, i - 1);
    }
}

// ── move_pane_in_direction / rotate_active_tab_panes ─────────────────────────

/// Swap the focused pane with its neighbour in `direction`.
///
/// Reuses [`pick_adjacent_pane`] to locate the nearest neighbour, then
/// calls [`swap_leaves`] to exchange their positions in the tree.
/// Focus follows the moved pane (i.e. the focused id is unchanged —
/// the id itself moves to the other slot). Resizes after.
pub(crate) fn move_pane_in_direction(state: &mut RunningState, direction: PaneDirection) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    if tab.zoomed_pane.is_some() {
        return;
    }
    let focused_id = tab.focused;
    let rects = active_tab_pane_rects(state);
    let Some(&(_, focused_rect)) = rects.iter().find(|(id, _)| *id == focused_id) else {
        return;
    };
    let candidates: Vec<(u32, (f32, f32, f32, f32))> = rects
        .into_iter()
        .filter(|(id, _)| *id != focused_id)
        .collect();
    let Some(neighbour_id) = pick_adjacent_pane(focused_rect, &candidates, direction) else {
        return;
    };
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        swap_leaves(&mut tab.tree, focused_id, neighbour_id);
        // Focus stays with the moved pane — the focused_id hasn't changed;
        // it just lives in the slot previously occupied by neighbour_id.
    }
    resize_active_tab_panes(state);
    state.window.request_redraw();
}

/// Rotate all pane-ids in the active tab's tree forward by one step.
/// Focus follows the focused pane (the focused_id is unchanged).
pub(crate) fn rotate_active_tab_panes(state: &mut RunningState) {
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        if tab.zoomed_pane.is_some() {
            return;
        }
        rotate_panes(&mut tab.tree);
    }
    resize_active_tab_panes(state);
    state.window.request_redraw();
}

/// Rotate all pane-ids in the active tab's tree backward by one step.
pub(crate) fn rotate_active_tab_panes_back(state: &mut RunningState) {
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        if tab.zoomed_pane.is_some() {
            return;
        }
        rotate_panes_back(&mut tab.tree);
    }
    resize_active_tab_panes(state);
    state.window.request_redraw();
}

/// Toggle zoom on the focused pane in the active tab.
pub(crate) fn toggle_pane_zoom(state: &mut RunningState) {
    let Some(tab) = state.tabs.get_mut(state.active_tab) else {
        return;
    };
    // Single-pane tab: nothing to zoom / unzoom.
    if matches!(tab.tree, PaneNode::Leaf(_)) {
        return;
    }
    if tab.zoomed_pane.is_some() {
        // Un-zoom: restore the normal tree layout.
        tab.zoomed_pane = None;
    } else {
        // Zoom: record the currently-focused pane.
        tab.zoomed_pane = Some(tab.focused);
    }
    // Resize all panes so their PTY/emulator dimensions match their new
    // visible extents.
    resize_active_tab_panes(state);
    state.renderer.set_selection(None);
    state.window.request_redraw();
}

// ── keyboard_resize_pane / find_resize_split ─────────────────────────────────

/// Nudge the split that contains the focused pane in the given direction.
pub(crate) fn keyboard_resize_pane(state: &mut RunningState, direction: PaneDirection) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    // No-op while zoomed — tree layout is invisible.
    if tab.zoomed_pane.is_some() {
        return;
    }
    let focused_id = tab.focused;
    let step = state.pane_resize_step_cells;
    let scale = state.window.scale_factor() as f32;
    let cell_w = state.renderer.cell_width() * scale;
    let cell_h = state.renderer.cell_height() * scale;

    // Walk the tree to find the innermost split that can resize in `direction`.
    let result = find_resize_split(&tab.tree, focused_id, direction);
    let Some(ResizeSplitResult {
        path: Some(path),
        focused_in_a,
    }) = result
    else {
        return;
    };

    // Compute the parent rect to convert a cell-step to a ratio delta.
    let surface = state.window.inner_size();
    let top_pad = state.renderer.body_top_px();
    let bottom_px = state.renderer.body_bottom_px(surface.height);
    let body = (
        0.0_f32,
        top_pad,
        surface.width as f32,
        (bottom_px - top_pad).max(0.0),
    );
    let Some(tab2) = state.tabs.get(state.active_tab) else {
        return;
    };
    let Some((_, _, pw, ph)) = walk_to_node_rect(&tab2.tree, body, &path) else {
        return;
    };
    let Some(current_ratio) = split_ratio_at(&tab2.tree, &path) else {
        return;
    };
    let split_dir_opt = {
        let mut node = &tab2.tree;
        for &step in &path {
            match node {
                PaneNode::Leaf(_) => {
                    node = &tab2.tree;
                    break;
                }
                PaneNode::Split { a, b, .. } => {
                    node = if step { b } else { a };
                }
            }
        }
        if let PaneNode::Split { direction, .. } = node {
            Some(*direction)
        } else {
            None
        }
    };
    let Some(split_dir) = split_dir_opt else {
        return;
    };

    // Convert step cells to a ratio delta.
    let delta = match (split_dir, direction) {
        // Vertical split (left|right children): resize in x.
        (SplitDir::Vertical, PaneDirection::Right) | (SplitDir::Vertical, PaneDirection::Left) => {
            if pw <= 0.0 {
                return;
            }
            let cell_ratio = cell_w / pw;
            let sign: f32 = match direction {
                PaneDirection::Right => {
                    if focused_in_a {
                        1.0
                    } else {
                        -1.0
                    }
                }
                PaneDirection::Left => {
                    if focused_in_a {
                        -1.0
                    } else {
                        1.0
                    }
                }
                _ => return,
            };
            sign * cell_ratio * f32::from(step)
        }
        // Horizontal split (top|bottom children): resize in y.
        (SplitDir::Horizontal, PaneDirection::Down) | (SplitDir::Horizontal, PaneDirection::Up) => {
            if ph <= 0.0 {
                return;
            }
            let cell_ratio = cell_h / ph;
            let sign: f32 = match direction {
                PaneDirection::Down => {
                    if focused_in_a {
                        1.0
                    } else {
                        -1.0
                    }
                }
                PaneDirection::Up => {
                    if focused_in_a {
                        -1.0
                    } else {
                        1.0
                    }
                }
                _ => return,
            };
            sign * cell_ratio * f32::from(step)
        }
        // The split's axis is perpendicular to `direction` — nothing to do.
        _ => return,
    };

    let new_ratio = (current_ratio + delta).clamp(0.05, 0.95);
    if let Some(tab_mut) = state.tabs.get_mut(state.active_tab) {
        set_split_ratio_at(&mut tab_mut.tree, &path, new_ratio);
    }
    if state.live_pane_resize {
        resize_active_tab_panes(state);
    }
    state.window.request_redraw();
}

/// Return the innermost `Split` node that (a) contains the focused leaf
/// `target` and (b) has an orientation matching `direction`.
pub(crate) fn find_resize_split(
    root: &PaneNode,
    target: PaneId,
    direction: PaneDirection,
) -> Option<ResizeSplitResult> {
    let wanted = match direction {
        PaneDirection::Left | PaneDirection::Right => SplitDir::Vertical,
        PaneDirection::Up | PaneDirection::Down => SplitDir::Horizontal,
    };
    let mut path: DividerPath = Vec::new();
    let r = find_resize_split_inner(root, target, wanted, &mut path)?;
    // `r.path == None` means the target leaf was found but no split of the
    // correct orientation was encountered on the way up.
    let found_path = r.path?;
    Some(ResizeSplitResult {
        path: Some(found_path),
        focused_in_a: r.focused_in_a,
    })
}

/// Internal recursive walker for [`find_resize_split`].
pub(crate) fn find_resize_split_inner(
    node: &PaneNode,
    target: PaneId,
    wanted: SplitDir,
    path: &mut DividerPath,
) -> Option<ResizeSplitResult> {
    match node {
        PaneNode::Leaf(id) => {
            // Signal "found target" with path=None (no matching split yet).
            if *id == target {
                Some(ResizeSplitResult {
                    path: None,
                    focused_in_a: true,
                })
            } else {
                None
            }
        }
        PaneNode::Split {
            direction, a, b, ..
        } => {
            // Descend into `a`.
            path.push(false);
            let in_a = find_resize_split_inner(a, target, wanted, path);
            path.pop();

            if let Some(mut r) = in_a {
                // Target found under `a`. If no inner split has claimed it yet
                // and this node's orientation matches, claim it now.
                if r.path.is_none() && *direction == wanted {
                    r.path = Some(path.clone());
                    r.focused_in_a = true;
                }
                return Some(r);
            }

            // Descend into `b`.
            path.push(true);
            let in_b = find_resize_split_inner(b, target, wanted, path);
            path.pop();

            if let Some(mut r) = in_b {
                if r.path.is_none() && *direction == wanted {
                    r.path = Some(path.clone());
                    r.focused_in_a = false;
                }
                return Some(r);
            }

            None
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── graft_in / remap_leaf_ids / drop_side_for ─────────────────────────────
    // (uses the `leaf` / `leaf_ids` helpers defined at the bottom of this module)

    #[test]
    fn graft_subtree_on_b_side() {
        // Tab with panes 0|1; graft a subtree (2/3) to the RIGHT of pane 1.
        let tree = split_in(leaf(0), 0, SplitDir::Vertical, 1, true);
        let subtree = split_in(leaf(2), 2, SplitDir::Horizontal, 3, true);
        let grafted = graft_in(tree, 1, SplitDir::Vertical, subtree, true);
        // Depth-first leaf order: 0, then 1, then the subtree 2/3.
        assert_eq!(leaf_ids(&grafted), vec![0, 1, 2, 3]);
        assert_eq!(count_leaves(&grafted), 4);
    }

    #[test]
    fn graft_subtree_on_a_side_puts_it_first() {
        let tree = leaf(0);
        let grafted = graft_in(tree, 0, SplitDir::Horizontal, leaf(7), false);
        // side_b = false → the subtree lands on the a (top/left) side.
        assert_eq!(leaf_ids(&grafted), vec![7, 0]);
    }

    #[test]
    fn graft_missing_target_leaves_tree_unchanged() {
        let tree = split_in(leaf(0), 0, SplitDir::Vertical, 1, true);
        let grafted = graft_in(tree, 99, SplitDir::Vertical, leaf(5), true);
        assert_eq!(leaf_ids(&grafted), vec![0, 1]);
    }

    #[test]
    fn split_in_still_grafts_single_leaf() {
        // split_in delegates to graft_in — behaviour unchanged.
        let tree = split_in(leaf(0), 0, SplitDir::Horizontal, 1, false);
        assert_eq!(leaf_ids(&tree), vec![1, 0]);
    }

    #[test]
    fn remap_rewrites_every_leaf() {
        let tree = split_in(leaf(0), 0, SplitDir::Vertical, 1, true);
        let map: std::collections::HashMap<PaneId, PaneId> =
            [(0, 10), (1, 11)].into_iter().collect();
        assert_eq!(leaf_ids(&remap_leaf_ids(tree, &map)), vec![10, 11]);
    }

    #[test]
    fn drop_side_follows_dominant_axis() {
        let r = (0.0, 0.0, 100.0, 100.0);
        assert_eq!(drop_side_for(r, 10.0, 50.0), DropSide::Left);
        assert_eq!(drop_side_for(r, 90.0, 50.0), DropSide::Right);
        assert_eq!(drop_side_for(r, 50.0, 10.0), DropSide::Top);
        assert_eq!(drop_side_for(r, 50.0, 90.0), DropSide::Bottom);
        // Wide rect: a point near the top still reads Top when the
        // normalised vertical offset dominates.
        let wide = (0.0, 0.0, 400.0, 100.0);
        assert_eq!(drop_side_for(wide, 200.0, 5.0), DropSide::Top);
    }

    #[test]
    fn drop_side_split_mapping_matches_shortcuts() {
        // Mirrors the SplitRight/SplitDown/SplitLeft/SplitUp actions'
        // (direction, side_b) pairs in shortcuts.rs.
        assert_eq!(DropSide::Right.split(), (SplitDir::Vertical, true));
        assert_eq!(DropSide::Left.split(), (SplitDir::Vertical, false));
        assert_eq!(DropSide::Bottom.split(), (SplitDir::Horizontal, true));
        assert_eq!(DropSide::Top.split(), (SplitDir::Horizontal, false));
    }

    #[test]
    fn half_rect_covers_the_named_half() {
        let r = (10.0, 20.0, 100.0, 60.0);
        assert_eq!(DropSide::Left.half_rect(r), (10.0, 20.0, 50.0, 60.0));
        assert_eq!(DropSide::Right.half_rect(r), (60.0, 20.0, 50.0, 60.0));
        assert_eq!(DropSide::Top.half_rect(r), (10.0, 20.0, 100.0, 30.0));
        assert_eq!(DropSide::Bottom.half_rect(r), (10.0, 50.0, 100.0, 30.0));
    }

    // ── cell_from_pane_origin ─────────────────────────────────────────────────

    /// 10×20 px cells, 4 px padding, pane origin at (100, 50), 80×24 grid.
    fn map(pos: (f32, f32), clamp: bool) -> Option<(u16, u16)> {
        cell_from_pane_origin(pos, (100.0, 50.0), 4.0, 10.0, 20.0, 80, 24, clamp)
    }

    #[test]
    fn cell_mapping_first_cell_at_pane_origin() {
        // First cell starts at origin.x + pad: (104, 50).
        assert_eq!(map((104.0, 50.0), false), Some((0, 0)));
        // Just inside cell (1, 1).
        assert_eq!(map((114.5, 70.5), false), Some((1, 1)));
    }

    #[test]
    fn cell_mapping_strict_rejects_leading_positions() {
        // Left of the first cell (inside padding) and above the pane.
        assert_eq!(map((101.0, 60.0), false), None);
        assert_eq!(map((110.0, 49.0), false), None);
    }

    #[test]
    fn cell_mapping_clamped_pins_leading_positions_to_zero() {
        assert_eq!(map((0.0, 0.0), true), Some((0, 0)));
        assert_eq!(map((101.0, 60.0), true), Some((0, 0)));
    }

    #[test]
    fn cell_mapping_trailing_overflow_clamps_to_last_cell() {
        // Way past the grid's right/bottom edge → last cell, both modes.
        assert_eq!(map((10_000.0, 10_000.0), false), Some((79, 23)));
        assert_eq!(map((10_000.0, 10_000.0), true), Some((79, 23)));
    }

    #[test]
    fn cell_mapping_rejects_degenerate_grids() {
        assert_eq!(
            cell_from_pane_origin((104.0, 50.0), (100.0, 50.0), 4.0, 10.0, 20.0, 0, 24, true),
            None
        );
        assert_eq!(
            cell_from_pane_origin((104.0, 50.0), (100.0, 50.0), 4.0, 0.0, 20.0, 80, 24, true),
            None
        );
    }

    /// Regression for split-view hit testing: a position inside a RIGHT-half
    /// pane must map to that pane's local columns, not the window-global
    /// ones. With a 200 px-wide left pane, the right pane's origin is at
    /// x=200 — a click at x=210 is its column 0 (was ~column 20 of a
    /// window-global grid before the fix, hitting nothing).
    #[test]
    fn cell_mapping_right_pane_is_pane_local() {
        let cell = cell_from_pane_origin(
            (210.0, 55.0),
            (200.0, 50.0), // right pane's rect origin
            4.0,
            10.0,
            20.0,
            20,
            24,
            false,
        );
        assert_eq!(cell, Some((0, 0)));
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build `Leaf(id)`.
    fn leaf(id: PaneId) -> PaneNode {
        PaneNode::Leaf(id)
    }

    /// Build a vertical split of `a` and `b` with ratio 0.5.
    fn vsplit(a: PaneNode, b: PaneNode) -> PaneNode {
        PaneNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Build a horizontal split of `a` and `b` with ratio 0.5.
    fn hsplit(a: PaneNode, b: PaneNode) -> PaneNode {
        PaneNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Return the leaf ids in depth-first order.
    fn leaf_ids(node: &PaneNode) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect_leaves(node, &mut out);
        out
    }

    // ── collect_leaves ────────────────────────────────────────────────────────

    #[test]
    fn collect_leaves_single() {
        let tree = leaf(1);
        assert_eq!(leaf_ids(&tree), vec![1]);
    }

    #[test]
    fn collect_leaves_two_pane_split() {
        let tree = vsplit(leaf(1), leaf(2));
        assert_eq!(leaf_ids(&tree), vec![1, 2]);
    }

    #[test]
    fn collect_leaves_three_pane_tree() {
        // vsplit(1, vsplit(2, 3)) → [1, 2, 3]
        let tree = vsplit(leaf(1), vsplit(leaf(2), leaf(3)));
        assert_eq!(leaf_ids(&tree), vec![1, 2, 3]);
    }

    // ── swap_leaves ───────────────────────────────────────────────────────────

    #[test]
    fn swap_leaves_two_panes() {
        let mut tree = vsplit(leaf(1), leaf(2));
        swap_leaves(&mut tree, 1, 2);
        assert_eq!(leaf_ids(&tree), vec![2, 1]);
    }

    #[test]
    fn swap_leaves_same_id_is_noop() {
        let mut tree = vsplit(leaf(1), leaf(2));
        swap_leaves(&mut tree, 1, 1);
        assert_eq!(leaf_ids(&tree), vec![1, 2]);
    }

    #[test]
    fn swap_leaves_absent_id_is_safe() {
        let mut tree = vsplit(leaf(1), leaf(2));
        // 99 is not in the tree — only the present side changes
        swap_leaves(&mut tree, 1, 99);
        // Neither id is fully in the tree so nothing meaningful changes.
        // The important thing is that no panic occurs and the tree remains valid.
        let ids = leaf_ids(&tree);
        assert_eq!(ids.len(), 2, "leaf count must be unchanged");
    }

    #[test]
    fn swap_leaves_three_panes_a_and_c() {
        // vsplit(1, vsplit(2, 3)) → swap 1 ↔ 3 → vsplit(3, vsplit(2, 1))
        let mut tree = vsplit(leaf(1), vsplit(leaf(2), leaf(3)));
        swap_leaves(&mut tree, 1, 3);
        assert_eq!(leaf_ids(&tree), vec![3, 2, 1]);
    }

    #[test]
    fn swap_leaves_preserves_tree_shape() {
        // Verify that the split node count is unchanged after a swap.
        let mut tree = vsplit(leaf(1), hsplit(leaf(2), leaf(3)));
        swap_leaves(&mut tree, 1, 2);
        // Shape: vsplit(2, hsplit(1, 3)) — still two splits.
        assert_eq!(count_leaves(&tree), 3);
        assert_eq!(leaf_ids(&tree), vec![2, 1, 3]);
    }

    // ── rotate_panes ──────────────────────────────────────────────────────────

    #[test]
    fn rotate_panes_single_leaf_is_noop() {
        let mut tree = leaf(1);
        rotate_panes(&mut tree);
        assert_eq!(leaf_ids(&tree), vec![1]);
    }

    #[test]
    fn rotate_panes_two_panes() {
        let mut tree = vsplit(leaf(1), leaf(2));
        rotate_panes(&mut tree);
        // 1→back, 2→front: [2, 1]
        assert_eq!(leaf_ids(&tree), vec![2, 1]);
    }

    #[test]
    fn rotate_panes_three_panes_forward() {
        let mut tree = vsplit(leaf(1), vsplit(leaf(2), leaf(3)));
        rotate_panes(&mut tree);
        // [1, 2, 3] → [2, 3, 1]
        assert_eq!(leaf_ids(&tree), vec![2, 3, 1]);
    }

    #[test]
    fn rotate_panes_three_full_cycle_returns_to_original() {
        let mut tree = vsplit(leaf(1), vsplit(leaf(2), leaf(3)));
        for _ in 0..3 {
            rotate_panes(&mut tree);
        }
        // After 3 rotations of 3 panes we return to the original order.
        assert_eq!(leaf_ids(&tree), vec![1, 2, 3]);
    }

    #[test]
    fn rotate_panes_preserves_leaf_count() {
        let mut tree = vsplit(leaf(1), hsplit(leaf(2), leaf(3)));
        rotate_panes(&mut tree);
        assert_eq!(count_leaves(&tree), 3);
    }

    // ── rotate_panes_back ─────────────────────────────────────────────────────

    #[test]
    fn rotate_panes_back_two_panes() {
        let mut tree = vsplit(leaf(1), leaf(2));
        rotate_panes_back(&mut tree);
        // Backward rotation of [1, 2] → [2, 1]
        assert_eq!(leaf_ids(&tree), vec![2, 1]);
    }

    #[test]
    fn rotate_panes_back_three_panes() {
        let mut tree = vsplit(leaf(1), vsplit(leaf(2), leaf(3)));
        rotate_panes_back(&mut tree);
        // [1, 2, 3] → [3, 1, 2]
        assert_eq!(leaf_ids(&tree), vec![3, 1, 2]);
    }

    #[test]
    fn rotate_forward_and_back_are_inverses() {
        let mut tree = vsplit(leaf(1), vsplit(leaf(2), leaf(3)));
        let original = leaf_ids(&tree);
        rotate_panes(&mut tree);
        rotate_panes_back(&mut tree);
        assert_eq!(
            leaf_ids(&tree),
            original,
            "forward then back must restore original order"
        );
    }

    // ── pick_adjacent_pane (direction logic) ─────────────────────────────────

    /// Build a minimal (PaneId, rect) candidate list from a horizontal layout:
    /// pane 1 on the left [0,0,100,100], pane 2 on the right [100,0,100,100].
    fn two_pane_horizontal_layout() -> Vec<(PaneId, (f32, f32, f32, f32))> {
        vec![
            (1, (0.0, 0.0, 100.0, 100.0)),
            (2, (100.0, 0.0, 100.0, 100.0)),
        ]
    }

    #[test]
    fn pick_adjacent_pane_right_finds_neighbour() {
        let layout = two_pane_horizontal_layout();
        let focused_rect = (0.0, 0.0, 100.0, 100.0);
        let candidates: Vec<_> = layout.iter().filter(|(id, _)| *id != 1).copied().collect();
        assert_eq!(
            pick_adjacent_pane(focused_rect, &candidates, PaneDirection::Right),
            Some(2),
        );
    }

    #[test]
    fn pick_adjacent_pane_left_finds_neighbour() {
        let layout = two_pane_horizontal_layout();
        let focused_rect = (100.0, 0.0, 100.0, 100.0);
        let candidates: Vec<_> = layout.iter().filter(|(id, _)| *id != 2).copied().collect();
        assert_eq!(
            pick_adjacent_pane(focused_rect, &candidates, PaneDirection::Left),
            Some(1),
        );
    }

    #[test]
    fn pick_adjacent_pane_no_neighbour_in_opposite_direction() {
        let layout = two_pane_horizontal_layout();
        let focused_rect = (0.0, 0.0, 100.0, 100.0);
        let candidates: Vec<_> = layout.iter().filter(|(id, _)| *id != 1).copied().collect();
        // Pane 1 is on the left — there is no neighbour to its left.
        assert_eq!(
            pick_adjacent_pane(focused_rect, &candidates, PaneDirection::Left),
            None,
        );
    }

    // ── swap_leaves invariant: correct pane ends in correct slot ─────────────

    #[test]
    fn swap_leaves_moves_correct_pane_to_correct_position() {
        // Tree: vsplit(hsplit(1, 2), 3)
        // After swap(2, 3): vsplit(hsplit(1, 3), 2)
        let mut tree = vsplit(hsplit(leaf(1), leaf(2)), leaf(3));
        swap_leaves(&mut tree, 2, 3);
        assert_eq!(leaf_ids(&tree), vec![1, 3, 2]);
    }
}
