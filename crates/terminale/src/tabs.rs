//! Tab and pane lifecycle management: switch, create, close, rename,
//! clipboard, selection, and the "tabs" portion of the UI (refresh_tab_bar).

use crate::{
    ClosedTab, PaneId, RenameState, RenameTarget, RunningState, TabBar, TabBarItem, TabState,
};
use terminale_config::Profile;

// ── group_label_for helper ────────────────────────────────────────────────────

/// Compute the group label for the run-start tab at `tab_idx` that belongs to
/// group `gid`.  When a group-rename is in progress (target == Group(gid)),
/// the live buffer (with trailing cursor `|`) is returned instead of the
/// stored name, so the pill updates in real time.
fn group_label_for(
    gid: crate::TabGroupId,
    tab_idx: usize,
    groups: &[crate::TabGroup],
    rename: &Option<(usize, crate::RenameTarget, String)>,
) -> Option<String> {
    // If there's an active group rename targeting this group and tab_idx is
    // the same tab (run start), show the live buffer.
    if let Some((ri, crate::RenameTarget::Group(rgid), buf)) = rename {
        if *rgid == gid && *ri == tab_idx {
            return Some(format!("{buf}|"));
        }
    }
    groups.iter().find(|g| g.id == gid).map(|g| g.name.clone())
}

// ── tab_bar_from / refresh_tab_bar ───────────────────────────────────────────

pub(crate) fn tab_bar_from(
    tabs: &[&TabState],
    active: usize,
    maximized: bool,
    groups: &[crate::TabGroup],
) -> TabBar {
    let items = build_tab_bar_items(tabs, active, groups, &None, &mut |t| crate::tab_label(t));
    TabBar {
        items,
        hovered: None,
        plus_hovered: false,
        close_hovered: None,
        maximized,
        window_ctrl_hovered: None,
    }
}

/// Build the `Vec<TabBarItem>` from tabs + groups, computing group accent
/// colours and determining which tab should carry the group label.
///
/// `rename_info` is `Some((tab_idx, target, buffer))` when an inline rename
/// is in progress — used by [`group_label_for`] to show the live buffer on
/// the pill when the target is a group.
fn build_tab_bar_items(
    tabs: &[&TabState],
    active: usize,
    groups: &[crate::TabGroup],
    rename_info: &Option<(usize, crate::RenameTarget, String)>,
    label_for: &mut impl FnMut(&TabState) -> String,
) -> Vec<terminale_render::TabBarItem> {
    tabs.iter()
        .enumerate()
        .map(|(idx, t)| {
            let group_accent: Option<[u8; 3]> = t
                .group
                .and_then(|gid| groups.iter().find(|g| g.id == gid))
                .map(|g| g.color);
            // A tab carries the group label only when it is the first tab
            // in a consecutive same-group run.
            let group_label: Option<String> = if let Some(gid) = t.group {
                let prev_group = if idx > 0 { tabs[idx - 1].group } else { None };
                if prev_group == Some(gid) {
                    None
                } else {
                    // First tab of this group run — emit the label (or live
                    // buffer when a rename of this group is in progress).
                    group_label_for(gid, idx, groups, rename_info)
                }
            } else {
                None
            };
            terminale_render::TabBarItem {
                label: label_for(t),
                icon: t.user_icon.clone().or_else(|| t.icon.clone()),
                active: idx == active,
                unread: t.unread,
                color: t.user_color.or(t.auto_color),
                badge: t.auto_badge.clone(),
                pinned: t.pinned,
                group_accent,
                group_label,
            }
        })
        .collect()
}

pub(crate) fn refresh_tab_bar(state: &mut RunningState) {
    let maximized = state.window.is_maximized();
    // While renaming a tab, the edited tab shows its live buffer + a caret
    // instead of its normal label. For group renames the live buffer is
    // handled by group_label_for (called inside the item-build loops).
    let rename_tab: Option<(usize, String)> =
        state.renaming.as_ref().and_then(|r| match r.target {
            RenameTarget::Tab | RenameTarget::Pane(_) => {
                Some((r.tab_idx, format!("{}|", r.buffer)))
            }
            RenameTarget::Group(_) => None,
        });
    // Build rename_info tuple used by group_label_for.
    let rename_info: Option<(usize, crate::RenameTarget, String)> = state
        .renaming
        .as_ref()
        .map(|r| (r.tab_idx, r.target, r.buffer.clone()));

    // Pre-compute per-tab busy flags so the spinner prefix can be injected
    // without holding any emulator lock inside the label closure.
    // A tab is busy when at least one of its panes is busy.
    let tab_busy: Vec<bool> = if state.tab_activity_spinner {
        state
            .tabs
            .iter()
            .map(|t| t.panes.values().any(crate::osc_handlers::pane_is_busy))
            .collect()
    } else {
        vec![false; state.tabs.len()]
    };

    let spinner_prefix = crate::SPINNER_FRAMES[state.spinner_frame % crate::SPINNER_FRAMES.len()];

    let label_for = |idx: usize, t: &TabState| -> String {
        let base = match &rename_tab {
            Some((ri, buf)) if *ri == idx => buf.clone(),
            _ => crate::tab_label(t),
        };
        if state.tab_activity_spinner && *tab_busy.get(idx).unwrap_or(&false) {
            format!("{spinner_prefix}  {base}")
        } else {
            base
        }
    };
    // PATCH in place when the bar already exists, so transient state
    // (hover, close-hover, plus-hover, window-ctrl-hover) survives a
    // redraw. Build from scratch only on first frame.
    if let Some(bar) = state.renderer.tab_bar_mut() {
        bar.items.clear();
        for (idx, t) in state.tabs.iter().enumerate() {
            let group_accent = t
                .group
                .and_then(|gid| state.tab_groups.iter().find(|g| g.id == gid))
                .map(|g| g.color);
            let group_label: Option<String> = if let Some(gid) = t.group {
                let prev_group = if idx > 0 {
                    state.tabs[idx - 1].group
                } else {
                    None
                };
                if prev_group == Some(gid) {
                    None
                } else {
                    group_label_for(gid, idx, &state.tab_groups, &rename_info)
                }
            } else {
                None
            };
            bar.items.push(TabBarItem {
                label: label_for(idx, t),
                icon: t.user_icon.clone().or_else(|| t.icon.clone()),
                active: idx == state.active_tab,
                unread: t.unread && idx != state.active_tab,
                color: t.user_color.or(t.auto_color),
                badge: t.auto_badge.clone(),
                pinned: t.pinned,
                group_accent,
                group_label,
            });
        }
        bar.maximized = maximized;
        // hovered / close_hovered / plus_hovered / window_ctrl_hovered all
        // kept as-is — they get updated by CursorMoved, not by us.
        return;
    }
    let tabs_ref: Vec<&TabState> = state.tabs.iter().collect();
    let groups_ref = state.tab_groups.clone();
    let bar = build_tab_bar_items_for_initial(
        &tabs_ref,
        state.active_tab,
        maximized,
        &groups_ref,
        &rename_info,
        &rename_tab,
        SpinnerCtx {
            tab_busy: &tab_busy,
            prefix: spinner_prefix,
            on: state.tab_activity_spinner,
        },
    );
    state.renderer.set_tab_bar(Some(bar));
}

/// Spinner state forwarded to [`build_tab_bar_items_for_initial`].
struct SpinnerCtx<'a> {
    /// Per-tab busy flags, indexed in step with the tabs slice.
    tab_busy: &'a [bool],
    /// The current braille-dots frame glyph to prepend when a tab is busy.
    prefix: &'a str,
    /// Master on/off switch. When `false`, no label injection is performed.
    on: bool,
}

/// Builds a full [`TabBar`] from scratch (used only on first frame / when no
/// bar exists yet). Extracted so the patch-in-place path and the from-scratch
/// path share the same group-label logic.
fn build_tab_bar_items_for_initial(
    tabs: &[&TabState],
    active: usize,
    maximized: bool,
    groups: &[crate::TabGroup],
    rename_info: &Option<(usize, crate::RenameTarget, String)>,
    rename_tab: &Option<(usize, String)>,
    spinner: SpinnerCtx<'_>,
) -> TabBar {
    let mut items = build_tab_bar_items(tabs, active, groups, rename_info, &mut |t| {
        crate::tab_label(t)
    });
    // Patch in live tab-rename buffer for Tab/Pane targets,
    // and prepend the spinner frame for busy tabs when enabled.
    for (idx, item) in items.iter_mut().enumerate() {
        if let Some((ri, buf)) = rename_tab {
            if *ri == idx {
                item.label = buf.clone();
            }
        }
        if spinner.on && *spinner.tab_busy.get(idx).unwrap_or(&false) {
            item.label = format!("{}  {}", spinner.prefix, item.label);
        }
    }
    TabBar {
        items,
        hovered: None,
        plus_hovered: false,
        close_hovered: None,
        maximized,
        window_ctrl_hovered: None,
    }
}

// ── start_rename / start_rename_pane / handle_rename_input ───────────────────

/// Begin renaming the active tab. Pre-fills the buffer with any existing
/// user title so editing an existing name is easy.
pub(crate) fn start_rename(state: &mut RunningState) {
    let idx = state.active_tab;
    let Some(tab) = state.tabs.get(idx) else {
        return;
    };
    let buffer = tab.user_title.clone().unwrap_or_default();
    // Renaming hijacks the keyboard; close any other modal first.
    state.menu_visible = false;
    state.renderer.set_overlay(None);
    state.renaming = Some(RenameState {
        tab_idx: idx,
        target: RenameTarget::Tab,
        buffer,
    });
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Begin renaming a specific split pane via its header strip.
/// Pre-fills the buffer with any existing `user_title` on that pane
/// so editing is non-destructive. Only active when the tab has more
/// than one pane (single-pane tabs have no header strip to click).
pub(crate) fn start_rename_pane(state: &mut RunningState, pane_id: PaneId) {
    let idx = state.active_tab;
    let Some(tab) = state.tabs.get(idx) else {
        return;
    };
    let buffer = tab
        .panes
        .get(&pane_id)
        .and_then(|p| p.user_title.clone())
        .unwrap_or_default();
    // Renaming hijacks the keyboard; close any other modal first.
    state.menu_visible = false;
    state.renderer.set_overlay(None);
    state.renaming = Some(RenameState {
        tab_idx: idx,
        target: RenameTarget::Pane(pane_id),
        buffer,
    });
    refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Handle one keypress while the inline rename editor is open. Returns `true`
/// when the key was consumed (so it must not reach the PTY).
pub(crate) fn handle_rename_input(
    state: &mut RunningState,
    logical_key: &winit::keyboard::Key,
    text: Option<winit::keyboard::SmolStr>,
) -> bool {
    use winit::keyboard::{Key, NamedKey};
    let Some(rename) = state.renaming.as_mut() else {
        return false;
    };
    match logical_key {
        Key::Named(NamedKey::Enter) => {
            let name = rename.buffer.trim().to_string();
            let idx = rename.tab_idx;
            let target = rename.target;
            state.renaming = None;
            match target {
                RenameTarget::Group(gid) => {
                    // Keep the old name if the buffer is empty.
                    if !name.is_empty() {
                        crate::tab_groups::rename_group(state, gid, name);
                    }
                }
                RenameTarget::Pane(pid) => {
                    if let Some(tab) = state.tabs.get_mut(idx) {
                        let new_title = (!name.is_empty()).then_some(name);
                        if let Some(pane) = tab.panes.get_mut(&pid) {
                            pane.user_title = new_title;
                        }
                    }
                }
                RenameTarget::Tab => {
                    if let Some(tab) = state.tabs.get_mut(idx) {
                        tab.user_title = (!name.is_empty()).then_some(name);
                    }
                }
            }
            refresh_tab_bar(state);
        }
        Key::Named(NamedKey::Escape) => {
            state.renaming = None;
            refresh_tab_bar(state);
        }
        Key::Named(NamedKey::Backspace) => {
            rename.buffer.pop();
            refresh_tab_bar(state);
        }
        _ => {
            // Append printable text (ignore control chars / pure modifiers).
            if let Some(t) = text {
                for ch in t.chars() {
                    if !ch.is_control() {
                        rename.buffer.push(ch);
                    }
                }
                refresh_tab_bar(state);
            } else {
                return true; // swallow other keys (arrows etc.) without effect
            }
        }
    }
    true
}

// ── switch_tab / activate_tab_by_index / activate_last_tab ───────────────────

pub(crate) fn switch_tab(state: &mut RunningState, idx: usize) {
    if idx >= state.tabs.len() {
        return;
    }
    if idx == state.active_tab {
        // Already on this tab — no-op; don't clobber previous_active_tab.
        return;
    }
    // Tracked input line is per-window but conceptually per-tab; drop it on
    // a tab switch so a half-typed `ssh …` on one tab can't be attributed to
    // the next.
    state.input_line.clear();
    // Exit copy mode, quick-select, and pane-select on tab switch.
    if state.copy_mode.active {
        state.copy_mode.exit();
    }
    state.quick_select = None;
    state.pane_select = None;
    // Remember where we were so `last_tab` can flip back.
    state.previous_active_tab = Some(state.active_tab);
    state.active_tab = idx;
    if let Some(t) = state.tabs.get_mut(idx) {
        t.unread = false;
    }
    // Notify plugins that focus moved to the new tab's focused pane.
    if let Some(focused_id) = state.tabs.get(idx).map(|t| t.focused) {
        state.pending_hook_pane_focus.push(focused_id);
    }
    state.renderer.set_selection(None);
    let scroll = state.tabs.get(idx).map_or(0, |t| t.scroll_lines);
    state.renderer.set_scroll_lines(scroll);
    state.window.request_redraw();
}

/// Jump to the tab at 0-based `idx`. No-op when `idx` is out of range.
pub(crate) fn activate_tab_by_index(state: &mut RunningState, idx: usize) {
    if idx < state.tabs.len() {
        switch_tab(state, idx);
    }
}

/// Toggle to the previously-active tab.
pub(crate) fn activate_last_tab(state: &mut RunningState) {
    if let Some(prev) = state.previous_active_tab {
        if prev < state.tabs.len() {
            switch_tab(state, prev);
        }
    }
}

// ── new_tab / new_tab_with_profile / new_tab_inner ───────────────────────────

pub(crate) fn new_tab(state: &mut RunningState) {
    new_tab_inner(state, None);
}

/// Like [`new_tab`] but launches a specific profile.
pub(crate) fn new_tab_with_profile(state: &mut RunningState, profile: &Profile) {
    new_tab_inner(state, Some(profile));
}

pub(crate) fn new_tab_inner(state: &mut RunningState, profile: Option<&Profile>) {
    let size = state.window.inner_size();
    let initial = (terminale_term::DEFAULT_COLS, terminale_term::DEFAULT_ROWS);
    // Inherit cwd from the active tab so "new tab" feels native.
    let inherited_cwd: Option<std::path::PathBuf> =
        state.tabs.get(state.active_tab).and_then(|t| {
            t.emulator
                .lock()
                .current_dir()
                .map(std::path::PathBuf::from)
        });
    // When the caller didn't pin a profile (the "+" button / Ctrl+T path),
    // fall back to the window's default profile so every tab matches the
    // first one (name + icon + command) rather than degrading to a bare
    // "shell" label. We still inherit the active tab's cwd on top.
    let effective: Option<Profile> = profile.cloned().or_else(|| state.default_profile.clone());
    let profile_owned: Option<Profile> = match (effective, inherited_cwd) {
        (Some(mut p), cwd) => {
            // Only override if the profile didn't already pin a cwd.
            if p.cwd.is_none() {
                p.cwd = cwd;
            }
            Some(p)
        }
        (None, _) => None,
    };
    let new = crate::spawn_tab(
        profile_owned.as_ref(),
        None,
        &state.renderer,
        initial,
        size.width,
        size.height,
        state.proxy.clone(),
        state.scrollback_lines,
    );
    state.tabs.push(new);
    state.active_tab = state.tabs.len() - 1;
    // Fresh tab → fresh tracked input line.
    state.input_line.clear();
    // Inherit the active theme palette so freshly-spawned tabs match the
    // rest of the window (rather than rendering with the built-in fallback).
    // Also apply the current command-block capture settings.
    if let Some(t) = state.tabs.last() {
        let mut emu = t.emulator.lock();
        emu.set_palette(state.palette);
        emu.set_command_blocks(state.command_blocks_enabled, state.max_command_blocks);
    }
    state.renderer.set_selection(None);
    state.window.request_redraw();

    // Enqueue tab_open hook for the App to fire on the next tick.
    let new_idx = state.active_tab;
    let title = state
        .tabs
        .get(new_idx)
        .map(crate::tab_label)
        .unwrap_or_default();
    state.pending_hook_tab_open.push((new_idx, title));

    // Enqueue session_start for the single pane that was just spawned.
    // New tabs always have pane id 0 (TabState::new_single assigns it).
    let program = state
        .tabs
        .get(new_idx)
        .and_then(|t| t.panes.get(&0))
        .map_or_else(|| "shell".to_string(), |p| p.profile_name.clone());
    state.pending_hook_session_start.push((0, program));
}

// ── active_tab_after_detach / reopen_closed_tab ──────────────────────────────

/// New active-tab index after the tab at `removed` is taken out of a list
/// that, *before* removal, had `active` as its active index and `len_before`
/// tabs.
pub(crate) fn active_tab_after_detach(active: usize, removed: usize, len_before: usize) -> usize {
    let len_after = len_before.saturating_sub(1);
    if len_after == 0 {
        return 0;
    }
    if active >= len_after {
        len_after - 1
    } else if active > removed {
        active - 1
    } else {
        active
    }
}

/// How many recently-closed tabs to remember for "reopen closed tab".
pub(crate) const MAX_CLOSED_TABS: usize = 16;

/// Reopen the most recently closed tab in its original directory.
pub(crate) fn reopen_closed_tab(state: &mut RunningState) {
    if let Some(closed) = state.closed_tabs.pop() {
        let profile = crate::profile_from_closed(&closed);
        new_tab_with_profile(state, &profile);
    }
}

// ── close_confirmed / request_close_tab / close_tab ──────────────────────────

/// How long an armed close stays valid while waiting for the confirming
/// second close action (`window.confirm_close`).
pub(crate) const CONFIRM_CLOSE_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// `true` when a close action should proceed right now.
pub(crate) fn close_confirmed(state: &mut RunningState) -> bool {
    if !state.confirm_close {
        return true;
    }
    let now = std::time::Instant::now();
    match state.pending_close {
        Some(deadline) if now <= deadline => {
            // Second action inside the window — let it through.
            state.pending_close = None;
            true
        }
        _ => {
            // Arm (or re-arm) and wait for the confirming action. Flash the
            // visual bell as non-modal feedback that the close was seen but
            // needs a confirming second action.
            state.pending_close = Some(now + CONFIRM_CLOSE_WINDOW);
            state.renderer.trigger_visual_bell();
            state.window.request_redraw();
            false
        }
    }
}

/// Close `idx`, honouring `window.confirm_close`.
pub(crate) fn request_close_tab(state: &mut RunningState, idx: usize) {
    if close_confirmed(state) {
        close_tab(state, idx);
    }
}

pub(crate) fn close_tab(state: &mut RunningState, idx: usize) {
    if idx >= state.tabs.len() {
        return;
    }
    // Enqueue tab_close hook for the App to fire on the next tick.
    state.pending_hook_tab_close.push(idx);
    // Remember enough to reopen this tab later (newest last, capped).
    let restore = state.tabs.get(idx).map(|tab| ClosedTab {
        profile_name: tab.profile_name.clone(),
        icon: tab.icon.clone(),
        cwd: tab
            .emulator
            .lock()
            .current_dir()
            .map(std::path::PathBuf::from),
    });
    if let Some(rec) = restore {
        state.closed_tabs.push(rec);
        if state.closed_tabs.len() > MAX_CLOSED_TABS {
            state.closed_tabs.remove(0);
        }
    }
    let len_before = state.tabs.len();
    state.tabs.remove(idx);
    // A closed tab may have emptied its group — drop now-orphaned groups so the
    // group registry never accumulates member-less entries.
    crate::tab_groups::prune_empty_groups(state);
    if state.tabs.is_empty() {
        // Last tab of THIS window gone — hide it and leave the empty `tabs`
        // for the App loop to reap.
        state.window.set_visible(false);
        return;
    }
    state.active_tab = active_tab_after_detach(state.active_tab, idx, len_before);
    // Adjust or invalidate the previous-active-tab pointer so it doesn't
    // point at a now-stale index after the removal.
    state.previous_active_tab = state.previous_active_tab.and_then(|prev| {
        if prev == idx {
            None
        } else {
            let adjusted = active_tab_after_detach(prev, idx, len_before);
            Some(adjusted)
        }
    });
    state.window.request_redraw();
}

// ── close_exited_pane ─────────────────────────────────────────────────────────

/// Close a pane that exited (PTY EOF) when `exit_behavior` is `Close`.
///
/// Temporarily re-focuses the given pane so `TabState::close_focused()` can
/// collapse it, then restores focus to the previous pane if the tab survives.
/// If the tab had only one pane, closes the entire tab instead.
///
/// Called from `osc_handlers::handle_pane_exits` — must NOT be called while
/// a borrow on `state.tabs` is held.
pub(crate) fn close_exited_pane(state: &mut RunningState, pane_id: crate::PaneId) {
    let active = state.active_tab;

    // Peek at pane count without holding a long-lived borrow.
    let is_single = state.tabs.get(active).is_none_or(|t| t.panes.len() <= 1);

    if is_single {
        // Single-pane tab: close the whole tab.
        close_tab(state, active);
        return;
    }

    // Multi-pane tab: temporarily switch focus to the exited pane so
    // `TabState::close_focused()` can collapse it, then restore the
    // original focused pane (which is still alive).
    let original_focus = state.tabs.get(active).map_or(pane_id, |t| t.focused);

    if let Some(tab) = state.tabs.get_mut(active) {
        tab.focused = pane_id;
        let _closed = tab.close_focused();
        // Restore user's focus if the original pane survived.
        if tab.panes.contains_key(&original_focus) {
            tab.focused = original_focus;
        }
    }

    crate::panes::resize_active_tab_panes(state);
    // refresh_tab_bar picks up the new layout (pane count changed).
    refresh_tab_bar(state);
    state.renderer.set_selection(None);
    state.window.request_redraw();
}

// ── restart_focused_pane ──────────────────────────────────────────────────────

/// Restart the focused pane's session **in place**: kill the child process
/// and respawn `profile` (falling back to the default shell) inside the
/// SAME pane, preserving the tab's pane tree and the pane's position.
///
/// Supersedes the old `restart_active_tab` (which only worked on crashed
/// tabs, spawned a hardcoded shell and destroyed split layouts by
/// rebuilding the whole TabState): this works on healthy panes too (the
/// context-menu "Restart session"), keeps split layouts intact, honours
/// the pane's profile command, and inherits the live OSC 7 cwd when the
/// profile doesn't pin one — so the new shell starts where the old one
/// was. Crashed panes restart naturally (`crashed` is reset below).
///
/// SSH tabs are skipped: their session is built by the async connect flow
/// (`finish_ssh_tab`), not a local spawn; the menu item is disabled for
/// them as well.
pub(crate) fn restart_focused_pane(
    state: &mut RunningState,
    profile: Option<&terminale_config::Profile>,
) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };
    if !tab.ssh_host_name.is_empty() {
        tracing::debug!("pane restart skipped: SSH session");
        return;
    }
    let pane_id = tab.focused;
    let Some(pane) = tab.panes.get(&pane_id) else {
        return;
    };
    let profile_name = pane.profile_name.clone();
    let icon = pane.icon.clone();
    // Spawn straight at the pane's current grid size — it keeps its
    // sub-rect, so no post-spawn resize (and no ConPTY reflow) is needed.
    let (cols, rows) = (pane.cols, pane.rows);
    let inherited_cwd: Option<std::path::PathBuf> = tab
        .emulator
        .lock()
        .current_dir()
        .map(std::path::PathBuf::from);

    // The respawn profile: the resolved config profile when the caller
    // provides one (menu / keybind path resolves it by name), with the
    // live cwd overlaid unless the profile pins its own; otherwise a
    // cwd-only profile running the default shell — same semantics as
    // new_tab / split.
    let respawn: terminale_config::Profile = match profile {
        Some(p) => {
            let mut p = p.clone();
            if p.cwd.is_none() {
                p.cwd = inherited_cwd;
            }
            p
        }
        None => terminale_config::Profile {
            name: profile_name.clone(),
            command: String::new(),
            args: Vec::new(),
            env: Default::default(),
            cwd: inherited_cwd,
            icon: icon.clone(),
        },
    };

    let spec = crate::build_spawn_spec(Some(&respawn), None);
    let proxy = state.proxy.clone();
    let notifier: terminale_core::DataNotifier = std::sync::Arc::new(move || {
        let _ = proxy.send_event(crate::UserEvent::PtyDataReady);
    });
    let Ok(mut session) = terminale_core::Session::spawn_with_notifier(&spec, cols, rows, notifier)
    else {
        tracing::warn!(profile = %respawn.name, "pane restart failed: could not spawn session");
        return;
    };
    let Some(output_rx) = session.take_output() else {
        return;
    };
    let mut emulator = terminale_term::Emulator::new(cols, rows);
    emulator.set_scrollback(state.scrollback_lines);
    emulator.set_palette(state.palette);
    emulator.set_command_blocks(state.command_blocks_enabled, state.max_command_blocks);

    let Some(tab) = state.tabs.get_mut(active) else {
        return;
    };
    let Some(pane) = tab.panes.get_mut(&pane_id) else {
        return;
    };
    // Replacing `session` drops the old one, which kills the old child
    // process (Session's Drop impl). The emulator is rebuilt from scratch
    // so no stale grid/scrollback survives the restart.
    pane.session = session;
    pane.output_rx = output_rx;
    pane.emulator = std::sync::Arc::new(parking_lot::Mutex::new(emulator));
    pane.crashed = false;
    pane.scroll_lines = 0;
    pane.custom_title = None;
    pane.last_output_at = None;
    pane.last_input_at = None;
    pane.autodetect_links.clear();

    // Notify plugins: the old session ended (no exit status — we killed
    // it; -1 mirrors the "no local status" convention), a new one started.
    state.pending_hook_session_exit.push((pane_id, -1));
    state
        .pending_hook_session_start
        .push((pane_id, respawn.name.clone()));

    // Re-fit to the pane's sub-rect (no-op when cols/rows already match,
    // thanks to the same-size guard) and reset the view state.
    crate::panes::resize_active_tab_panes(state);
    state.renderer.set_scroll_lines(0);
    state.renderer.set_selection(None);
    refresh_tab_bar(state);
    state.window.request_redraw();
}

// ── selection / clipboard ─────────────────────────────────────────────────────

/// Extract the currently-selected text from the active tab's grid, or
/// `None` when there's no (non-empty) selection.
pub(crate) fn selection_text(state: &RunningState) -> Option<String> {
    let sel = state.renderer.selection()?;
    let scroll = state.renderer.scroll_lines();
    let tab = state.tabs.get(state.active_tab)?;
    let emu = tab.emulator.lock();
    let text = if sel.block {
        let (a_col, a_row) = sel.anchor;
        let (c_col, c_row) = sel.cursor;
        let (col_lo, col_hi) = if a_col <= c_col {
            (a_col, c_col)
        } else {
            (c_col, a_col)
        };
        let (row_lo, row_hi) = if a_row <= c_row {
            (a_row, c_row)
        } else {
            (c_row, a_row)
        };
        (row_lo..=row_hi)
            .map(|r| emu.text_in_range((col_lo, r), (col_hi, r), scroll))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        emu.text_in_range(sel.anchor, sel.cursor, scroll)
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) fn copy_selection(state: &mut RunningState) {
    let Some(text) = selection_text(state) else {
        return;
    };
    crate::push_clipboard_history(state, text.clone());
    if let Some(cb) = state.clipboard.as_mut() {
        if let Err(e) = cb.set_text(text) {
            tracing::warn!(?e, "clipboard copy failed");
        }
    }
}

/// Build the byte payload for a paste. Normalises CRLF→LF and strips
/// embedded paste markers in bracketed mode.
pub(crate) fn build_paste_payload(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\n").replace('\r', "\n");
    if bracketed {
        let body = body.replace("\x1b[201~", "").replace("\x1b[200~", "");
        let mut out = Vec::with_capacity(body.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        body.into_bytes()
    }
}

/// Write `text` to the active pane's PTY immediately, applying
/// `strip_control_chars` if the config requests it. Bracketed-paste wrapping
/// is applied when the focused program has enabled bracketed paste.
///
/// Called from two places:
/// 1. [`paste_clipboard`] — when no confirmation is needed.
/// 2. From the App loop — when the user confirmed the paste-guard dialog.
pub(crate) fn send_paste_text(state: &mut RunningState, text: &str) {
    let strip_control_chars = state.paste_strip_control_chars;
    let Some(tab) = state.tabs.get_mut(state.active_tab) else {
        return;
    };
    let bracketed = tab.emulator.lock().bracketed_paste_enabled();
    // Optionally strip control bytes before building the payload.
    let payload = if strip_control_chars {
        let stripped = crate::paste_guard::strip_control_chars(text);
        build_paste_payload(&stripped, bracketed)
    } else {
        build_paste_payload(text, bracketed)
    };
    let _ = tab.session.write_input(&payload);
    // Stamp the paste as user input so the busy-spinner fallback can tell
    // the resulting echo / prompt repaint apart from real command output.
    tab.focused_pane_mut().last_input_at = Some(std::time::Instant::now());
}

/// Outcome of a paste attempt: either the text was sent directly, or a
/// confirmation dialog must be shown first.
pub(crate) enum PasteAction {
    /// The text was sent to the PTY immediately — nothing more to do.
    Sent,
    /// A confirmation dialog should be shown for this text before sending.
    NeedsConfirm { text: String, bracketed: bool },
}

/// Attempt to paste the clipboard contents into the active pane.
///
/// If the paste-safety policy requires a confirmation dialog (multi-line text
/// without bracketed paste, or unconditional multi-line confirmation), this
/// returns [`PasteAction::NeedsConfirm`] with the pending text so the caller
/// can open the dialog. Otherwise the text is sent immediately and
/// [`PasteAction::Sent`] is returned.
pub(crate) fn paste_clipboard(state: &mut RunningState) -> PasteAction {
    let Some(cb) = state.clipboard.as_mut() else {
        return PasteAction::Sent;
    };
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return PasteAction::Sent;
    };
    match cb.get_text() {
        Ok(text) => {
            let bracketed = tab.emulator.lock().bracketed_paste_enabled();
            if crate::paste_guard::paste_needs_confirm(
                &text,
                bracketed,
                state.paste_confirm_multiline,
                state.paste_confirm_when_unbracketed,
            ) {
                PasteAction::NeedsConfirm { text, bracketed }
            } else {
                send_paste_text(state, &text);
                PasteAction::Sent
            }
        }
        Err(e) => {
            tracing::warn!(?e, "clipboard paste failed");
            PasteAction::Sent
        }
    }
}

pub(crate) fn select_all(state: &mut RunningState) {
    use terminale_render::CellRect;
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let (cols, rows) = tab.emulator.lock().size();
    state.renderer.set_selection(Some(CellRect {
        anchor: (0, 0),
        cursor: (cols.saturating_sub(1), rows.saturating_sub(1)),
        block: false,
    }));
}

pub(crate) fn clear_screen(state: &mut RunningState) {
    if let Some(tab) = state.tabs.get(state.active_tab) {
        let _ = tab.session.write_input(b"\x1b[2J\x1b[H");
    }
    state.renderer.set_selection(None);
}

// ── copy_current_path ─────────────────────────────────────────────────────────
// Note: open_settings is defined in main.rs (it just sets open_settings_requested).

/// Copy the active tab's working directory (from OSC 7) to the clipboard.
pub(crate) fn copy_current_path(state: &mut RunningState) {
    let Some(path) = state
        .tabs
        .get(state.active_tab)
        .and_then(|t| t.emulator.lock().current_dir().map(ToString::to_string))
    else {
        return;
    };
    if let Some(cb) = state.clipboard.as_mut() {
        if let Err(e) = cb.set_text(path) {
            tracing::warn!(?e, "clipboard copy (current path) failed");
        }
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenameTarget, TabGroup};

    #[test]
    fn group_pill_shows_live_buffer_during_rename() {
        // Arrange: one group with id=7, name "Deploy". A rename is in progress
        // targeting Group(7), buffer "Dep". The run starts at tab_idx=2.
        let groups = vec![TabGroup {
            id: 7,
            name: "Deploy".into(),
            color: [0x4e, 0xa8, 0xff],
        }];
        let rename_info: Option<(usize, RenameTarget, String)> =
            Some((2, RenameTarget::Group(7), "Dep".into()));

        // The label for the run-start tab (tab_idx=2) in group 7 must reflect
        // the live buffer with the trailing cursor character.
        let label = group_label_for(7, 2, &groups, &rename_info);
        assert_eq!(
            label,
            Some("Dep|".to_string()),
            "run-start label must show live rename buffer with caret"
        );

        // A tab that is not the run start (different tab_idx) must NOT show
        // the live buffer even if it is in the same group.
        let label_non_start = group_label_for(7, 3, &groups, &rename_info);
        // tab_idx=3 != rename_info tab_idx=2 → stored name, not live buffer.
        assert_eq!(
            label_non_start,
            Some("Deploy".to_string()),
            "non-run-start must use stored group name, not live buffer"
        );
    }

    #[test]
    fn group_label_for_no_rename_returns_stored_name() {
        let groups = vec![TabGroup {
            id: 3,
            name: "Build".into(),
            color: [0; 3],
        }];
        let label = group_label_for(3, 0, &groups, &None);
        assert_eq!(label, Some("Build".to_string()));
    }
}
