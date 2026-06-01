//! Command palette: fuzzy search, ranking, modes, and input handling.

use crate::{CommandPaletteState, PaletteItem, PaletteMode, RunningState, ShortcutAction};
use winit::keyboard::NamedKey;

// ── PALETTE_ACTIONS ───────────────────────────────────────────────────────────

/// Every action surfaced in the command palette, with its display label.
/// This is the pre-filter display order (roughly most-common first).
/// `CommandPalette` itself is deliberately omitted — there's no point
/// re-opening the palette from inside it.
pub(crate) const PALETTE_ACTIONS: &[(ShortcutAction, &str)] = &[
    (ShortcutAction::NewTab, "New Tab"),
    (ShortcutAction::NewWindow, "New Window"),
    (ShortcutAction::ProfilePicker, "New Tab with Profile…"),
    (ShortcutAction::NewSshTab, "New SSH Tab…"),
    (ShortcutAction::CloseTab, "Close Tab"),
    (ShortcutAction::ReopenClosedTab, "Reopen Closed Tab"),
    (ShortcutAction::RestartTab, "Restart Tab"),
    (ShortcutAction::NextTab, "Next Tab"),
    (ShortcutAction::PrevTab, "Previous Tab"),
    (ShortcutAction::MoveTabLeft, "Move Tab Left"),
    (ShortcutAction::MoveTabRight, "Move Tab Right"),
    (ShortcutAction::MoveTabToNewWindow, "Move Tab to New Window"),
    (ShortcutAction::MovePaneToNewTab, "Move Pane to New Tab"),
    (
        ShortcutAction::MovePaneToNewWindow,
        "Move Pane to New Window",
    ),
    (ShortcutAction::Copy, "Copy"),
    (ShortcutAction::Paste, "Paste"),
    (ShortcutAction::SelectAll, "Select All"),
    (ShortcutAction::Find, "Find in Terminal"),
    (ShortcutAction::Clear, "Clear Screen"),
    (ShortcutAction::ClearScrollback, "Clear Scrollback"),
    (ShortcutAction::AiAssistant, "AI Assistant"),
    (
        ShortcutAction::ExplainSelection,
        "Explain Selection with AI",
    ),
    (ShortcutAction::FixLastCommand, "Fix Last Failed Command"),
    (
        ShortcutAction::CopyLastCommandOutput,
        "Copy Last Command Output",
    ),
    (ShortcutAction::CopyBlockOutput, "Copy Block Output"),
    (ShortcutAction::CopyLastCommand, "Copy Last Command"),
    (ShortcutAction::RerunLastCommand, "Re-run Last Command"),
    (ShortcutAction::EditLastCommand, "Edit Last Command"),
    (ShortcutAction::Settings, "Open Settings"),
    (ShortcutAction::FontIncrease, "Increase Font Size"),
    (ShortcutAction::FontDecrease, "Decrease Font Size"),
    (ShortcutAction::FontReset, "Reset Font Size"),
    (ShortcutAction::ScrollLineUp, "Scroll Line Up"),
    (ShortcutAction::ScrollLineDown, "Scroll Line Down"),
    (ShortcutAction::ScrollPageUp, "Scroll Page Up"),
    (ShortcutAction::ScrollPageDown, "Scroll Page Down"),
    (ShortcutAction::ScrollTop, "Scroll to Top"),
    (ShortcutAction::ScrollBottom, "Scroll to Bottom"),
    (ShortcutAction::ToggleStayOnTop, "Toggle Stay on Top"),
    (ShortcutAction::SnapMaximize, "Snap: Maximize"),
    (ShortcutAction::SnapTop, "Snap: Top Half"),
    (ShortcutAction::SnapBottom, "Snap: Bottom Half"),
    (ShortcutAction::SnapLeft, "Snap: Left Half"),
    (ShortcutAction::SnapRight, "Snap: Right Half"),
    (ShortcutAction::SnapCenter, "Snap: Center"),
    (ShortcutAction::SnapTopLeft, "Snap: Top-Left Quarter"),
    (ShortcutAction::SnapTopRight, "Snap: Top-Right Quarter"),
    (ShortcutAction::SnapBottomLeft, "Snap: Bottom-Left Quarter"),
    (
        ShortcutAction::SnapBottomRight,
        "Snap: Bottom-Right Quarter",
    ),
    (ShortcutAction::ShowSnapLayouts, "Show Snap Layouts"),
    (ShortcutAction::SplitRight, "Split Pane Right"),
    (ShortcutAction::SplitDown, "Split Pane Down"),
    (ShortcutAction::SplitLeft, "Split Pane Left"),
    (ShortcutAction::SplitUp, "Split Pane Up"),
    (ShortcutAction::ClosePane, "Close Pane"),
    (ShortcutAction::FocusPaneLeft, "Focus Pane Left"),
    (ShortcutAction::FocusPaneRight, "Focus Pane Right"),
    (ShortcutAction::FocusPaneUp, "Focus Pane Up"),
    (ShortcutAction::FocusPaneDown, "Focus Pane Down"),
    (ShortcutAction::TogglePaneZoom, "Toggle Pane Zoom"),
    (ShortcutAction::ResizePaneLeft, "Resize Pane Left"),
    (ShortcutAction::ResizePaneRight, "Resize Pane Right"),
    (ShortcutAction::ResizePaneUp, "Resize Pane Up"),
    (ShortcutAction::ResizePaneDown, "Resize Pane Down"),
    // Tab-index jumps.
    (ShortcutAction::ActivateTab1, "Go to Tab 1"),
    (ShortcutAction::ActivateTab2, "Go to Tab 2"),
    (ShortcutAction::ActivateTab3, "Go to Tab 3"),
    (ShortcutAction::ActivateTab4, "Go to Tab 4"),
    (ShortcutAction::ActivateTab5, "Go to Tab 5"),
    (ShortcutAction::ActivateTab6, "Go to Tab 6"),
    (ShortcutAction::ActivateTab7, "Go to Tab 7"),
    (ShortcutAction::ActivateTab8, "Go to Tab 8"),
    (ShortcutAction::ActivateTab9, "Go to Last Tab (Tab 9)"),
    (ShortcutAction::LastTab, "Go to Last-Used Tab"),
    (ShortcutAction::PrevPrompt, "Jump to Previous Prompt"),
    (ShortcutAction::NextPrompt, "Jump to Next Prompt"),
    (
        ShortcutAction::JumpToPrevFailedCommand,
        "Jump to Previous Failed Command",
    ),
    (
        ShortcutAction::JumpToNextFailedCommand,
        "Jump to Next Failed Command",
    ),
    (
        ShortcutAction::OpenFailedCommandPicker,
        "Failed Commands\u{2026}",
    ),
    (ShortcutAction::CopyMode, "Enter Copy Mode"),
    (ShortcutAction::QuickSelect, "Quick Select"),
    (ShortcutAction::PaneSelect, "Pane Select"),
    (ShortcutAction::ReloadConfig, "Reload Config"),
    (
        ShortcutAction::ToggleBroadcastInput,
        "Toggle Broadcast Input",
    ),
    (ShortcutAction::OpenSnippets, "Snippets\u{2026}"),
    (ShortcutAction::SaveWorkspace, "Save Workspace\u{2026}"),
    (ShortcutAction::OpenWorkspace, "Open Workspace\u{2026}"),
    (
        ShortcutAction::ImportSshHosts,
        "Import SSH Hosts from SSH Config",
    ),
    (
        ShortcutAction::OpenCommandHistory,
        "Command History\u{2026}",
    ),
    (
        ShortcutAction::ExportScrollback,
        "Export Scrollback\u{2026}",
    ),
    (
        ShortcutAction::OpenClipboardHistory,
        "Clipboard History\u{2026}",
    ),
    (ShortcutAction::OpenDirectoryJump, "Directory Jump\u{2026}"),
    (ShortcutAction::ImportTheme, "Import Theme\u{2026}"),
    (ShortcutAction::ToggleTabPin, "Pin / Unpin Tab"),
    // Tab-group actions.
    (ShortcutAction::NewTabGroup, "New Tab Group"),
    (ShortcutAction::AssignTabToGroup, "Assign Tab to Group"),
    (ShortcutAction::ClearTabGroup, "Clear Tab Group"),
    (ShortcutAction::RenameTabGroup, "Rename Tab Group\u{2026}"),
    // Pane swap / rotate.
    (ShortcutAction::MovePaneLeft, "Move Pane Left"),
    (ShortcutAction::MovePaneRight, "Move Pane Right"),
    (ShortcutAction::MovePaneUp, "Move Pane Up"),
    (ShortcutAction::MovePaneDown, "Move Pane Down"),
    (ShortcutAction::RotatePanes, "Rotate Panes Forward"),
    (ShortcutAction::RotatePanesBack, "Rotate Panes Backward"),
];

// ── fuzzy_score / rank_candidates ─────────────────────────────────────────────

/// Case-insensitive subsequence fuzzy match. Returns `None` when `query`
/// isn't a subsequence of `cand`, else a score where higher is better.
/// Rewards matches at word boundaries, consecutive runs, and a leading
/// prefix — the heuristics most fuzzy finders use.
pub(crate) fn fuzzy_score(query: &str, cand: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let c: Vec<char> = cand.chars().flat_map(char::to_lowercase).collect();
    let mut qi = 0usize;
    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    for (ci, &ch) in c.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if ch == q[qi] {
            score += 1;
            if prev == Some(ci.wrapping_sub(1)) {
                score += 5; // consecutive run
            }
            let at_boundary = ci == 0 || c.get(ci - 1).is_some_and(|p| !p.is_alphanumeric());
            if at_boundary {
                score += 8;
            }
            if ci == qi {
                score += 2; // leading prefix
            }
            prev = Some(ci);
            qi += 1;
        }
    }
    if qi == q.len() {
        // Slightly prefer shorter candidates on ties.
        Some(score - (c.len() as i32) / 8)
    } else {
        None
    }
}

/// Fuzzy-filter + rank a list of `(item, label, binding)` candidates,
/// returning each survivor paired with its renderer row. Highest score
/// first; ties keep the input order.
pub(crate) fn rank_candidates(
    query: &str,
    candidates: Vec<(PaletteItem, String, String)>,
) -> Vec<(PaletteItem, terminale_render::PaletteEntry)> {
    let mut scored: Vec<(i32, usize, PaletteItem, terminale_render::PaletteEntry)> =
        Vec::with_capacity(candidates.len());
    for (i, (item, label, binding)) in candidates.into_iter().enumerate() {
        if let Some(score) = fuzzy_score(query, &label) {
            scored.push((
                score,
                i,
                item,
                terminale_render::PaletteEntry { label, binding },
            ));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, it, e)| (it, e)).collect()
}

/// One direct-connect palette candidate per configured SSH host, rendered
/// as a searchable `SSH: <name>` row. Shared by the `Actions` list and the
/// scoped `SshQuickConnect` picker so both stay in sync.
pub(crate) fn ssh_host_rows(ssh_host_names: &[String]) -> Vec<(PaletteItem, String, String)> {
    ssh_host_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            (
                PaletteItem::OpenSsh(idx),
                format!("SSH: {name}"),
                String::new(),
            )
        })
        .collect()
}

/// One candidate per configured snippet, rendered as a searchable row with
/// the snippet name as the primary label and the description as the binding
/// hint column (since bindings are not applicable in scoped modes).
pub(crate) fn snippet_rows(
    snippet_names: &[(String, String)],
) -> Vec<(PaletteItem, String, String)> {
    snippet_names
        .iter()
        .enumerate()
        .map(|(idx, (name, desc))| (PaletteItem::InsertSnippet(idx), name.clone(), desc.clone()))
        .collect()
}

/// One candidate per saved named workspace.
pub(crate) fn workspace_rows(
    workspace_list: &[(String, std::path::PathBuf)],
) -> Vec<(PaletteItem, String, String)> {
    workspace_list
        .iter()
        .enumerate()
        .map(|(idx, (name, _path))| {
            (
                PaletteItem::OpenNamedWorkspace(idx),
                name.clone(),
                String::new(),
            )
        })
        .collect()
}

/// Build the deduplicated, most-recent-first command-history list from an
/// iterator of `&str` slices (each is a `command_text` from a
/// `CommandBlock`). Empty strings are filtered out; duplicates keep only the
/// most-recent occurrence; the result is capped at `max_entries`.
///
/// `iter` must yield the blocks **oldest first** (the natural storage order).
/// The function reverses the list internally so the most-recent entry lands
/// at index 0 in the output.
pub(crate) fn build_command_history<'a>(
    iter: impl Iterator<Item = &'a str>,
    max_entries: usize,
) -> Vec<String> {
    // Collect in reverse so we see the newest entry first while deduplicating.
    let mut seen = std::collections::HashSet::new();
    let mut result: Vec<String> = Vec::new();
    // Gather all items, then reverse so the newest come first.
    let all: Vec<&str> = iter.collect();
    for cmd in all.into_iter().rev() {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        if seen.insert(cmd) {
            result.push(cmd.to_string());
            if result.len() >= max_entries {
                break;
            }
        }
    }
    result
}

/// One palette candidate per history entry.
pub(crate) fn command_history_rows(history: &[String]) -> Vec<(PaletteItem, String, String)> {
    history
        .iter()
        .map(|cmd| {
            (
                PaletteItem::InsertCommand(cmd.clone()),
                cmd.clone(),
                String::new(),
            )
        })
        .collect()
}

/// One palette candidate per clipboard-history entry (most-recent first).
/// Each row's `PaletteItem` is `PasteClipboardEntry` so selecting it pastes
/// the text into the focused pane via the normal bracketed-paste path.
pub(crate) fn clipboard_history_rows(entries: &[String]) -> Vec<(PaletteItem, String, String)> {
    entries
        .iter()
        .map(|text| {
            // Collapse newlines so the label fits on one palette row.
            let label = text
                .lines()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            (
                PaletteItem::PasteClipboardEntry(text.clone()),
                if label.is_empty() {
                    text.clone()
                } else {
                    label
                },
                String::new(),
            )
        })
        .collect()
}

/// One palette candidate per failed command block (non-zero exit code),
/// newest first.  Each entry carries the prompt_line for `JumpToBlock`.
///
/// `failed_blocks` is a slice of `(prompt_line, label_text)` pairs where
/// `label_text` is the command text (possibly empty if the shell didn't
/// emit the B marker).  The hint column shows the exit code.
pub(crate) fn failed_command_rows(
    failed_blocks: &[(i32, String)],
) -> Vec<(PaletteItem, String, String)> {
    failed_blocks
        .iter()
        .map(|(line, label)| {
            let display = if label.is_empty() {
                format!("<line {line}>")
            } else {
                label.clone()
            };
            (PaletteItem::JumpToBlock(*line), display, String::new())
        })
        .collect()
}

/// One palette candidate per ranked directory in the frecency store
/// (highest frecency first). Each row's `PaletteItem` is `JumpToDirectory`
/// so selecting it sends `cd <path>\n` to the focused pane's PTY.
pub(crate) fn directory_jump_rows(dirs: &[String]) -> Vec<(PaletteItem, String, String)> {
    dirs.iter()
        .map(|path| {
            (
                PaletteItem::JumpToDirectory(path.clone()),
                path.clone(),
                String::new(),
            )
        })
        .collect()
}

/// Build plugin-contributed command rows for the `Actions` palette.
/// One row per entry in `plugin_command_names`, labelled exactly as the
/// plugin registered it.
pub(crate) fn plugin_command_rows(names: &[String]) -> Vec<(PaletteItem, String, String)> {
    names
        .iter()
        .enumerate()
        .map(|(idx, name)| (PaletteItem::PluginCommand(idx), name.clone(), String::new()))
        .collect()
}

/// Build + rank the palette's rows for the current `mode` and `query`.
/// In `Actions` mode this is the bindable-action set plus a "Change Theme…"
/// entry; in `Themes` mode it's every theme in `theme_names` (built-ins +
/// user-defined), with the active one marked; in `SshQuickConnect` mode it's
/// only the configured SSH hosts as `SSH: <name>` rows; in `Snippets` mode
/// it's only the user's configured snippets; in `CommandHistory` mode it's
/// the deduplicated list of previously run commands; in `ClipboardHistory`
/// mode it's the clipboard ring (most-recent first).
#[allow(clippy::too_many_arguments)]
pub(crate) fn palette_ranked(
    query: &str,
    mode: PaletteMode,
    sc: &terminale_config::ShortcutsConfig,
    current_theme: &str,
    theme_names: &[String],
    ssh_host_names: &[String],
    snippet_names: &[(String, String)],
    workspace_list: &[(String, std::path::PathBuf)],
    command_history: &[String],
    clipboard_history: &[String],
    dir_jump_dirs: &[String],
    failed_commands: &[(i32, String)],
    plugin_command_names: &[String],
) -> Vec<(PaletteItem, terminale_render::PaletteEntry)> {
    match mode {
        PaletteMode::Actions => {
            let mut cands: Vec<(PaletteItem, String, String)> = PALETTE_ACTIONS
                .iter()
                // Hide the "New SSH Tab…" picker entry when no hosts exist —
                // the per-host `SSH: <name>` rows below cover the useful case.
                .filter(|(a, _)| {
                    !matches!(a, ShortcutAction::NewSshTab) || !ssh_host_names.is_empty()
                })
                .map(|(a, label)| {
                    (
                        PaletteItem::Action(*a),
                        (*label).to_string(),
                        crate::binding_for(*a, sc),
                    )
                })
                .collect();
            cands.push((
                PaletteItem::OpenThemePicker,
                "Change Theme\u{2026}".to_string(),
                String::new(),
            ));
            // One direct-connect row per configured host: "SSH: <name>".
            cands.extend(ssh_host_rows(ssh_host_names));
            // Plugin-registered commands appear at the bottom of the Actions list.
            cands.extend(plugin_command_rows(plugin_command_names));
            rank_candidates(query, cands)
        }
        PaletteMode::SshQuickConnect => {
            // Scoped to the SSH hosts only — nothing else clutters the list.
            rank_candidates(query, ssh_host_rows(ssh_host_names))
        }
        PaletteMode::Themes => {
            let cands: Vec<(PaletteItem, String, String)> = theme_names
                .iter()
                .map(|name| {
                    let marker = if name == current_theme {
                        "\u{25CF} current".to_string()
                    } else {
                        String::new()
                    };
                    (PaletteItem::SetTheme(name.clone()), name.clone(), marker)
                })
                .collect();
            rank_candidates(query, cands)
        }
        PaletteMode::Snippets => {
            // Scoped to user snippets only.
            rank_candidates(query, snippet_rows(snippet_names))
        }
        PaletteMode::WorkspaceNamePrompt => {
            // No rows in the name-prompt mode — the query IS the name; the
            // palette shows an empty list with the prompt hint.
            Vec::new()
        }
        PaletteMode::WorkspacePicker => {
            // Scoped to saved workspaces only.
            rank_candidates(query, workspace_rows(workspace_list))
        }
        PaletteMode::CommandHistory => {
            // Scoped to the collected command history (deduped, newest first).
            rank_candidates(query, command_history_rows(command_history))
        }
        PaletteMode::ClipboardHistory => {
            // Scoped to the clipboard history ring (most-recent first).
            rank_candidates(query, clipboard_history_rows(clipboard_history))
        }
        PaletteMode::DirectoryJump => {
            // Scoped to the frecency-ranked directory list.
            rank_candidates(query, directory_jump_rows(dir_jump_dirs))
        }
        PaletteMode::FailedCommandPicker => {
            // Scoped to command blocks with a non-zero exit code (newest first).
            rank_candidates(query, failed_command_rows(failed_commands))
        }
    }
}

// ── open/close/refresh palette ────────────────────────────────────────────────

pub(crate) fn open_command_palette(state: &mut RunningState) {
    state.command_palette = Some(CommandPaletteState::new());
    refresh_palette(state);
    state.window.request_redraw();
}

/// Open the workspace picker sub-mode in the command palette.
pub(crate) fn open_workspace_picker(state: &mut RunningState) {
    // Refresh the cached workspace list so the picker shows the latest.
    state.workspace_list = crate::workspace::list_workspaces();
    if state.command_palette.is_none() {
        state.command_palette = Some(CommandPaletteState::new());
    }
    if let Some(p) = state.command_palette.as_mut() {
        p.mode = PaletteMode::WorkspacePicker;
        p.query.clear();
        p.selected = 0;
    }
    refresh_palette(state);
    state.window.request_redraw();
}

/// Open the command-history picker. Collects command blocks from the panes
/// matching the configured scope, deduplicates them (newest first), caches
/// the result, then opens the palette in `CommandHistory` mode.
pub(crate) fn open_command_history(state: &mut RunningState) {
    use terminale_config::CommandHistoryScope;

    let scope = state.command_history_scope;
    let max = state.command_history_max_entries;
    let active_tab = state.active_tab;

    // Collect raw command_text strings from the relevant panes (oldest first,
    // matching natural block storage order — `build_command_history` reverses).
    let mut raw: Vec<String> = Vec::new();

    match scope {
        CommandHistoryScope::CurrentPane => {
            if let Some(tab) = state.tabs.get(active_tab) {
                let emu = tab.emulator.lock();
                for b in emu.command_blocks() {
                    raw.push(b.command_text.clone());
                }
            }
        }
        CommandHistoryScope::CurrentTab => {
            if let Some(tab) = state.tabs.get(active_tab) {
                for pane in tab.panes.values() {
                    let emu = pane.emulator.lock();
                    for b in emu.command_blocks() {
                        raw.push(b.command_text.clone());
                    }
                }
            }
        }
        CommandHistoryScope::Window => {
            for tab in &state.tabs {
                for pane in tab.panes.values() {
                    let emu = pane.emulator.lock();
                    for b in emu.command_blocks() {
                        raw.push(b.command_text.clone());
                    }
                }
            }
        }
    }

    state.command_history_cache = build_command_history(raw.iter().map(String::as_str), max);

    let mut pal = CommandPaletteState::new();
    pal.mode = PaletteMode::CommandHistory;
    state.command_palette = Some(pal);
    refresh_palette(state);
    state.window.request_redraw();
}

pub(crate) fn close_palette(state: &mut RunningState) {
    state.command_palette = None;
    state.renderer.set_command_palette(None);
    state.window.request_redraw();
}

/// Open the clipboard-history picker in the command palette.
pub(crate) fn open_clipboard_history(state: &mut RunningState) {
    let mut pal = CommandPaletteState::new();
    pal.mode = PaletteMode::ClipboardHistory;
    state.command_palette = Some(pal);
    refresh_palette(state);
    state.window.request_redraw();
}

/// Open the directory-jump picker in the command palette.
///
/// Rebuilds the frecency-ranked cache from the in-memory store before
/// switching the palette into `DirectoryJump` mode, so the list reflects
/// the current visit state immediately.
pub(crate) fn open_directory_jump(state: &mut RunningState) {
    // Rebuild the ranked cache using the current wall clock.
    let now_unix = chrono::Utc::now().timestamp();
    state.dir_jump_cache = state.dir_jump_store.ranked(now_unix);

    let mut pal = CommandPaletteState::new();
    pal.mode = PaletteMode::DirectoryJump;
    state.command_palette = Some(pal);
    refresh_palette(state);
    state.window.request_redraw();
}

/// Open the failed-command picker. Collects command blocks with a non-zero
/// exit code from the active pane (newest first), caches them, then opens
/// the palette in `FailedCommandPicker` mode.
pub(crate) fn open_failed_command_picker(state: &mut RunningState) {
    let active_tab = state.active_tab;

    // Collect failed blocks from the active pane, newest first.
    let mut failed: Vec<(i32, String)> = Vec::new();
    if let Some(tab) = state.tabs.get(active_tab) {
        let emu = tab.emulator.lock();
        for b in emu.command_blocks().iter().rev() {
            if b.exit_code.is_some_and(|c| c != 0) {
                failed.push((b.prompt_line, b.command_text.clone()));
            }
        }
    }

    state.failed_command_cache = failed;
    let mut pal = CommandPaletteState::new();
    pal.mode = PaletteMode::FailedCommandPicker;
    state.command_palette = Some(pal);
    refresh_palette(state);
    state.window.request_redraw();
}

/// Recompute the ranked list for the current query and hand it to the
/// renderer, clamping the selection into range.
pub(crate) fn refresh_palette(state: &mut RunningState) {
    let Some(pal) = state.command_palette.as_ref() else {
        return;
    };
    let query = pal.query.clone();
    let mode = pal.mode;
    let sc = state.shortcuts.clone();
    let current_theme = state.theme_name.clone();
    let theme_names = state.theme_names.clone();
    let ssh_host_names = state.ssh_host_names.clone();
    let snippet_names = state.snippet_names.clone();
    let workspace_list = state.workspace_list.clone();
    let command_history = state.command_history_cache.clone();
    let clipboard_history: Vec<String> = state.clipboard_history_ring.iter().cloned().collect();
    let dir_jump_dirs = state.dir_jump_cache.clone();
    let failed_commands = state.failed_command_cache.clone();
    let plugin_command_names = state.plugin_command_names.clone();
    let ranked = palette_ranked(
        &query,
        mode,
        &sc,
        &current_theme,
        &theme_names,
        &ssh_host_names,
        &snippet_names,
        &workspace_list,
        &command_history,
        &clipboard_history,
        &dir_jump_dirs,
        &failed_commands,
        &plugin_command_names,
    );
    let entries: Vec<terminale_render::PaletteEntry> = ranked.into_iter().map(|(_, e)| e).collect();
    let selected = if entries.is_empty() {
        0
    } else {
        pal.selected.min(entries.len() - 1)
    };
    if let Some(p) = state.command_palette.as_mut() {
        p.selected = selected;
    }
    let placeholder = match mode {
        PaletteMode::Actions => {
            "Search commands \u{2014} type to filter, \u{2191}\u{2193} to move, Enter to run"
        }
        PaletteMode::SshQuickConnect => "Search SSH hosts \u{2014} Enter or click to connect",
        PaletteMode::Themes => "Search themes \u{2014} Enter or click to apply",
        PaletteMode::Snippets => "Search snippets \u{2014} Enter or click to insert",
        PaletteMode::WorkspaceNamePrompt => "Type a workspace name, then press Enter to save",
        PaletteMode::WorkspacePicker => "Search workspaces \u{2014} Enter or click to restore",
        PaletteMode::CommandHistory => {
            if command_history.is_empty() {
                "No history yet \u{2014} requires shell integration (OSC 133)"
            } else {
                "Search history \u{2014} Enter or click to load onto prompt"
            }
        }
        PaletteMode::ClipboardHistory => {
            if clipboard_history.is_empty() {
                "No clipboard history yet \u{2014} copy some text first"
            } else {
                "Search clipboard history \u{2014} Enter or click to paste"
            }
        }
        PaletteMode::DirectoryJump => {
            if dir_jump_dirs.is_empty() {
                "No directories visited yet \u{2014} requires OSC 7 shell integration"
            } else {
                "Search directories \u{2014} Enter or click to jump"
            }
        }
        PaletteMode::FailedCommandPicker => {
            if failed_commands.is_empty() {
                "No failed commands yet \u{2014} requires shell integration (OSC 133)"
            } else {
                "Search failed commands \u{2014} Enter or click to jump to block"
            }
        }
    }
    .to_string();
    state
        .renderer
        .set_command_palette(Some(terminale_render::CommandPalette {
            query,
            entries,
            selected,
            placeholder,
        }));
}

/// Move the palette selection by `dir`, wrapping around the result list.
pub(crate) fn palette_move(state: &mut RunningState, dir: i32) {
    let (query, mode) = match state.command_palette.as_ref() {
        Some(p) => (p.query.clone(), p.mode),
        None => return,
    };
    let sc = state.shortcuts.clone();
    let current_theme = state.theme_name.clone();
    let theme_names = state.theme_names.clone();
    let ssh_host_names = state.ssh_host_names.clone();
    let snippet_names = state.snippet_names.clone();
    let workspace_list = state.workspace_list.clone();
    let command_history = state.command_history_cache.clone();
    let clipboard_history: Vec<String> = state.clipboard_history_ring.iter().cloned().collect();
    let dir_jump_dirs = state.dir_jump_cache.clone();
    let failed_commands = state.failed_command_cache.clone();
    let plugin_command_names = state.plugin_command_names.clone();
    let len = palette_ranked(
        &query,
        mode,
        &sc,
        &current_theme,
        &theme_names,
        &ssh_host_names,
        &snippet_names,
        &workspace_list,
        &command_history,
        &clipboard_history,
        &dir_jump_dirs,
        &failed_commands,
        &plugin_command_names,
    )
    .len();
    if len == 0 {
        return;
    }
    if let Some(p) = state.command_palette.as_mut() {
        let cur = p.selected as i32;
        p.selected = (cur + dir).rem_euclid(len as i32) as usize;
    }
    refresh_palette(state);
}

/// Activate the palette's currently-selected row (shared by Enter and mouse
/// click). Dispatches the action / theme / SSH connect / snippet insert, or
/// drills into the theme / snippet picker.
pub(crate) fn activate_palette_selection(state: &mut RunningState) {
    let sc = state.shortcuts.clone();
    let current_theme = state.theme_name.clone();
    let theme_names = state.theme_names.clone();
    let ssh_host_names = state.ssh_host_names.clone();
    let snippet_names = state.snippet_names.clone();
    let workspace_list = state.workspace_list.clone();
    let command_history = state.command_history_cache.clone();
    let clipboard_history: Vec<String> = state.clipboard_history_ring.iter().cloned().collect();
    let dir_jump_dirs = state.dir_jump_cache.clone();
    let (query, selected, mode) = state
        .command_palette
        .as_ref()
        .map_or((String::new(), 0, PaletteMode::Actions), |p| {
            (p.query.clone(), p.selected, p.mode)
        });

    // In the workspace-name prompt mode, Enter commits whatever is in the
    // query field as the workspace name (no ranked rows to select).
    if mode == PaletteMode::WorkspaceNamePrompt {
        let name = query.trim().to_string();
        close_palette(state);
        if !name.is_empty() {
            state.pending_save_workspace = Some(name);
        }
        return;
    }

    let failed_commands = state.failed_command_cache.clone();
    let plugin_command_names = state.plugin_command_names.clone();
    let ranked = palette_ranked(
        &query,
        mode,
        &sc,
        &current_theme,
        &theme_names,
        &ssh_host_names,
        &snippet_names,
        &workspace_list,
        &command_history,
        &clipboard_history,
        &dir_jump_dirs,
        &failed_commands,
        &plugin_command_names,
    );
    let chosen = ranked.into_iter().nth(selected).map(|(item, _)| item);
    match chosen {
        Some(PaletteItem::Action(action)) => {
            close_palette(state);
            crate::dispatch_shortcut(state, action);
        }
        Some(PaletteItem::SetTheme(name)) => {
            close_palette(state);
            state.pending_theme = Some(name);
        }
        Some(PaletteItem::OpenSsh(idx)) => {
            close_palette(state);
            state.pending_ssh_host = Some(idx);
        }
        Some(PaletteItem::InsertSnippet(idx)) => {
            close_palette(state);
            state.pending_insert_snippet = Some(idx);
        }
        Some(PaletteItem::OpenNamedWorkspace(idx)) => {
            // Look up path from the cached workspace_list.
            let path = workspace_list.get(idx).map(|(_, p)| p.clone());
            close_palette(state);
            if let Some(p) = path {
                state.pending_open_workspace_path = Some(p);
            }
        }
        Some(PaletteItem::OpenThemePicker) => {
            // Drill into the theme picker without closing.
            if let Some(p) = state.command_palette.as_mut() {
                p.mode = PaletteMode::Themes;
                p.query.clear();
                p.selected = 0;
            }
            refresh_palette(state);
        }
        Some(PaletteItem::InsertCommand(cmd)) => {
            // Load the command onto the prompt for editing (no newline).
            close_palette(state);
            if !cmd.is_empty() {
                state.pending_insert_command = Some(cmd);
            }
        }
        Some(PaletteItem::PasteClipboardEntry(text)) => {
            // Paste the clipboard entry into the focused pane.
            close_palette(state);
            if !text.is_empty() {
                state.pending_paste_clipboard_entry = Some(text);
            }
        }
        Some(PaletteItem::JumpToDirectory(path)) => {
            // Send `cd '<path>'\n` to the focused pane's PTY.
            close_palette(state);
            if !path.is_empty() {
                state.pending_cd_path = Some(crate::dir_jump::build_cd_payload(&path));
            }
        }
        Some(PaletteItem::JumpToBlock(prompt_line)) => {
            // Scroll the viewport to the target block's prompt row.
            close_palette(state);
            crate::jump_to_absolute_line(state, prompt_line);
        }
        Some(PaletteItem::PluginCommand(idx)) => {
            // Enqueue invocation for the App to run on the next tick
            // (where `&mut self.plugins` is available).
            close_palette(state);
            state.pending_plugin_invoke = Some(idx);
        }
        None => {
            close_palette(state);
        }
    }
}

/// Route a key to the command palette. Returns `true` if consumed (so it
/// must not reach search / hotkeys / the PTY).
pub(crate) fn handle_palette_input(
    state: &mut RunningState,
    logical: &winit::keyboard::Key,
    text: Option<winit::keyboard::SmolStr>,
) -> bool {
    use winit::keyboard::Key;
    match logical {
        Key::Named(NamedKey::Escape) => {
            // In a sub-picker (Themes, Snippets, WorkspaceNamePrompt, or
            // WorkspacePicker), Esc backs out to the action list rather than
            // closing the palette entirely.
            let in_subpicker = state.command_palette.as_ref().is_some_and(|p| {
                matches!(
                    p.mode,
                    PaletteMode::Themes
                        | PaletteMode::Snippets
                        | PaletteMode::WorkspaceNamePrompt
                        | PaletteMode::WorkspacePicker
                        | PaletteMode::CommandHistory
                        | PaletteMode::ClipboardHistory
                        | PaletteMode::DirectoryJump
                        | PaletteMode::FailedCommandPicker
                )
            });
            if in_subpicker {
                if let Some(p) = state.command_palette.as_mut() {
                    p.mode = PaletteMode::Actions;
                    p.query.clear();
                    p.selected = 0;
                }
                refresh_palette(state);
            } else {
                close_palette(state);
            }
            return true;
        }
        Key::Named(NamedKey::Enter) => {
            activate_palette_selection(state);
            return true;
        }
        Key::Named(NamedKey::ArrowDown) => {
            palette_move(state, 1);
            return true;
        }
        Key::Named(NamedKey::ArrowUp) => {
            palette_move(state, -1);
            return true;
        }
        Key::Named(NamedKey::Tab) => {
            let dir = if state.modifiers.shift_key() { -1 } else { 1 };
            palette_move(state, dir);
            return true;
        }
        Key::Named(NamedKey::Backspace) => {
            if let Some(p) = state.command_palette.as_mut() {
                p.query.pop();
                p.selected = 0;
            }
            refresh_palette(state);
            return true;
        }
        _ => {}
    }
    // Append printable text.
    if let Some(t) = text {
        if !t.is_empty() && t.chars().all(|ch| !ch.is_control()) {
            if let Some(p) = state.command_palette.as_mut() {
                p.query.push_str(&t);
                p.selected = 0;
            }
            refresh_palette(state);
            return true;
        }
    }
    // Swallow everything else (stray modifiers etc.) while open.
    true
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_command_history: basic assembly ─────────────────────────────────

    #[test]
    fn history_empty_input_yields_empty() {
        let result = build_command_history(std::iter::empty(), 100);
        assert!(result.is_empty(), "empty input must yield empty history");
    }

    #[test]
    fn history_filters_empty_and_whitespace_only() {
        let cmds = ["", "  ", "git status", "\t", "ls"];
        let result = build_command_history(cmds.iter().copied(), 100);
        // Only the two non-blank entries must survive.
        assert_eq!(
            result.len(),
            2,
            "empty/whitespace commands must be filtered"
        );
        assert!(result.contains(&"git status".to_string()));
        assert!(result.contains(&"ls".to_string()));
    }

    #[test]
    fn history_deduplicates_keeps_newest() {
        // Oldest first as stored; "git status" appears at pos 0 and pos 2.
        // The newest occurrence (pos 2) should be kept and should appear
        // earlier in the output (index 0, since we reverse).
        let cmds = ["git status", "ls", "git status"];
        let result = build_command_history(cmds.iter().copied(), 100);
        assert_eq!(result.len(), 2, "duplicate must be removed");
        // "git status" was newest so it is first.
        assert_eq!(result[0], "git status");
        assert_eq!(result[1], "ls");
    }

    #[test]
    fn history_most_recent_first() {
        let cmds = ["a", "b", "c"];
        let result = build_command_history(cmds.iter().copied(), 100);
        assert_eq!(result, vec!["c", "b", "a"]);
    }

    #[test]
    fn history_respects_max_entries_cap() {
        let cmds: Vec<String> = (0..20).map(|i| format!("cmd_{i}")).collect();
        let result = build_command_history(cmds.iter().map(String::as_str), 5);
        assert_eq!(result.len(), 5, "result must be capped at max_entries");
        // Newest 5 are cmd_19 … cmd_15.
        assert_eq!(result[0], "cmd_19");
        assert_eq!(result[4], "cmd_15");
    }

    #[test]
    fn history_trims_leading_trailing_whitespace() {
        let cmds = ["  git status  ", "ls  "];
        let result = build_command_history(cmds.iter().copied(), 100);
        assert_eq!(result.len(), 2);
        // After trim, "git status" not "  git status  ".
        assert_eq!(result[0], "ls");
        assert_eq!(result[1], "git status");
    }

    // ── command_history_rows ──────────────────────────────────────────────────

    #[test]
    fn history_rows_produce_insert_command_items() {
        let history = vec!["git log".to_string(), "ls -la".to_string()];
        let rows = command_history_rows(&history);
        assert_eq!(rows.len(), 2);
        // Each row's PaletteItem must be InsertCommand with the correct text.
        assert!(
            matches!(&rows[0].0, PaletteItem::InsertCommand(cmd) if cmd == "git log"),
            "first row must be InsertCommand(\"git log\")"
        );
        assert!(
            matches!(&rows[1].0, PaletteItem::InsertCommand(cmd) if cmd == "ls -la"),
            "second row must be InsertCommand(\"ls -la\")"
        );
    }

    #[test]
    fn history_rows_empty_history_yields_empty_rows() {
        let rows = command_history_rows(&[]);
        assert!(rows.is_empty());
    }

    // ── fuzzy filtering of history entries ───────────────────────────────────

    #[test]
    fn history_fuzzy_filters_correctly() {
        let history = vec![
            "cargo build".to_string(),
            "git status".to_string(),
            "cargo test".to_string(),
        ];
        let rows = command_history_rows(&history);
        let ranked = rank_candidates("cargo", rows);
        // "cargo build" and "cargo test" should both match; "git status" should not.
        assert_eq!(
            ranked.len(),
            2,
            "fuzzy filter must exclude non-matching entry"
        );
        let labels: Vec<&str> = ranked.iter().map(|(_, e)| e.label.as_str()).collect();
        assert!(labels.contains(&"cargo build"));
        assert!(labels.contains(&"cargo test"));
        assert!(!labels.contains(&"git status"));
    }

    // ── insert payload semantics ──────────────────────────────────────────────

    #[test]
    fn history_insert_payload_has_no_trailing_newline() {
        // Confirm the InsertCommand variant carries the raw text, no newline.
        let item = PaletteItem::InsertCommand("git log --oneline".to_string());
        if let PaletteItem::InsertCommand(cmd) = item {
            assert!(
                !cmd.ends_with('\n'),
                "InsertCommand payload must not have a trailing newline"
            );
            assert_eq!(cmd, "git log --oneline");
        } else {
            panic!("expected InsertCommand");
        }
    }

    // ── fuzzy_score (regression guard) ───────────────────────────────────────

    #[test]
    fn fuzzy_score_empty_query_always_matches() {
        assert!(fuzzy_score("", "anything").is_some());
        assert_eq!(fuzzy_score("", "").unwrap(), 0);
    }

    #[test]
    fn fuzzy_score_non_subsequence_returns_none() {
        assert!(fuzzy_score("xyz", "abc").is_none());
    }

    #[test]
    fn fuzzy_score_prefix_scores_higher_than_interior() {
        let prefix = fuzzy_score("ca", "cargo build").unwrap();
        let interior = fuzzy_score("ca", "build cargo").unwrap();
        // "cargo build" has "ca" at the start; "build cargo" has it after a space.
        // Both match but the prefix form should score higher.
        assert!(
            prefix > interior,
            "prefix match must outscore interior match"
        );
    }

    // ── clipboard_history_rows ────────────────────────────────────────────────

    /// Rows must be PasteClipboardEntry items with the correct text payload.
    #[test]
    fn clipboard_history_rows_produce_paste_items() {
        let entries = vec!["hello world".to_string(), "token=abc123".to_string()];
        let rows = clipboard_history_rows(&entries);
        assert_eq!(rows.len(), 2);
        assert!(
            matches!(&rows[0].0, PaletteItem::PasteClipboardEntry(t) if t == "hello world"),
            "first row must be PasteClipboardEntry(\"hello world\")"
        );
        assert!(
            matches!(&rows[1].0, PaletteItem::PasteClipboardEntry(t) if t == "token=abc123"),
            "second row must be PasteClipboardEntry(\"token=abc123\")"
        );
    }

    /// Empty entry list yields empty rows.
    #[test]
    fn clipboard_history_rows_empty_yields_empty() {
        let rows = clipboard_history_rows(&[]);
        assert!(rows.is_empty());
    }

    /// Multi-line text in a clipboard entry is collapsed to a single label line.
    #[test]
    fn clipboard_history_rows_collapse_newlines_in_label() {
        let text = "line one\nline two\nline three".to_string();
        let rows = clipboard_history_rows(std::slice::from_ref(&text));
        assert_eq!(rows.len(), 1);
        // Label must not contain raw newline characters.
        assert!(
            !rows[0].1.contains('\n'),
            "label must not contain newline: {:?}",
            rows[0].1
        );
        // But the PasteClipboardEntry payload must carry the original text verbatim.
        assert!(
            matches!(&rows[0].0, PaletteItem::PasteClipboardEntry(t) if t == &text),
            "payload must be original text with newlines preserved"
        );
    }

    /// Fuzzy filter over clipboard history entries works correctly.
    #[test]
    fn clipboard_history_fuzzy_filter() {
        let entries = vec![
            "git status".to_string(),
            "/home/user/.config/terminale/config.toml".to_string(),
            "cargo build".to_string(),
        ];
        let rows = clipboard_history_rows(&entries);
        let ranked = rank_candidates("cargo", rows);
        assert_eq!(ranked.len(), 1, "only the cargo entry should match");
        assert!(matches!(&ranked[0].0, PaletteItem::PasteClipboardEntry(t) if t == "cargo build"));
    }
}
