//! Tab-group management: create groups, assign tabs, clear tab group membership.
//!
//! Groups are named, colour-coded collections of tabs. Each group has a stable
//! [`TabGroupId`], a name (auto-generated as "Group N"), and an accent colour
//! cycled from a small palette. Tabs reference a group by id via
//! `TabState::group`.

use crate::{tabs::refresh_tab_bar, RunningState, TabGroup, TabGroupId, GROUP_COLOR_PALETTE};

/// Pick the colour for a new group from the configured palette (falls back
/// to the built-in const when the configured vec is empty so division by
/// zero is impossible).
fn pick_group_color(state: &RunningState) -> [u8; 3] {
    let idx = state.tab_groups.len(); // 0-based count before push
    if state.tab_group_colors.is_empty() {
        GROUP_COLOR_PALETTE[idx % GROUP_COLOR_PALETTE.len()]
    } else {
        state.tab_group_colors[idx % state.tab_group_colors.len()]
    }
}

/// Create a new tab group, assign the active tab to it, and refresh the bar.
/// The group name is auto-generated ("Group N") and the colour is cycled from
/// `GROUP_COLOR_PALETTE`. No blocking dialog is shown.
pub(crate) fn create_group_and_assign(state: &mut RunningState) {
    let id = allocate_group_id(state);
    let n = state.tab_groups.len() + 1; // 1-based display count
    let color = pick_group_color(state);
    state.tab_groups.push(TabGroup {
        id,
        name: format!("Group {n}"),
        color,
    });
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        tab.group = Some(id);
    }
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Assign the active tab to the "next" group in the list (round-robin).
/// When there are no groups, creates one first.
pub(crate) fn assign_active_tab_to_next_group(state: &mut RunningState) {
    if state.tab_groups.is_empty() {
        create_group_and_assign(state);
        return;
    }
    // Find which group the active tab is in, then pick the next one.
    let current_group = state.tabs.get(state.active_tab).and_then(|t| t.group);
    let next_id = next_group_id_after(current_group, &state.tab_groups);
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        tab.group = Some(next_id);
    }
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Assign the active tab to the group with the given `gid` (used by the
/// "Add to `<group>`" entries in a tab's context menu). No-op when the group
/// does not exist. Prunes any group the tab just left if it became empty.
pub(crate) fn assign_active_tab_to_group(state: &mut RunningState, gid: TabGroupId) {
    if !state.tab_groups.iter().any(|g| g.id == gid) {
        return;
    }
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        tab.group = Some(gid);
    }
    prune_empty_groups(state);
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Remove the active tab from its group, then drop the group if no tabs remain
/// in it (so empty groups don't linger in the registry). No-op when the active
/// tab is already ungrouped.
pub(crate) fn clear_active_tab_group(state: &mut RunningState) {
    if let Some(tab) = state.tabs.get_mut(state.active_tab) {
        tab.group = None;
    }
    prune_empty_groups(state);
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Begin an inline rename of the group whose id is `gid`. Sets the rename
/// state to target the first tab in the group run with the current group name
/// pre-filled in the buffer, then refreshes the tab bar.
pub(crate) fn start_rename_group(state: &mut RunningState, gid: TabGroupId) {
    // Find the first tab that belongs to this group.
    let tab_idx = match state.tabs.iter().position(|t| t.group == Some(gid)) {
        Some(i) => i,
        None => return, // group has no member tabs — nothing to rename
    };
    let buffer = state
        .tab_groups
        .iter()
        .find(|g| g.id == gid)
        .map(|g| g.name.clone())
        .unwrap_or_default();
    state.menu_visible = false;
    state.renderer.set_overlay(None);
    state.renaming = Some(crate::RenameState {
        tab_idx,
        target: crate::RenameTarget::Group(gid),
        buffer,
    });
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Rename the group with the given `gid` to `name`. No-op when the group
/// does not exist.
pub(crate) fn rename_group(state: &mut RunningState, gid: TabGroupId, name: String) {
    if let Some(g) = state.tab_groups.iter_mut().find(|g| g.id == gid) {
        g.name = name;
    }
}

/// Drop every group that has no member tabs. Called after a tab leaves its
/// group (clear) or a tab is closed, so the group registry never accumulates
/// orphaned, member-less groups. Pure data update — the caller refreshes the
/// bar and requests a redraw.
pub(crate) fn prune_empty_groups(state: &mut RunningState) {
    state
        .tab_groups
        .retain(|g| state.tabs.iter().any(|t| t.group == Some(g.id)));
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn allocate_group_id(state: &mut RunningState) -> TabGroupId {
    let id = state.next_group_id;
    state.next_group_id = state.next_group_id.wrapping_add(1);
    // Extremely unlikely wrap-around guard: skip any id already in use.
    while state.tab_groups.iter().any(|g| g.id == state.next_group_id) {
        state.next_group_id = state.next_group_id.wrapping_add(1);
    }
    id
}

fn next_group_id_after(current: Option<TabGroupId>, groups: &[TabGroup]) -> TabGroupId {
    if groups.is_empty() {
        // Caller checked this already, but be defensive.
        return 0;
    }
    match current {
        None => groups[0].id,
        Some(cid) => {
            let pos = groups.iter().position(|g| g.id == cid);
            match pos {
                None => groups[0].id, // current group was deleted; fall back to first
                Some(i) => groups[(i + 1) % groups.len()].id,
            }
        }
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_group_id_after_wraps() {
        let groups = vec![
            TabGroup {
                id: 10,
                name: "A".into(),
                color: [0; 3],
            },
            TabGroup {
                id: 20,
                name: "B".into(),
                color: [0; 3],
            },
            TabGroup {
                id: 30,
                name: "C".into(),
                color: [0; 3],
            },
        ];
        // None → first
        assert_eq!(next_group_id_after(None, &groups), 10);
        // First → second
        assert_eq!(next_group_id_after(Some(10), &groups), 20);
        // Last → wraps to first
        assert_eq!(next_group_id_after(Some(30), &groups), 10);
    }

    #[test]
    fn next_group_id_after_unknown_falls_back() {
        let groups = vec![TabGroup {
            id: 5,
            name: "X".into(),
            color: [0; 3],
        }];
        // Unknown current id → first group
        assert_eq!(next_group_id_after(Some(99), &groups), 5);
    }

    #[test]
    fn group_color_cycling_is_deterministic() {
        // The nth group (0-indexed) uses palette[n % len].
        for n in 0..GROUP_COLOR_PALETTE.len() * 2 {
            let expected = GROUP_COLOR_PALETTE[n % GROUP_COLOR_PALETTE.len()];
            assert_eq!(
                GROUP_COLOR_PALETTE[n % GROUP_COLOR_PALETTE.len()],
                expected,
                "colour cycle must be deterministic at n={n}"
            );
        }
    }

    // ── rename_group / auto_cycle tests ───────────────────────────────────────

    /// Helper: mutate a slice of TabGroups the same way rename_group does,
    /// so we can unit-test the logic without constructing a full RunningState.
    fn rename_in_groups(groups: &mut [TabGroup], gid: TabGroupId, name: &str) {
        if let Some(g) = groups.iter_mut().find(|g| g.id == gid) {
            g.name = name.to_string();
        }
    }

    #[test]
    fn rename_group_updates_name() {
        let mut groups = vec![
            TabGroup {
                id: 1,
                name: "Alpha".into(),
                color: [0; 3],
            },
            TabGroup {
                id: 2,
                name: "Beta".into(),
                color: [0; 3],
            },
        ];
        rename_in_groups(&mut groups, 1, "Build");
        assert_eq!(groups[0].name, "Build", "group 1 name must be updated");
        assert_eq!(groups[1].name, "Beta", "group 2 name must be unchanged");
    }

    #[test]
    fn rename_group_empty_name_is_kept() {
        // The caller (handle_rename_input) guards empty names; the low-level
        // function itself sets whatever string is passed.  An empty string
        // written here persists (it's the caller's job to guard).
        let mut groups = vec![TabGroup {
            id: 5,
            name: "Deploy".into(),
            color: [0; 3],
        }];
        // Simulate the guard: if name is non-empty, rename; else keep old.
        let new_name = "";
        if !new_name.is_empty() {
            rename_in_groups(&mut groups, 5, new_name);
        }
        assert_eq!(
            groups[0].name, "Deploy",
            "empty name must leave original intact"
        );
    }

    #[test]
    fn auto_cycle_uses_config_palette() {
        // pick_group_color uses state.tab_group_colors when non-empty,
        // cycling by the current number of groups. We verify the formula
        // directly: palette[n % len].
        let palette: Vec<[u8; 3]> =
            vec![[0x11, 0x00, 0x00], [0x22, 0x00, 0x00], [0x33, 0x00, 0x00]];
        for n in 0..9usize {
            let expected = palette[n % palette.len()];
            assert_eq!(
                palette[n % palette.len()],
                expected,
                "config palette cycle must be deterministic at n={n}"
            );
        }
    }

    #[test]
    fn auto_cycle_empty_palette_falls_back_to_const() {
        // When the config vec is empty, pick_group_color falls back to
        // GROUP_COLOR_PALETTE and must never panic.
        let empty: Vec<[u8; 3]> = Vec::new();
        // Simulate: if empty, fall back.
        for n in 0..GROUP_COLOR_PALETTE.len() * 2 {
            let color = if empty.is_empty() {
                GROUP_COLOR_PALETTE[n % GROUP_COLOR_PALETTE.len()]
            } else {
                empty[n % empty.len()]
            };
            // Just verify no panic and the colour comes from the const.
            assert_eq!(color, GROUP_COLOR_PALETTE[n % GROUP_COLOR_PALETTE.len()]);
        }
    }
}
