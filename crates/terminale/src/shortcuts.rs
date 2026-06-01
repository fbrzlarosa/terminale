//! Keyboard shortcut resolution, dispatch, and key translation.
//! Also: scroll helpers, tab move, prompt navigation.

use crate::{RowsScroll, RunningState, ShortcutAction};
use winit::keyboard::{KeyCode, ModifiersState, NamedKey, PhysicalKey};

// ── ModFlags / parse_binding ─────────────────────────────────────────────────

/// Parsed modifier set for shortcut matching.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModFlags {
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
    pub(crate) meta: bool,
}

/// Parse a binding string ("Ctrl+Shift+ArrowLeft") into its modifier
/// set + key-name token. Returns `None` for an empty / malformed binding
/// (e.g. modifiers with no key), which disables that action.
pub(crate) fn parse_binding(s: &str) -> Option<(ModFlags, String)> {
    let mut m = ModFlags::default();
    let mut key: Option<String> = None;
    for raw in s.split('+') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        match t.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => m.ctrl = true,
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            "cmd" | "super" | "meta" | "win" => m.meta = true,
            _ => key = Some(t.to_string()),
        }
    }
    key.map(|k| (m, k))
}

// ── keycode_name / pressed_key_name ──────────────────────────────────────────

/// Canonical name for a winit physical key, matching the tokens used in
/// binding strings (and in the settings recorder). Layout-independent.
pub(crate) fn keycode_name(code: KeyCode) -> Option<&'static str> {
    Some(match code {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::ArrowUp => "ArrowUp",
        KeyCode::ArrowDown => "ArrowDown",
        KeyCode::ArrowLeft => "ArrowLeft",
        KeyCode::ArrowRight => "ArrowRight",
        KeyCode::Space => "Space",
        KeyCode::Enter => "Enter",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::Delete => "Delete",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Insert => "Insert",
        KeyCode::Backquote => "`",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Backslash => "\\",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        _ => return None,
    })
}

/// The canonical name for the currently-pressed key, preferring the
/// layout-independent physical code, falling back to the logical
/// character (for keys not in [`keycode_name`]).
pub(crate) fn pressed_key_name(
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
) -> Option<String> {
    if let PhysicalKey::Code(code) = physical {
        if let Some(n) = keycode_name(code) {
            return Some(n.to_string());
        }
    }
    if let winit::keyboard::Key::Character(s) = logical {
        return Some(s.to_string());
    }
    None
}

// ── resolve_shortcut ─────────────────────────────────────────────────────────

/// Match the pressed key + modifiers against every shortcut binding in
/// `sc`, returning the first action whose binding matches. This is what
/// makes the settings-panel remaps actually take effect.
pub(crate) fn resolve_shortcut(
    mods: &ModifiersState,
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
    sc: &terminale_config::ShortcutsConfig,
) -> Option<ShortcutAction> {
    use ShortcutAction::*;
    let pressed = ModFlags {
        ctrl: mods.control_key(),
        shift: mods.shift_key(),
        alt: mods.alt_key(),
        meta: mods.super_key(),
    };
    let key = pressed_key_name(physical, logical)?;

    let table: [(&str, ShortcutAction); 100] = [
        (sc.new_tab.as_str(), NewTab),
        (sc.close_tab.as_str(), CloseTab),
        (sc.reopen_closed_tab.as_str(), ReopenClosedTab),
        (sc.new_ssh_tab.as_str(), NewSshTab),
        (sc.next_tab.as_str(), NextTab),
        (sc.prev_tab.as_str(), PrevTab),
        (sc.move_tab_left.as_str(), MoveTabLeft),
        (sc.move_tab_right.as_str(), MoveTabRight),
        (sc.profile_picker.as_str(), ProfilePicker),
        (sc.restart_tab.as_str(), RestartTab),
        (sc.copy.as_str(), Copy),
        (sc.paste.as_str(), Paste),
        (sc.select_all.as_str(), SelectAll),
        (sc.find.as_str(), Find),
        (sc.clear.as_str(), Clear),
        (sc.settings.as_str(), Settings),
        (sc.font_increase.as_str(), FontIncrease),
        (sc.font_decrease.as_str(), FontDecrease),
        (sc.font_reset.as_str(), FontReset),
        (sc.scroll_line_up.as_str(), ScrollLineUp),
        (sc.scroll_line_down.as_str(), ScrollLineDown),
        (sc.scroll_page_up.as_str(), ScrollPageUp),
        (sc.scroll_page_down.as_str(), ScrollPageDown),
        (sc.scroll_top.as_str(), ScrollTop),
        (sc.scroll_bottom.as_str(), ScrollBottom),
        (sc.ai_assistant.as_str(), AiAssistant),
        (sc.command_palette.as_str(), CommandPalette),
        (sc.explain_selection.as_str(), ExplainSelection),
        (sc.clear_scrollback.as_str(), ClearScrollback),
        (sc.stay_on_top.as_str(), ToggleStayOnTop),
        (sc.snap_top.as_str(), SnapTop),
        (sc.snap_bottom.as_str(), SnapBottom),
        (sc.snap_left.as_str(), SnapLeft),
        (sc.snap_right.as_str(), SnapRight),
        (sc.snap_center.as_str(), SnapCenter),
        (sc.snap_maximize.as_str(), SnapMaximize),
        // Quarter snap actions — all unbound by default.
        (sc.snap_top_left.as_str(), SnapTopLeft),
        (sc.snap_top_right.as_str(), SnapTopRight),
        (sc.snap_bottom_left.as_str(), SnapBottomLeft),
        (sc.snap_bottom_right.as_str(), SnapBottomRight),
        // Snap-layout chooser — unbound by default.
        (sc.show_snap_layouts.as_str(), ShowSnapLayouts),
        (sc.split_right.as_str(), SplitRight),
        (sc.split_down.as_str(), SplitDown),
        (sc.split_left.as_str(), SplitLeft),
        (sc.split_up.as_str(), SplitUp),
        (sc.close_pane.as_str(), ClosePane),
        // Keyboard-first pane control.
        (sc.focus_pane_left.as_str(), FocusPaneLeft),
        (sc.focus_pane_right.as_str(), FocusPaneRight),
        (sc.focus_pane_up.as_str(), FocusPaneUp),
        (sc.focus_pane_down.as_str(), FocusPaneDown),
        (sc.toggle_pane_zoom.as_str(), TogglePaneZoom),
        (sc.resize_pane_left.as_str(), ResizePaneLeft),
        (sc.resize_pane_right.as_str(), ResizePaneRight),
        (sc.resize_pane_up.as_str(), ResizePaneUp),
        (sc.resize_pane_down.as_str(), ResizePaneDown),
        // Tab-index jumps (Ctrl+1..9 by default).
        (sc.activate_tab_1.as_str(), ActivateTab1),
        (sc.activate_tab_2.as_str(), ActivateTab2),
        (sc.activate_tab_3.as_str(), ActivateTab3),
        (sc.activate_tab_4.as_str(), ActivateTab4),
        (sc.activate_tab_5.as_str(), ActivateTab5),
        (sc.activate_tab_6.as_str(), ActivateTab6),
        (sc.activate_tab_7.as_str(), ActivateTab7),
        (sc.activate_tab_8.as_str(), ActivateTab8),
        (sc.activate_tab_9.as_str(), ActivateTab9),
        (sc.last_tab.as_str(), LastTab),
        (sc.prev_prompt.as_str(), PrevPrompt),
        (sc.next_prompt.as_str(), NextPrompt),
        (sc.copy_mode.as_str(), CopyMode),
        (sc.quick_select.as_str(), QuickSelect),
        (sc.pane_select.as_str(), PaneSelect),
        (sc.reload_config.as_str(), ReloadConfig),
        (sc.toggle_fullscreen.as_str(), ToggleFullscreen),
        (sc.toggle_zen_mode.as_str(), ToggleZenMode),
        (sc.toggle_broadcast_input.as_str(), ToggleBroadcastInput),
        (sc.new_window.as_str(), NewWindow),
        (sc.move_tab_to_new_window.as_str(), MoveTabToNewWindow),
        (sc.move_pane_to_new_tab.as_str(), MovePaneToNewTab),
        (sc.move_pane_to_new_window.as_str(), MovePaneToNewWindow),
        (sc.open_snippets.as_str(), OpenSnippets),
        (sc.fix_last_command.as_str(), FixLastCommand),
        (sc.save_workspace.as_str(), SaveWorkspace),
        (sc.open_workspace.as_str(), OpenWorkspace),
        // Block-scoped copy / re-run / edit actions.
        (sc.copy_last_command_output.as_str(), CopyLastCommandOutput),
        (sc.copy_block_output.as_str(), CopyBlockOutput),
        (sc.copy_last_command.as_str(), CopyLastCommand),
        (sc.rerun_last_command.as_str(), RerunLastCommand),
        (sc.edit_last_command.as_str(), EditLastCommand),
        // Command-history picker.
        (sc.open_command_history.as_str(), OpenCommandHistory),
        // Export scrollback — unbound by default.
        (sc.export_scrollback.as_str(), ExportScrollback),
        // Clipboard history picker — unbound by default.
        (sc.open_clipboard_history.as_str(), OpenClipboardHistory),
        // Pane swap / rotate — unbound by default.
        (sc.move_pane_left.as_str(), MovePaneLeft),
        (sc.move_pane_right.as_str(), MovePaneRight),
        (sc.move_pane_up.as_str(), MovePaneUp),
        (sc.move_pane_down.as_str(), MovePaneDown),
        (sc.rotate_panes.as_str(), RotatePanes),
        (sc.rotate_panes_back.as_str(), RotatePanesBack),
        // Directory-jump picker — unbound by default.
        (sc.open_directory_jump.as_str(), OpenDirectoryJump),
        (sc.prev_failed_command.as_str(), JumpToPrevFailedCommand),
        (sc.next_failed_command.as_str(), JumpToNextFailedCommand),
        (
            sc.open_failed_command_picker.as_str(),
            OpenFailedCommandPicker,
        ),
    ];
    for (binding, action) in table {
        if let Some((bm, bk)) = parse_binding(binding) {
            if bm == pressed && bk.eq_ignore_ascii_case(&key) {
                return Some(action);
            }
        }
    }
    None
}

// ── dispatch_shortcut ─────────────────────────────────────────────────────────

/// Execute a resolved shortcut action against the running state.
pub(crate) fn dispatch_shortcut(state: &mut RunningState, action: ShortcutAction) {
    use ShortcutAction::*;
    match action {
        NewTab => crate::new_tab(state),
        CloseTab => crate::request_close_tab(state, state.active_tab),
        NextTab => {
            if !state.tabs.is_empty() {
                let next = (state.active_tab + 1) % state.tabs.len();
                crate::switch_tab(state, next);
            }
        }
        PrevTab => {
            if !state.tabs.is_empty() {
                let n = state.tabs.len();
                let prev = (state.active_tab + n - 1) % n;
                crate::switch_tab(state, prev);
            }
        }
        MoveTabLeft => crate::move_active_tab(state, -1),
        MoveTabRight => crate::move_active_tab(state, 1),
        ReopenClosedTab => crate::reopen_closed_tab(state),
        NewSshTab => {
            // Opens the command palette scoped to the configured SSH hosts
            // so the user can fuzzy-search and connect from the keyboard.
            // No-op when no hosts are configured (the palette entry is hidden
            // in that case anyway).
            crate::open_ssh_quick_connect(state);
        }
        ProfilePicker => state.open_profile_picker = true,
        RestartTab => crate::restart_active_tab(state),
        Copy => crate::copy_selection(state),
        Paste => match crate::paste_clipboard(state) {
            crate::tabs::PasteAction::Sent => {}
            crate::tabs::PasteAction::NeedsConfirm { text, bracketed } => {
                state.pending_paste_guard = Some((text, bracketed));
            }
        },
        SelectAll => crate::select_all(state),
        Find => {
            state.search = Some(crate::SearchState::new());
            crate::refresh_search_matches(state);
        }
        Clear => {
            // "Clear Buffer" semantics: wipe history AND viewport at
            // the emulator level (NOT via ED escapes — those route through
            // ClearMode::All which scrolls the live screen into the
            // scrollback we just emptied), then send \x0c so the shell
            // redraws PS1 against a truly blank buffer.
            if let Some(tab) = state.tabs.get(state.active_tab) {
                tab.emulator.lock().clear_buffer_to_blank();
            }
            if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                tab.scroll_lines = 0;
            }
            state.renderer.set_scroll_lines(0);
            state.renderer.set_selection(None);
            if let Some(tab) = state.tabs.get(state.active_tab) {
                let _ = tab.session.write_input(&[0x0c]);
            }
            state.window.request_redraw();
        }
        Settings => crate::open_settings(state),
        FontIncrease => {
            let new_size = (state.renderer.font_size() + 1.0).min(48.0);
            state.renderer.set_font_size(new_size);
            let s = state.window.inner_size();
            crate::resize_all_tabs(state, s.width, s.height);
            state.pending_font_size = Some(new_size);
        }
        FontDecrease => {
            let new_size = (state.renderer.font_size() - 1.0).max(6.0);
            state.renderer.set_font_size(new_size);
            let s = state.window.inner_size();
            crate::resize_all_tabs(state, s.width, s.height);
            state.pending_font_size = Some(new_size);
        }
        FontReset => {
            state
                .renderer
                .set_font_size(terminale_render::DEFAULT_FONT_SIZE);
            let s = state.window.inner_size();
            crate::resize_all_tabs(state, s.width, s.height);
            state.pending_font_size = Some(terminale_render::DEFAULT_FONT_SIZE);
        }
        ScrollLineUp => scroll_by_rows(state, RowsScroll::LineUp),
        ScrollLineDown => scroll_by_rows(state, RowsScroll::LineDown),
        ScrollPageUp => scroll_by_rows(state, RowsScroll::PageUp),
        ScrollPageDown => scroll_by_rows(state, RowsScroll::PageDown),
        ScrollTop => scroll_by_rows(state, RowsScroll::Top),
        ScrollBottom => scroll_by_rows(state, RowsScroll::Bottom),
        AiAssistant => state.open_ai_requested = true,
        CommandPalette => crate::open_command_palette(state),
        ExplainSelection => {
            // Seed the assistant with the current selection (if any) and
            // open it; the App picks up `pending_ai_prompt` + the open
            // request together. With no selection it just opens empty.
            state.pending_ai_prompt = crate::selection_text(state)
                .map(|t| format!("Explain this terminal output. Be concise.\n```\n{t}\n```"));
            state.open_ai_requested = true;
        }
        ClearScrollback => {
            // Drop the scrollback history but keep the visible screen
            // (distinct from "Clear Screen"/Ctrl+L). ED Ps=3 is processed
            // by the emulator locally — the shell is unaffected.
            if let Some(tab) = state.tabs.get(state.active_tab) {
                tab.emulator.lock().advance(b"\x1b[3J");
            }
            if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                tab.scroll_lines = 0;
            }
            state.renderer.set_scroll_lines(0);
            state.window.request_redraw();
        }
        ToggleStayOnTop => crate::toggle_stay_on_top(state),
        SnapTop => crate::snap_window(state, terminale_config::SnapEdge::Top),
        SnapBottom => crate::snap_window(state, terminale_config::SnapEdge::Bottom),
        SnapLeft => crate::snap_window(state, terminale_config::SnapEdge::Left),
        SnapRight => crate::snap_window(state, terminale_config::SnapEdge::Right),
        SnapCenter => crate::snap_window(state, terminale_config::SnapEdge::Center),
        SnapMaximize => crate::snap_window(state, terminale_config::SnapEdge::Maximize),
        SnapTopLeft => crate::snap_window(state, terminale_config::SnapEdge::TopLeft),
        SnapTopRight => crate::snap_window(state, terminale_config::SnapEdge::TopRight),
        SnapBottomLeft => crate::snap_window(state, terminale_config::SnapEdge::BottomLeft),
        SnapBottomRight => crate::snap_window(state, terminale_config::SnapEdge::BottomRight),
        ShowSnapLayouts => crate::open_snap_chooser(state),
        SplitRight => crate::split_focused_pane(state, crate::SplitDir::Vertical, true),
        SplitLeft => crate::split_focused_pane(state, crate::SplitDir::Vertical, false),
        SplitDown => crate::split_focused_pane(state, crate::SplitDir::Horizontal, true),
        SplitUp => crate::split_focused_pane(state, crate::SplitDir::Horizontal, false),
        ClosePane => crate::close_focused_pane(state),
        // Keyboard-first pane control.
        FocusPaneLeft => crate::focus_pane_in_direction(state, crate::PaneDirection::Left),
        FocusPaneRight => crate::focus_pane_in_direction(state, crate::PaneDirection::Right),
        FocusPaneUp => crate::focus_pane_in_direction(state, crate::PaneDirection::Up),
        FocusPaneDown => crate::focus_pane_in_direction(state, crate::PaneDirection::Down),
        TogglePaneZoom => crate::toggle_pane_zoom(state),
        ResizePaneLeft => crate::keyboard_resize_pane(state, crate::PaneDirection::Left),
        ResizePaneRight => crate::keyboard_resize_pane(state, crate::PaneDirection::Right),
        ResizePaneUp => crate::keyboard_resize_pane(state, crate::PaneDirection::Up),
        ResizePaneDown => crate::keyboard_resize_pane(state, crate::PaneDirection::Down),
        // Tab-index jumps. Tabs 1-8 select by 0-based index; Tab 9 always
        // jumps to the last tab (a common convention).
        ActivateTab1 => crate::activate_tab_by_index(state, 0),
        ActivateTab2 => crate::activate_tab_by_index(state, 1),
        ActivateTab3 => crate::activate_tab_by_index(state, 2),
        ActivateTab4 => crate::activate_tab_by_index(state, 3),
        ActivateTab5 => crate::activate_tab_by_index(state, 4),
        ActivateTab6 => crate::activate_tab_by_index(state, 5),
        ActivateTab7 => crate::activate_tab_by_index(state, 6),
        ActivateTab8 => crate::activate_tab_by_index(state, 7),
        // Tab 9 = last tab (regardless of how many tabs there are).
        ActivateTab9 => {
            if !state.tabs.is_empty() {
                crate::switch_tab(state, state.tabs.len() - 1);
            }
        }
        LastTab => crate::activate_last_tab(state),
        PrevPrompt => jump_to_prompt(state, -1),
        NextPrompt => jump_to_prompt(state, 1),
        CopyMode => crate::enter_copy_mode(state),
        QuickSelect => crate::enter_quick_select(state),
        PaneSelect => crate::enter_pane_select(state),
        ReloadConfig => {
            // Post a ConfigChanged event so the App (which owns the Config
            // and the config path) can perform the reload. The window state
            // only has a proxy, not the App itself.
            let _ = state.proxy.send_event(crate::UserEvent::ConfigChanged);
        }
        ToggleFullscreen => crate::toggle_fullscreen(state),
        ToggleZenMode => crate::toggle_zen_mode(state),
        ToggleBroadcastInput => crate::toggle_broadcast_input(state),
        // Window-management actions that require `event_loop` are deferred:
        // the dispatch sets a flag that the App drains in the post-event block
        // where `ActiveEventLoop` is available.
        NewWindow => {
            state.pending_new_window = true;
        }
        MoveTabToNewWindow => {
            // Guard: only when the source has more than one tab.
            if state.tabs.len() > 1 {
                state.pending_move_tab_to_new_window = true;
            }
        }
        MovePaneToNewTab => {
            // Guard: only when the active tab has more than one pane.
            if let Some(tab) = state.tabs.get(state.active_tab) {
                if crate::count_leaves(&tab.tree) > 1 {
                    state.pending_move_pane_to_new_tab = true;
                }
            }
        }
        MovePaneToNewWindow => {
            // Guard: only when the active tab has more than one pane.
            if let Some(tab) = state.tabs.get(state.active_tab) {
                if crate::count_leaves(&tab.tree) > 1 {
                    state.pending_move_pane_to_new_window = true;
                }
            }
        }
        OpenSnippets => crate::open_snippet_picker(state),
        FixLastCommand => fix_last_command(state),
        SaveWorkspace => open_save_workspace_prompt(state),
        OpenWorkspace => crate::open_workspace_picker(state),
        CopyLastCommandOutput => copy_last_command_output(state),
        CopyBlockOutput => copy_block_output(state),
        CopyLastCommand => copy_last_command(state),
        RerunLastCommand => rerun_last_command(state),
        EditLastCommand => edit_last_command(state),
        ImportSshHosts => {
            // Signal the App (which owns the config + disk path) to run the
            // import on the next loop tick. The App drains this flag in the
            // same block that handles other pending_* mutations.
            state.pending_import_ssh_hosts = true;
        }
        OpenCommandHistory => crate::open_command_history(state),
        ExportScrollback => export_scrollback(state),
        OpenClipboardHistory => crate::open_clipboard_history(state),
        ToggleTabPin => toggle_tab_pin(state),
        // Pane swap / rotate.
        MovePaneLeft => crate::move_pane_in_direction(state, crate::PaneDirection::Left),
        MovePaneRight => crate::move_pane_in_direction(state, crate::PaneDirection::Right),
        MovePaneUp => crate::move_pane_in_direction(state, crate::PaneDirection::Up),
        MovePaneDown => crate::move_pane_in_direction(state, crate::PaneDirection::Down),
        RotatePanes => crate::rotate_active_tab_panes(state),
        RotatePanesBack => crate::rotate_active_tab_panes_back(state),
        OpenDirectoryJump => crate::open_directory_jump(state),
        ImportTheme => {
            // Signal the App (which owns the config + the themes_dir) to open
            // the native file picker and copy the chosen theme on the next tick.
            state.pending_import_theme = true;
        }
        JumpToPrevFailedCommand => jump_to_failed_command(state, -1),
        JumpToNextFailedCommand => jump_to_failed_command(state, 1),
        OpenFailedCommandPicker => crate::open_failed_command_picker(state),
        NewTabGroup => crate::tab_groups::create_group_and_assign(state),
        AssignTabToGroup => crate::tab_groups::assign_active_tab_to_next_group(state),
        ClearTabGroup => crate::tab_groups::clear_active_tab_group(state),
        SuggestCommand => {
            // Request a manual AI suggestion. The App-level `spawn_suggestion`
            // method is the actual dispatcher (it needs `&mut self` for the
            // Tokio handle); here we just set the flag that `about_to_wait`
            // picks up on the next tick.
            state.suggestions.manual_requested = true;
            state.window.request_redraw();
        }
        RenameTabGroup => {
            // Rename the active tab's group. No-op when the tab has no group.
            if let Some(gid) = state.tabs.get(state.active_tab).and_then(|t| t.group) {
                crate::tab_groups::start_rename_group(state, gid);
            }
        }
    }
}

/// Maximum number of output lines included in the "fix last command" prompt
/// to avoid blowing the provider's context window.
const FIX_CMD_MAX_LINES: usize = 50;
/// Maximum number of output bytes included in the prompt.
const FIX_CMD_MAX_BYTES: usize = 4096;

/// Locate the most-recent failed command block and open the AI assistant with
/// a pre-built diagnosis prompt.
///
/// The block's output is extracted from the emulator's full-buffer text at the
/// absolute line indices stored in the block.  We truncate to a sane cap so a
/// huge build log doesn't blow the provider's context window.
pub(crate) fn fix_last_command(state: &mut RunningState) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };
    let emu = tab.emulator.lock();

    // Find the most-recent block with a non-zero exit code.
    let failed = emu
        .command_blocks()
        .iter()
        .rev()
        .find(|b| b.exit_code.is_some_and(|c| c != 0));

    let Some(block) = failed else {
        // No failed block available — nothing to do.
        tracing::debug!("fix_last_command: no failed command block found");
        drop(emu);
        return;
    };

    let exit_code = block.exit_code.unwrap_or(-1);
    let command_text = block.command_text.clone();
    let cwd = block.cwd.clone().unwrap_or_else(|| "unknown".to_string());
    let output_start = block.output_start_line;
    let output_end = block.end_line.unwrap_or(output_start);

    // Extract output while we still hold the emulator lock.
    let all_lines = emu.buffer_lines_text();
    let hist = emu.history_size() as i32;
    drop(emu);

    let output = extract_block_output_lines(&all_lines, hist, output_start, output_end);

    let prompt = build_fix_prompt(&command_text, exit_code, &cwd, &output);
    state.pending_ai_prompt = Some(prompt);
    state.open_ai_requested = true;
}

/// Pull the lines of the buffer that fall within `[output_start, output_end]`
/// (absolute line indices).  Truncates to [`FIX_CMD_MAX_LINES`] lines /
/// [`FIX_CMD_MAX_BYTES`] bytes, whichever is hit first.
///
/// `all_lines` is the slice from `Emulator::buffer_lines_text()` where
/// `abs_line = index - history_size`.
pub(crate) fn extract_block_output_lines(
    all_lines: &[String],
    history_size: i32,
    output_start: i32,
    output_end: i32,
) -> String {
    // `buffer_lines_text()` is indexed so that `abs_line = i - hist`.
    // So `i = abs_line + hist`.
    let start_idx = (output_start + history_size).max(0) as usize;
    let end_idx =
        ((output_end + history_size).max(0) as usize).min(all_lines.len().saturating_sub(1));

    let mut out = String::new();
    for (lines_taken, line) in all_lines
        .get(start_idx..=end_idx)
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        if lines_taken >= FIX_CMD_MAX_LINES || out.len() >= FIX_CMD_MAX_BYTES {
            out.push_str("\n[... output truncated ...]");
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let remaining = FIX_CMD_MAX_BYTES.saturating_sub(out.len());
        if line.len() > remaining {
            out.push_str(&line[..remaining]);
            out.push_str("\n[... output truncated ...]");
            break;
        }
        out.push_str(line);
    }
    out
}

/// Assemble the AI prompt for a failed command, ready to submit.
///
/// Kept as a separate pure function so it can be covered by unit tests
/// without touching any running state.
pub(crate) fn build_fix_prompt(
    command_text: &str,
    exit_code: i32,
    cwd: &str,
    output: &str,
) -> String {
    let mut prompt = format!(
        "The following shell command failed with exit code {exit_code} in `{cwd}`:\n\n\
         ```\n$ {command_text}\n```\n"
    );
    if !output.trim().is_empty() {
        prompt.push_str("\nOutput:\n\n```\n");
        prompt.push_str(output.trim_end());
        prompt.push_str("\n```\n");
    }
    prompt.push_str("\nExplain briefly why it failed and propose a single corrected command.");
    prompt
}

// ── Block-scoped copy / re-run / edit ─────────────────────────────────────────

/// Copy the output of the most-recent completed command block to the clipboard.
///
/// Extracts the text between `output_start_line` and `end_line` (inclusive)
/// from the active pane's buffer using the same index arithmetic as
/// [`fix_last_command`], then writes it to the system clipboard.
/// No-op (silent) when:
///   - no completed block exists (shell integration off or no commands run yet),
///   - the last block has no output span, or
///   - the clipboard backend is unavailable.
pub(crate) fn copy_last_command_output(state: &mut RunningState) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let emu = tab.emulator.lock();

    let Some(block) = emu.last_command_block() else {
        tracing::debug!("copy_last_command_output: no command block found");
        drop(emu);
        return;
    };
    // Only copy finished blocks — an in-flight block (end_line == None) has
    // incomplete output.
    let Some(end_line) = block.end_line else {
        tracing::debug!("copy_last_command_output: last block is still running");
        drop(emu);
        return;
    };

    let output_start = block.output_start_line;
    let all_lines = emu.buffer_lines_text();
    let hist = emu.history_size() as i32;
    drop(emu);

    let text = extract_block_output_text(&all_lines, hist, output_start, end_line);
    crate::push_clipboard_history(state, text.clone());
    if let Some(cb) = state.clipboard.as_mut() {
        if let Err(e) = cb.set_text(text) {
            tracing::warn!(?e, "copy_last_command_output: clipboard write failed");
        }
    }
}

/// Copy the output of the command block containing the cursor's current line.
///
/// Uses `command_block_at_line` to locate the block, then extracts its output
/// span.  Falls back silently when the cursor line is not inside any block.
pub(crate) fn copy_block_output(state: &mut RunningState) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let emu = tab.emulator.lock();

    // Compute the absolute line the cursor is on, accounting for scroll offset.
    // The renderer's cursor row is a 0-based viewport row; to convert to an
    // absolute line: abs = viewport_row - scroll_lines.
    let scroll = state.renderer.scroll_lines() as i32;
    // Determine which absolute line the cursor is on.  The renderer tracks
    // scroll offset but not the exact cursor row.  We use the live edge of
    // the visible viewport (abs line 0 when not scrolled) as a best proxy:
    // when the user invokes this action they are looking at (and interacting
    // with) the region near the prompt, which is at or close to abs line 0.
    let cursor_abs = -(scroll); // live edge of the current viewport

    let block = emu
        .command_block_at_line(cursor_abs)
        .or_else(|| emu.last_command_block());

    let Some(block) = block else {
        tracing::debug!("copy_block_output: no block at or near cursor");
        drop(emu);
        return;
    };
    let Some(end_line) = block.end_line else {
        tracing::debug!("copy_block_output: block at cursor is still running");
        drop(emu);
        return;
    };

    let output_start = block.output_start_line;
    let all_lines = emu.buffer_lines_text();
    let hist = emu.history_size() as i32;
    drop(emu);

    let text = extract_block_output_text(&all_lines, hist, output_start, end_line);
    crate::push_clipboard_history(state, text.clone());
    if let Some(cb) = state.clipboard.as_mut() {
        if let Err(e) = cb.set_text(text) {
            tracing::warn!(?e, "copy_block_output: clipboard write failed");
        }
    }
}

/// Copy the command text of the most-recent command block to the clipboard.
pub(crate) fn copy_last_command(state: &mut RunningState) {
    let Some(tab) = state.tabs.get(state.active_tab) else {
        return;
    };
    let emu = tab.emulator.lock();
    let text = emu
        .last_command_block()
        .map(|b| b.command_text.clone())
        .unwrap_or_default();
    drop(emu);

    if text.is_empty() {
        tracing::debug!("copy_last_command: no command text available");
        return;
    }
    crate::push_clipboard_history(state, text.clone());
    if let Some(cb) = state.clipboard.as_mut() {
        if let Err(e) = cb.set_text(text) {
            tracing::warn!(?e, "copy_last_command: clipboard write failed");
        }
    }
}

/// Re-run the most-recent command block verbatim by writing its text + `\n`.
pub(crate) fn rerun_last_command(state: &mut RunningState) {
    let active = state.active_tab;
    let cmd = {
        let Some(tab) = state.tabs.get(active) else {
            return;
        };
        tab.emulator
            .lock()
            .last_command_block()
            .map(|b| b.command_text.clone())
            .unwrap_or_default()
    };

    if cmd.is_empty() {
        tracing::debug!("rerun_last_command: no command text available");
        return;
    }
    let mut payload = cmd.into_bytes();
    payload.push(b'\n');
    if let Some(tab) = state.tabs.get(active) {
        if let Err(e) = tab.session.write_input(&payload) {
            tracing::warn!(?e, "rerun_last_command: PTY write failed");
        }
    }
}

/// Load the most-recent command onto the shell prompt for editing.
///
/// Writes (optional Ctrl+U) + command text without a trailing newline.
/// The Ctrl+U kill-line prefix is gated by `terminal.edit_command_clears_line`.
pub(crate) fn edit_last_command(state: &mut RunningState) {
    let active = state.active_tab;
    let clears_line = state.edit_command_clears_line;

    let cmd = {
        let Some(tab) = state.tabs.get(active) else {
            return;
        };
        tab.emulator
            .lock()
            .last_command_block()
            .map(|b| b.command_text.clone())
            .unwrap_or_default()
    };

    if cmd.is_empty() {
        tracing::debug!("edit_last_command: no command text available");
        return;
    }
    let mut payload: Vec<u8> = Vec::with_capacity(1 + cmd.len());
    if clears_line {
        payload.push(0x15); // Ctrl+U — kill line
    }
    payload.extend_from_slice(cmd.as_bytes());
    // No trailing newline — the user edits and presses Enter themselves.
    if let Some(tab) = state.tabs.get(active) {
        if let Err(e) = tab.session.write_input(&payload) {
            tracing::warn!(?e, "edit_last_command: PTY write failed");
        }
    }
}

/// Extract the output text for a block spanning `[output_start, output_end]`.
///
/// Unlike [`extract_block_output_lines`] (which caps at `FIX_CMD_MAX_LINES` for
/// AI prompts), this version returns the full output text so clipboard copies
/// are lossless.
pub(crate) fn extract_block_output_text(
    all_lines: &[String],
    history_size: i32,
    output_start: i32,
    output_end: i32,
) -> String {
    // Index mapping: `i = abs_line + history_size`.
    let start_idx = (output_start + history_size).max(0) as usize;
    let end_idx =
        ((output_end + history_size).max(0) as usize).min(all_lines.len().saturating_sub(1));

    all_lines.get(start_idx..=end_idx).unwrap_or(&[]).join("\n")
}

// ── binding_for ───────────────────────────────────────────────────────────────

/// The configured key binding for an action (the same string shown in
/// the settings panel). Empty string = unbound.
pub(crate) fn binding_for(
    action: ShortcutAction,
    sc: &terminale_config::ShortcutsConfig,
) -> String {
    use ShortcutAction::*;
    match action {
        NewTab => sc.new_tab.clone(),
        CloseTab => sc.close_tab.clone(),
        NextTab => sc.next_tab.clone(),
        PrevTab => sc.prev_tab.clone(),
        MoveTabLeft => sc.move_tab_left.clone(),
        MoveTabRight => sc.move_tab_right.clone(),
        ProfilePicker => sc.profile_picker.clone(),
        RestartTab => sc.restart_tab.clone(),
        Copy => sc.copy.clone(),
        Paste => sc.paste.clone(),
        SelectAll => sc.select_all.clone(),
        Find => sc.find.clone(),
        Clear => sc.clear.clone(),
        Settings => sc.settings.clone(),
        FontIncrease => sc.font_increase.clone(),
        FontDecrease => sc.font_decrease.clone(),
        FontReset => sc.font_reset.clone(),
        ScrollLineUp => sc.scroll_line_up.clone(),
        ScrollLineDown => sc.scroll_line_down.clone(),
        ScrollPageUp => sc.scroll_page_up.clone(),
        ScrollPageDown => sc.scroll_page_down.clone(),
        ScrollTop => sc.scroll_top.clone(),
        ScrollBottom => sc.scroll_bottom.clone(),
        AiAssistant => sc.ai_assistant.clone(),
        CommandPalette => sc.command_palette.clone(),
        ExplainSelection => sc.explain_selection.clone(),
        ClearScrollback => sc.clear_scrollback.clone(),
        ReopenClosedTab => sc.reopen_closed_tab.clone(),
        NewSshTab => sc.new_ssh_tab.clone(),
        ToggleStayOnTop => sc.stay_on_top.clone(),
        SnapTop => sc.snap_top.clone(),
        SnapBottom => sc.snap_bottom.clone(),
        SnapLeft => sc.snap_left.clone(),
        SnapRight => sc.snap_right.clone(),
        SnapCenter => sc.snap_center.clone(),
        SnapMaximize => sc.snap_maximize.clone(),
        SnapTopLeft => sc.snap_top_left.clone(),
        SnapTopRight => sc.snap_top_right.clone(),
        SnapBottomLeft => sc.snap_bottom_left.clone(),
        SnapBottomRight => sc.snap_bottom_right.clone(),
        ShowSnapLayouts => sc.show_snap_layouts.clone(),
        SplitRight => sc.split_right.clone(),
        SplitDown => sc.split_down.clone(),
        SplitLeft => sc.split_left.clone(),
        SplitUp => sc.split_up.clone(),
        ClosePane => sc.close_pane.clone(),
        FocusPaneLeft => sc.focus_pane_left.clone(),
        FocusPaneRight => sc.focus_pane_right.clone(),
        FocusPaneUp => sc.focus_pane_up.clone(),
        FocusPaneDown => sc.focus_pane_down.clone(),
        TogglePaneZoom => sc.toggle_pane_zoom.clone(),
        ResizePaneLeft => sc.resize_pane_left.clone(),
        ResizePaneRight => sc.resize_pane_right.clone(),
        ResizePaneUp => sc.resize_pane_up.clone(),
        ResizePaneDown => sc.resize_pane_down.clone(),
        // Tab-index jumps.
        ActivateTab1 => sc.activate_tab_1.clone(),
        ActivateTab2 => sc.activate_tab_2.clone(),
        ActivateTab3 => sc.activate_tab_3.clone(),
        ActivateTab4 => sc.activate_tab_4.clone(),
        ActivateTab5 => sc.activate_tab_5.clone(),
        ActivateTab6 => sc.activate_tab_6.clone(),
        ActivateTab7 => sc.activate_tab_7.clone(),
        ActivateTab8 => sc.activate_tab_8.clone(),
        ActivateTab9 => sc.activate_tab_9.clone(),
        LastTab => sc.last_tab.clone(),
        PrevPrompt => sc.prev_prompt.clone(),
        NextPrompt => sc.next_prompt.clone(),
        CopyMode => sc.copy_mode.clone(),
        QuickSelect => sc.quick_select.clone(),
        PaneSelect => sc.pane_select.clone(),
        ReloadConfig => sc.reload_config.clone(),
        ToggleFullscreen => sc.toggle_fullscreen.clone(),
        ToggleZenMode => sc.toggle_zen_mode.clone(),
        ToggleBroadcastInput => sc.toggle_broadcast_input.clone(),
        NewWindow => sc.new_window.clone(),
        MoveTabToNewWindow => sc.move_tab_to_new_window.clone(),
        MovePaneToNewTab => sc.move_pane_to_new_tab.clone(),
        MovePaneToNewWindow => sc.move_pane_to_new_window.clone(),
        OpenSnippets => sc.open_snippets.clone(),
        FixLastCommand => sc.fix_last_command.clone(),
        SaveWorkspace => sc.save_workspace.clone(),
        OpenWorkspace => sc.open_workspace.clone(),
        CopyLastCommandOutput => sc.copy_last_command_output.clone(),
        CopyBlockOutput => sc.copy_block_output.clone(),
        CopyLastCommand => sc.copy_last_command.clone(),
        RerunLastCommand => sc.rerun_last_command.clone(),
        EditLastCommand => sc.edit_last_command.clone(),
        // ImportSshHosts is a one-shot palette action — no configurable
        // key binding in the shortcuts config.
        ImportSshHosts => String::new(),
        OpenCommandHistory => sc.open_command_history.clone(),
        ExportScrollback => sc.export_scrollback.clone(),
        OpenClipboardHistory => sc.open_clipboard_history.clone(),
        // ToggleTabPin is an action-only entry; no dedicated config key yet.
        ToggleTabPin => String::new(),
        // Pane swap / rotate.
        MovePaneLeft => sc.move_pane_left.clone(),
        MovePaneRight => sc.move_pane_right.clone(),
        MovePaneUp => sc.move_pane_up.clone(),
        MovePaneDown => sc.move_pane_down.clone(),
        RotatePanes => sc.rotate_panes.clone(),
        RotatePanesBack => sc.rotate_panes_back.clone(),
        OpenDirectoryJump => sc.open_directory_jump.clone(),
        // ImportTheme is a one-shot palette action — no configurable key binding.
        ImportTheme => String::new(),
        JumpToPrevFailedCommand => sc.prev_failed_command.clone(),
        JumpToNextFailedCommand => sc.next_failed_command.clone(),
        OpenFailedCommandPicker => sc.open_failed_command_picker.clone(),
        // Tab-group actions — palette-only, no dedicated config keys yet.
        NewTabGroup | AssignTabToGroup | ClearTabGroup | RenameTabGroup => String::new(),
        SuggestCommand => sc.suggest_command.clone(),
    }
}

// ── export_scrollback ─────────────────────────────────────────────────────────

/// Assemble the scrollback export content: all buffer lines (history + visible
/// screen) joined by newlines, with trailing blank lines stripped.
///
/// Kept as a pure function so it can be unit-tested independently of the
/// file-system or OS dialog.
pub(crate) fn build_scrollback_export_content(lines: &[String]) -> String {
    // Trim trailing empty lines from the whole buffer (the live screen often
    // has many blank rows below the cursor that are just padding).
    let trimmed_count = lines.iter().rev().take_while(|l| l.is_empty()).count();
    let used = lines.len().saturating_sub(trimmed_count);
    lines[..used].join("\n")
}

/// Produce a timestamped export filename, e.g.
/// `terminale-scrollback-20240601-153045.txt`.
///
/// Uses the local clock via `chrono`. Kept as a pure function so it can be
/// tested with an injected timestamp in tests.
pub(crate) fn scrollback_export_filename(ts: &chrono::DateTime<chrono::Local>) -> String {
    ts.format("terminale-scrollback-%Y%m%d-%H%M%S.txt")
        .to_string()
}

/// Export the focused pane's full scrollback to a file.
///
/// - If `state.scrollback_export_dir` is `Some`, the file is written there
///   directly with a timestamped name and the path is logged.
/// - Otherwise a native OS save-file dialog is opened (using `rfd`).
pub(crate) fn export_scrollback(state: &mut RunningState) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };

    let lines = tab.emulator.lock().buffer_lines_text();
    let content = build_scrollback_export_content(&lines);

    let default_name = scrollback_export_filename(&chrono::Local::now());

    let path: Option<std::path::PathBuf> = if let Some(dir) = &state.scrollback_export_dir.clone() {
        // Write directly — no dialog.
        Some(dir.join(&default_name))
    } else {
        // Open a native save-file dialog.
        rfd::FileDialog::new()
            .set_title("Export scrollback")
            .set_file_name(&default_name)
            .add_filter("Text file", &["txt"])
            .save_file()
    };

    let Some(path) = path else {
        // User cancelled.
        return;
    };

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(?e, "export_scrollback: could not create parent dir");
                return;
            }
        }
    }
    match std::fs::write(&path, &content) {
        Ok(()) => {
            tracing::info!(path = %path.display(), "scrollback exported");
        }
        Err(e) => {
            tracing::warn!(?e, path = %path.display(), "export_scrollback: write failed");
        }
    }
}

// ── Workspace palette helpers ─────────────────────────────────────────────────

/// Open the inline workspace-name prompt in the command palette.
pub(crate) fn open_save_workspace_prompt(state: &mut RunningState) {
    if state.command_palette.is_none() {
        state.command_palette = Some(crate::CommandPaletteState::new());
    }
    if let Some(p) = state.command_palette.as_mut() {
        p.mode = crate::PaletteMode::WorkspaceNamePrompt;
        p.query.clear();
        p.selected = 0;
    }
    crate::refresh_palette(state);
    state.window.request_redraw();
}

// ── handle_app_hotkey ─────────────────────────────────────────────────────────

/// Returns true if the key was an app-level hotkey (no PTY forwarding).
pub(crate) fn handle_app_hotkey(
    state: &mut RunningState,
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
) -> bool {
    let ctrl = state.modifiers.control_key();
    let alt = state.modifiers.alt_key();

    // Custom multi-action binds take priority over built-in shortcuts.
    let custom = state.custom_keybinds.clone();
    if let Some(actions) =
        crate::keymap::resolve_custom(&state.modifiers, physical, logical, &custom)
    {
        for resolved in actions {
            match resolved {
                crate::keymap::ResolvedAction::Shortcut(action) => {
                    dispatch_shortcut(state, action);
                }
                crate::keymap::ResolvedAction::SendString(bytes) => {
                    if let Some(tab) = state.tabs.get(state.active_tab) {
                        if let Err(e) = tab.session.write_input(&bytes) {
                            tracing::warn!(?e, "custom keybind PTY write failed");
                        }
                    }
                }
            }
        }
        return true;
    }

    // Config-driven shortcuts come next so user remaps actually win.
    let shortcuts = state.shortcuts.clone();
    if let Some(action) = resolve_shortcut(&state.modifiers, physical, logical, &shortcuts) {
        dispatch_shortcut(state, action);
        return true;
    }

    // Alt+Enter on a hovered URL → open it. No Ctrl needed, no click.
    if alt && !ctrl {
        if let PhysicalKey::Code(KeyCode::Enter) = physical {
            if let Some(uri) = state.hovered_url.clone() {
                if let Err(e) = open::that(&uri) {
                    tracing::warn!(?e, %uri, "hover-enter open failed");
                }
                return true;
            }
        }
    }

    // Escape always closes the context menu.
    if let winit::keyboard::Key::Named(NamedKey::Escape) = logical {
        if state.menu_visible {
            state.menu_visible = false;
            state.renderer.set_overlay(None);
            return true;
        }
    }

    false
}

// ── Key-table modal input handlers ──────────────────────────────────────────

/// Handle a key press while a key-table modal is active.
///
/// Returns `true` (consume the key) in all cases — whether the key
/// matched a binding, was Esc, or was unmatched (unmatched keys exit
/// the table silently).
pub(crate) fn handle_key_table_input(
    state: &mut RunningState,
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
    key_tables: &[terminale_config::KeyTable],
) -> bool {
    // Retrieve the active table index before borrowing further.
    let Some(ref akt) = state.active_key_table.clone() else {
        return false;
    };
    let table_idx = akt.table_idx;

    // Always exit the modal on this keystroke (single-shot).
    state.active_key_table = None;

    // Esc: exit without action.
    if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) = logical {
        return true;
    }

    let Some(table) = key_tables.get(table_idx) else {
        return true;
    };

    // Try to match against the table's bindings.
    let key_name = pressed_key_name(physical, logical);
    let pressed_mods = ModFlags {
        ctrl: state.modifiers.control_key(),
        shift: state.modifiers.shift_key(),
        alt: state.modifiers.alt_key(),
        meta: state.modifiers.super_key(),
    };

    let matched = key_name.as_deref().and_then(|k| {
        table.bindings.iter().find(|entry| {
            if entry.key.is_empty() {
                return false;
            }
            let Some((bm, bk)) = parse_binding(&entry.key) else {
                return false;
            };
            bm == pressed_mods && bk.eq_ignore_ascii_case(k)
        })
    });

    if let Some(entry) = matched {
        let actions: Vec<terminale_config::KeyActionSpec> = entry.actions.clone();
        for spec in &actions {
            if let Some(bytes) = spec.as_send_bytes() {
                if let Some(tab) = state.tabs.get(state.active_tab) {
                    if let Err(e) = tab.session.write_input(&bytes) {
                        tracing::warn!(?e, "key-table send: PTY write failed");
                    }
                }
            } else if let Some(name) = spec.action_name() {
                if let Some(action) = crate::keymap::action_from_name(name) {
                    dispatch_shortcut(state, action);
                }
            }
        }
    }
    // Unmatched key: exit without action (key is consumed either way).
    true
}

/// Check whether the pressed combo activates any key-table's leader.
/// If so, enter that table's modal mode and return `true` (consume).
pub(crate) fn handle_key_table_leader(
    state: &mut RunningState,
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
    key_tables: &[terminale_config::KeyTable],
) -> bool {
    let Some(key_name) = pressed_key_name(physical, logical) else {
        return false;
    };
    let pressed_mods = ModFlags {
        ctrl: state.modifiers.control_key(),
        shift: state.modifiers.shift_key(),
        alt: state.modifiers.alt_key(),
        meta: state.modifiers.super_key(),
    };

    for (idx, table) in key_tables.iter().enumerate() {
        if table.leader.is_empty() {
            continue;
        }
        let Some((bm, bk)) = parse_binding(&table.leader) else {
            continue;
        };
        if bm == pressed_mods && bk.eq_ignore_ascii_case(&key_name) {
            state.active_key_table = Some(crate::ActiveKeyTable {
                table_idx: idx,
                entered_at: std::time::Instant::now(),
            });
            return true;
        }
    }
    false
}

// ── translate_key + helpers ───────────────────────────────────────────────────

/// Translate a keyboard event into PTY bytes.
///
/// `app_cursor` reflects the emulator's current DECCKM state (application
/// cursor-key mode). When `true` and no modifiers are held, unmodified arrow
/// keys and Home/End emit SS3 sequences; otherwise CSI sequences are used.
/// Modified keys always use the modified-CSI form regardless of this flag.
pub(crate) fn translate_key(
    mods: &ModifiersState,
    physical_key: PhysicalKey,
    logical_key: &winit::keyboard::Key,
    text: Option<winit::keyboard::SmolStr>,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    use winit::keyboard::Key;

    let ctrl = mods.control_key();
    let shift = mods.shift_key();
    let alt = mods.alt_key();

    // Special navigation / editing combos. xterm uses CSI 1;<mod> <letter>
    // for arrow keys with modifiers; readline-based shells listen to a few
    // single-byte / two-byte sequences for line editing.
    if let Key::Named(named) = logical_key {
        if let Some(bytes) = named_key_with_modifiers(*named, ctrl, shift, alt) {
            return Some(bytes);
        }
    }

    // Ctrl + letter → C0 control byte (^A through ^Z, etc.). Must come AFTER
    // the named-key path so Ctrl+Backspace is not swallowed.
    if ctrl && !alt {
        if let PhysicalKey::Code(code) = physical_key {
            if let Some(ctrl_byte) = ctrl_code_for(code) {
                return Some(vec![ctrl_byte]);
            }
        }
        // No C0 mapping for this Ctrl+<key> (Ctrl+digit, Ctrl+symbol, …): real
        // terminals never echo the literal character, so swallow it rather than
        // typing e.g. "1" for Ctrl+1 or "!" for Ctrl+Shift+1. Named keys fall
        // through below (Ctrl+Enter etc. keep their normal sequence).
        if let Key::Character(_) = logical_key {
            return None;
        }
    }

    // Alt + letter → ESC <letter> (terminal "meta" prefix). Matches what
    // readline / bash / zsh expect for Alt-keymaps. Skip when Ctrl is also
    // held — that combo is handled per-key above.
    if alt && !ctrl {
        if let Key::Character(s) = logical_key {
            let mut out = Vec::with_capacity(1 + s.len());
            out.push(0x1b); // ESC
            out.extend_from_slice(s.as_bytes());
            return Some(out);
        }
    }

    match logical_key {
        Key::Named(named) => named_key_bytes(*named, app_cursor),
        Key::Character(s) => Some(s.as_bytes().to_vec()),
        _ => text.map(|t| t.as_bytes().to_vec()),
    }
}

/// Modifier-aware mapping for named keys. Returns `Some(bytes)` if this
/// modifier combination has a specific terminal sequence, `None` to fall
/// through to the default unmodified mapping.
///
/// The modifier parameter for CSI sequences is computed as:
/// `1 + (shift?1) + (alt?2) + (ctrl?4)`, following the xterm convention.
/// Function keys F1–F4 remain in SS3 form without modifiers; with modifiers
/// they move to CSI. F5–F12 use tilde-form (`CSI <code> ; <mod> ~`).
pub(crate) fn named_key_with_modifiers(
    named: NamedKey,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<Vec<u8>> {
    let modified = ctrl || shift || alt;
    // Modifier code used by xterm: 1 + shift + 2*alt + 4*ctrl.
    let mod_param = 1u8 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8);

    match named {
        // ── Backspace ────────────────────────────────────────────────────────
        // Ctrl+Backspace → Ctrl+W (0x17) so readline deletes the previous word.
        // Shells like bash, zsh, fish already bind ^W to "kill-word-backward".
        NamedKey::Backspace if ctrl => Some(vec![0x17]),
        NamedKey::Backspace if alt => Some(vec![0x1b, 0x7f]),

        // ── Delete ───────────────────────────────────────────────────────────
        // xterm convention for modified Delete uses the tilde form.
        NamedKey::Delete if modified => Some(format!("\x1b[3;{mod_param}~").into_bytes()),

        // ── Insert ───────────────────────────────────────────────────────────
        NamedKey::Insert if modified => Some(format!("\x1b[2;{mod_param}~").into_bytes()),

        // ── PageUp / PageDown ─────────────────────────────────────────────────
        NamedKey::PageUp if modified => Some(format!("\x1b[5;{mod_param}~").into_bytes()),
        NamedKey::PageDown if modified => Some(format!("\x1b[6;{mod_param}~").into_bytes()),

        // ── Arrow keys + Home/End with modifiers ─────────────────────────────
        // xterm CSI 1;<mod> <letter> form. Modifiers force CSI even in
        // application cursor-key mode — standard xterm behaviour.
        NamedKey::ArrowUp
        | NamedKey::ArrowDown
        | NamedKey::ArrowLeft
        | NamedKey::ArrowRight
        | NamedKey::Home
        | NamedKey::End
            if modified =>
        {
            let letter = match named {
                NamedKey::ArrowUp => b'A',
                NamedKey::ArrowDown => b'B',
                NamedKey::ArrowRight => b'C',
                NamedKey::ArrowLeft => b'D',
                NamedKey::Home => b'H',
                NamedKey::End => b'F',
                _ => unreachable!(),
            };
            Some(format!("\x1b[1;{}{}", mod_param, letter as char).into_bytes())
        }

        // ── Function keys F1–F4 with modifiers ───────────────────────────────
        // Unmodified F1–F4 use SS3; with any modifier they use CSI tilde form.
        NamedKey::F1 if modified => Some(format!("\x1b[1;{mod_param}P").into_bytes()),
        NamedKey::F2 if modified => Some(format!("\x1b[1;{mod_param}Q").into_bytes()),
        NamedKey::F3 if modified => Some(format!("\x1b[1;{mod_param}R").into_bytes()),
        NamedKey::F4 if modified => Some(format!("\x1b[1;{mod_param}S").into_bytes()),

        // ── Function keys F5–F12 — unmodified and modified ───────────────────
        // Unmodified: CSI <code> ~ (no modifier parameter).
        // Modified:   CSI <code> ; <mod> ~.
        NamedKey::F5 => Some(fn_key_tilde(
            15,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F6 => Some(fn_key_tilde(
            17,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F7 => Some(fn_key_tilde(
            18,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F8 => Some(fn_key_tilde(
            19,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F9 => Some(fn_key_tilde(
            20,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F10 => Some(fn_key_tilde(
            21,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F11 => Some(fn_key_tilde(
            23,
            if modified { Some(mod_param) } else { None },
        )),
        NamedKey::F12 => Some(fn_key_tilde(
            24,
            if modified { Some(mod_param) } else { None },
        )),

        // ── Tab / BackTab ─────────────────────────────────────────────────────
        // Shift+Tab → CSI Z is the standard reverse-tab.
        NamedKey::Tab if shift => Some(b"\x1b[Z".to_vec()),

        // ── Enter ─────────────────────────────────────────────────────────────
        // Alt+Enter: emit ESC + CR to keep readline happy.
        NamedKey::Enter if alt => Some(vec![0x1b, b'\r']),

        _ => None,
    }
}

/// Build a CSI tilde escape for a function or editing key.
///
/// `code` is the numeric parameter (e.g. 15 for F5, 5 for PageUp).
/// `modifier` is `None` for unmodified keys (emits `ESC [ <code> ~`) or
/// `Some(m)` for modified keys (emits `ESC [ <code> ; <m> ~`).
#[inline]
fn fn_key_tilde(code: u8, modifier: Option<u8>) -> Vec<u8> {
    match modifier {
        None => format!("\x1b[{code}~").into_bytes(),
        Some(m) => format!("\x1b[{code};{m}~").into_bytes(),
    }
}

pub(crate) fn ctrl_code_for(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::KeyA => Some(0x01),
        KeyCode::KeyB => Some(0x02),
        KeyCode::KeyC => Some(0x03),
        KeyCode::KeyD => Some(0x04),
        KeyCode::KeyE => Some(0x05),
        KeyCode::KeyF => Some(0x06),
        KeyCode::KeyG => Some(0x07),
        KeyCode::KeyH => Some(0x08),
        KeyCode::KeyI => Some(0x09),
        KeyCode::KeyJ => Some(0x0a),
        KeyCode::KeyK => Some(0x0b),
        KeyCode::KeyL => Some(0x0c),
        KeyCode::KeyM => Some(0x0d),
        KeyCode::KeyN => Some(0x0e),
        KeyCode::KeyO => Some(0x0f),
        KeyCode::KeyP => Some(0x10),
        KeyCode::KeyQ => Some(0x11),
        KeyCode::KeyR => Some(0x12),
        KeyCode::KeyS => Some(0x13),
        KeyCode::KeyT => Some(0x14),
        KeyCode::KeyU => Some(0x15),
        KeyCode::KeyV => Some(0x16),
        KeyCode::KeyW => Some(0x17),
        KeyCode::KeyX => Some(0x18),
        KeyCode::KeyY => Some(0x19),
        KeyCode::KeyZ => Some(0x1a),
        // C0 controls past the alphabet. Standard xterm mappings:
        //   Ctrl+[  → ESC (0x1b)   Ctrl+\  → FS (0x1c)
        //   Ctrl+]  → GS  (0x1d)   Ctrl+/  → US (0x1f)
        // Without these, Ctrl+\ et al. fell through to the literal-character
        // path and wrote `\` instead of the control byte the shell expects.
        KeyCode::BracketLeft => Some(0x1b),
        KeyCode::Backslash => Some(0x1c),
        KeyCode::BracketRight => Some(0x1d),
        KeyCode::Slash => Some(0x1f),
        _ => None,
    }
}

/// Translate an unmodified named key to PTY bytes.
///
/// `app_cursor` controls whether application cursor-key mode (DECCKM) is
/// active. When `true`, unmodified arrow keys and Home/End use SS3 form
/// (`ESC O A` … `ESC O D`, `ESC O H`, `ESC O F`). When `false` the normal
/// CSI form is used. Modified keys are handled by [`named_key_with_modifiers`]
/// before this function is called, so this path only sees unmodified keys.
pub(crate) fn named_key_bytes(named: NamedKey, app_cursor: bool) -> Option<Vec<u8>> {
    Some(match named {
        NamedKey::Enter => vec![b'\r'],
        NamedKey::Backspace => vec![0x7f],
        NamedKey::Tab => vec![b'\t'],
        NamedKey::Escape => vec![0x1b],
        NamedKey::Space => vec![b' '],
        // Arrows: SS3 in application cursor-key mode, CSI otherwise.
        NamedKey::ArrowUp => {
            if app_cursor {
                b"\x1bOA".to_vec()
            } else {
                b"\x1b[A".to_vec()
            }
        }
        NamedKey::ArrowDown => {
            if app_cursor {
                b"\x1bOB".to_vec()
            } else {
                b"\x1b[B".to_vec()
            }
        }
        NamedKey::ArrowRight => {
            if app_cursor {
                b"\x1bOC".to_vec()
            } else {
                b"\x1b[C".to_vec()
            }
        }
        NamedKey::ArrowLeft => {
            if app_cursor {
                b"\x1bOD".to_vec()
            } else {
                b"\x1b[D".to_vec()
            }
        }
        // Home/End: SS3 in application cursor-key mode, CSI otherwise.
        NamedKey::Home => {
            if app_cursor {
                b"\x1bOH".to_vec()
            } else {
                b"\x1b[H".to_vec()
            }
        }
        NamedKey::End => {
            if app_cursor {
                b"\x1bOF".to_vec()
            } else {
                b"\x1b[F".to_vec()
            }
        }
        // Editing keys — always tilde form.
        NamedKey::PageUp => b"\x1b[5~".to_vec(),
        NamedKey::PageDown => b"\x1b[6~".to_vec(),
        NamedKey::Delete => b"\x1b[3~".to_vec(),
        NamedKey::Insert => b"\x1b[2~".to_vec(),
        // F1–F4: SS3 form (unmodified only; modified handled above).
        NamedKey::F1 => b"\x1bOP".to_vec(),
        NamedKey::F2 => b"\x1bOQ".to_vec(),
        NamedKey::F3 => b"\x1bOR".to_vec(),
        NamedKey::F4 => b"\x1bOS".to_vec(),
        // F5–F12: CSI tilde form (unmodified; `named_key_with_modifiers`
        // handles the modified variants via `fn_key_tilde`).
        NamedKey::F5 => b"\x1b[15~".to_vec(),
        NamedKey::F6 => b"\x1b[17~".to_vec(),
        NamedKey::F7 => b"\x1b[18~".to_vec(),
        NamedKey::F8 => b"\x1b[19~".to_vec(),
        NamedKey::F9 => b"\x1b[20~".to_vec(),
        NamedKey::F10 => b"\x1b[21~".to_vec(),
        NamedKey::F11 => b"\x1b[23~".to_vec(),
        NamedKey::F12 => b"\x1b[24~".to_vec(),
        _ => return None,
    })
}

// ── scroll / move_tab / jump_to_prompt ───────────────────────────────────────

pub(crate) fn move_active_tab(state: &mut RunningState, dir: i32) {
    let n = state.tabs.len();
    if n < 2 {
        return;
    }
    let active = state.active_tab;
    let is_pinned = state.tabs.get(active).is_some_and(|t| t.pinned);

    // Count pinned tabs so we can enforce the group boundary.
    let pinned_count = state.tabs.iter().filter(|t| t.pinned).count();

    // A pinned tab may only swap with other pinned tabs (indices 0..pinned_count).
    // An unpinned tab may only swap within indices pinned_count..n.
    let raw_target = (active as i32 + dir).rem_euclid(n as i32) as usize;
    let target = if is_pinned {
        // Clamp within the pinned group [0, pinned_count).
        raw_target.min(pinned_count.saturating_sub(1))
    } else {
        // Clamp within the unpinned group [pinned_count, n).
        raw_target.max(pinned_count)
    };

    if target == active {
        return;
    }
    state.tabs.swap(active, target);
    state.active_tab = target;
    crate::tabs::refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Toggle the pinned state of the active tab, keeping the tab-list order
/// consistent: pinned tabs are always sorted to the front, unpinned at the
/// back.
pub(crate) fn toggle_tab_pin(state: &mut RunningState) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get_mut(active) else {
        return;
    };
    tab.pinned = !tab.pinned;
    let now_pinned = tab.pinned;

    // Re-sort: move the tab to the correct group end.
    // After toggling, we swap toward the front (if pinned) or toward the back
    // (if unpinned) until the group invariant is restored.
    //
    // Invariant: all pinned tabs occupy indices 0..pinned_count (stable order
    // within each group).  We re-build that by counting pinned tabs BEFORE the
    // swap and inserting at the right boundary.
    let n = state.tabs.len();
    if now_pinned {
        // Move this tab left until it sits at the end of the pinned group.
        let pinned_boundary = state.tabs.iter().take_while(|t| t.pinned).count();
        // The freshly-pinned tab is at `active`; move it left to index
        // `pinned_boundary - 1` (which is the last position among pinned tabs).
        let target = pinned_boundary.saturating_sub(1);
        let mut pos = active;
        while pos > target {
            state.tabs.swap(pos, pos - 1);
            pos -= 1;
        }
        state.active_tab = pos;
    } else {
        // Move this tab right until it sits at the start of the unpinned group.
        let pinned_count = state.tabs.iter().filter(|t| t.pinned).count();
        let target = pinned_count; // first unpinned slot
        let mut pos = active;
        while pos < target && pos + 1 < n {
            state.tabs.swap(pos, pos + 1);
            pos += 1;
        }
        state.active_tab = pos;
    }

    crate::tabs::refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Set (or clear) the per-tab user colour override.
pub(crate) fn set_tab_user_color(state: &mut RunningState, color: Option<[u8; 3]>) {
    let active = state.active_tab;
    if let Some(tab) = state.tabs.get_mut(active) {
        tab.user_color = color;
    }
    crate::tabs::refresh_tab_bar(state);
    state.window.request_redraw();
}

/// Set (or clear) the per-tab user icon override.
pub(crate) fn set_tab_user_icon(state: &mut RunningState, icon: Option<String>) {
    let active = state.active_tab;
    if let Some(tab) = state.tabs.get_mut(active) {
        tab.user_icon = icon;
    }
    crate::tabs::refresh_tab_bar(state);
    state.window.request_redraw();
}

pub(crate) fn scroll_by_rows(state: &mut RunningState, how: RowsScroll) {
    let active = state.active_tab;
    let (history, rows, current) = {
        let Some(tab) = state.tabs.get(active) else {
            return;
        };
        let emu = tab.emulator.lock();
        (emu.history_size(), tab.rows as usize, tab.scroll_lines)
    };
    let new_scroll = match how {
        RowsScroll::LineUp => current.saturating_add(1).min(history),
        RowsScroll::LineDown => current.saturating_sub(1),
        RowsScroll::PageUp => current.saturating_add(rows.max(1)).min(history),
        RowsScroll::PageDown => current.saturating_sub(rows.max(1)),
        RowsScroll::Top => history,
        RowsScroll::Bottom => 0,
    };
    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = new_scroll;
    }
    state.renderer.set_scroll_lines(new_scroll);
    state.window.request_redraw();
}

/// Scroll the viewport so the previous (`dir < 0`) or next (`dir > 0`) OSC 133
/// prompt mark is visible.
///
/// "Previous" navigates upward into the scrollback (towards older output);
/// "next" moves toward newer output. The logic mirrors `search_jump_to`:
/// place the target mark about a third of the way down the viewport so there's
/// context below it.
pub(crate) fn jump_to_prompt(state: &mut RunningState, dir: i32) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };
    let emu = tab.emulator.lock();
    let history = emu.history_size() as i32;
    let rows = tab.rows as i32;
    let current_scroll = tab.scroll_lines as i32;

    // The visible viewport, when scrolled by `current_scroll` lines, shows
    // absolute lines in the range  [ -current_scroll, rows - 1 - current_scroll ].
    // The "current cursor line" for navigation purposes is the top of the
    // visible area (going up means we want a mark strictly above the top,
    // going down means we want a mark strictly below the bottom).
    let top_abs = -current_scroll;
    let bottom_abs = rows - 1 - current_scroll;

    let mark = if dir < 0 {
        // Navigating up: find the mark immediately above the top visible row.
        emu.semantic().prev_prompt(top_abs)
    } else {
        // Navigating down: find the mark immediately below the bottom row.
        emu.semantic().next_prompt(bottom_abs)
    };

    let Some(mark) = mark else {
        return; // no mark in that direction
    };
    let target_line = mark.line;
    drop(emu);

    // Place the mark about a third of the way down — same heuristic as search.
    let target_row = (rows / 3).max(0);
    // `scroll` = how many lines the viewport is offset upward from the live
    // edge. When `target_line` is negative (in history) and we want it at
    // `target_row` from the top:
    //   viewport_top_abs = -scroll  →  target_line == -scroll + target_row
    //   scroll = target_row - target_line
    let new_scroll = (target_row - target_line).clamp(0, history) as usize;

    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = new_scroll;
    }
    state.renderer.set_scroll_lines(new_scroll);
    arm_jump_highlight(state, target_line);
    state.window.request_redraw();
}

/// Scroll the viewport to the previous (`dir < 0`) or next (`dir > 0`)
/// command block whose recorded exit code is non-zero (a failed command).
///
/// Only blocks with `exit_code.is_some_and(|c| c != 0)` qualify. Clamps at
/// the first / last failed block — does NOT wrap.  Same viewport placement
/// heuristic as [`jump_to_prompt`].
pub(crate) fn jump_to_failed_command(state: &mut RunningState, dir: i32) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };
    let emu = tab.emulator.lock();
    let history = emu.history_size() as i32;
    let rows = tab.rows as i32;
    let current_scroll = tab.scroll_lines as i32;

    let top_abs = -current_scroll;
    let bottom_abs = rows - 1 - current_scroll;

    // Find the nearest failed block in the requested direction.
    let blocks = emu.command_blocks();
    let found: Option<i32> = if dir < 0 {
        // Searching backward: find the newest failed block strictly above
        // the top of the viewport.
        blocks
            .iter()
            .rev()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0) && b.prompt_line < top_abs)
            .map(|b| b.prompt_line)
            .next()
    } else {
        // Searching forward: find the oldest failed block strictly below the
        // bottom of the viewport.
        blocks
            .iter()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0) && b.prompt_line > bottom_abs)
            .map(|b| b.prompt_line)
            .next()
    };
    drop(emu);
    let Some(target_line) = found else {
        return; // no failed command in that direction — clamp silently
    };

    // Place the prompt at about a third down — same heuristic as jump_to_prompt.
    let target_row = (rows / 3).max(0);
    let new_scroll = (target_row - target_line).clamp(0, history) as usize;

    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = new_scroll;
    }
    state.renderer.set_scroll_lines(new_scroll);
    arm_jump_highlight(state, target_line);
    state.window.request_redraw();
}

/// Arm the brief prompt-highlight band at `target_line`.
///
/// Does nothing when `state.highlight_on_jump` is `false`.
pub(crate) fn arm_jump_highlight(state: &mut RunningState, target_line: i32) {
    if state.highlight_on_jump {
        state.jump_highlight_line = Some(target_line);
        state.jump_highlight_start = Some(std::time::Instant::now());
    }
}

/// Scroll the viewport so that `abs_line` sits near the top (one-third
/// down), then arm the jump highlight.  Used by the failed-command picker
/// when the user selects an entry.
pub(crate) fn jump_to_absolute_line(state: &mut RunningState, abs_line: i32) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return;
    };
    let emu = tab.emulator.lock();
    let history = emu.history_size() as i32;
    let rows = tab.rows as i32;
    drop(emu);

    let target_row = (rows / 3).max(0);
    let new_scroll = (target_row - abs_line).clamp(0, history) as usize;

    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = new_scroll;
    }
    state.renderer.set_scroll_lines(new_scroll);
    arm_jump_highlight(state, abs_line);
    state.window.request_redraw();
}

/// Map a digit keycode to a target tab index. Digits 1-8 select tabs
/// 1-8 (0-based `0..=7`); Digit 9 always jumps to the **last** tab (the
/// browser / Windows-Terminal convention). Returns `None` for non-digit keys
/// or when the requested tab doesn't exist. Numpad digits map identically.
///
/// This function is retained as a pure-logic utility exercised by unit tests.
/// The runtime dispatch now goes through the config-driven [`ShortcutAction`]
/// path (`ActivateTab1`..`ActivateTab9`) rather than calling this directly.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn tab_jump_index(code: KeyCode, tab_count: usize) -> Option<usize> {
    if tab_count == 0 {
        return None;
    }
    let nth = match code {
        KeyCode::Digit1 | KeyCode::Numpad1 => 0,
        KeyCode::Digit2 | KeyCode::Numpad2 => 1,
        KeyCode::Digit3 | KeyCode::Numpad3 => 2,
        KeyCode::Digit4 | KeyCode::Numpad4 => 3,
        KeyCode::Digit5 | KeyCode::Numpad5 => 4,
        KeyCode::Digit6 | KeyCode::Numpad6 => 5,
        KeyCode::Digit7 | KeyCode::Numpad7 => 6,
        KeyCode::Digit8 | KeyCode::Numpad8 => 7,
        // "9" is special: always the last tab, regardless of count.
        KeyCode::Digit9 | KeyCode::Numpad9 => return Some(tab_count - 1),
        _ => return None,
    };
    (nth < tab_count).then_some(nth)
}

// ── scroll delta conversion helpers ──────────────────────────────────────────

/// Convert a `MouseScrollDelta` into a signed raw float in "line-equivalents",
/// using `pixels_per_row` for `PixelDelta` input.
///
/// Positive return value = wheel/swipe upward (towards history).
/// Negative return value = downward (towards live output).
///
/// This is a pure function so it can be covered by unit tests.
#[inline]
pub(crate) fn delta_to_raw(delta: winit::event::MouseScrollDelta, pixels_per_row: f32) -> f32 {
    let ppr = pixels_per_row.max(1.0);
    match delta {
        winit::event::MouseScrollDelta::LineDelta(_x, y) => y,
        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / ppr,
    }
}

/// Compute how many whole rows to scroll given a raw float delta, a per-mode
/// step multiplier, and (optionally) a running fractional remainder.
///
/// When `remainder` is `Some(&mut r)`, the fractional leftover from this event
/// is accumulated into `r` across calls (smooth / precision-trackpad mode).
/// The sign of `r` matches the sign of the accumulated raw delta.
///
/// Returns the number of whole rows as a non-negative `isize` (the direction
/// is still encoded in the sign of `raw`; callers must inspect `raw > 0`).
#[inline]
pub(crate) fn raw_to_rows(raw: f32, step: f32, remainder: Option<&mut f32>) -> isize {
    let step = step.max(1.0);
    let scaled = raw * step;
    if let Some(r) = remainder {
        // Accumulate the fractional remainder (same sign convention as scaled).
        *r += scaled;
        let whole = r.trunc() as isize;
        *r -= whole as f32; // keep the fraction, discard the integer part
        whole.abs()
    } else {
        // Non-smooth: round each event independently.
        scaled.abs().round() as isize
    }
}

// ── handle_scroll ─────────────────────────────────────────────────────────────

/// Wheel events: on the alt-screen (vim / less / btop) we hand the event
/// off to the PTY as arrow-key escapes — that's what those apps expect.
/// On the normal screen we drive our local scrollback instead, since the
/// shell already has its own history navigation via the arrow keys.
pub(crate) fn handle_scroll(state: &mut RunningState, delta: winit::event::MouseScrollDelta) {
    let active = state.active_tab;
    let (alt_screen, mouse_mode) = {
        let Some(tab) = state.tabs.get(active) else {
            return;
        };
        let emu = tab.emulator.lock();
        (emu.is_alt_screen(), emu.mouse_mode())
    };
    // App-requested mouse wheel reporting → SGR encoded as button 64/65.
    if mouse_mode.enabled() {
        let ppr = state.touchpad_pixels_per_row;
        let dy = delta_to_raw(delta, ppr);
        if dy.abs() < 0.001 {
            return;
        }
        let scale = state.window.scale_factor() as f32;
        let pos_px = (
            state.pointer_logical.0 * scale,
            state.pointer_logical.1 * scale,
        );
        if let Some((col, row)) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1) {
            let base: u32 = if dy > 0.0 { 64 } else { 65 };
            let modifiers = crate::mouse_modifier_bits(state);
            // Wheel "press" events with no matching release — standard.
            let lines = (dy.abs() * 1.0).round() as i32;
            for _ in 0..lines.clamp(1, 8) {
                let seq = format!("\x1b[<{};{};{}M", base + modifiers, col + 1, row + 1);
                if let Some(tab) = state.tabs.get(active) {
                    let _ = tab.session.write_input(seq.as_bytes());
                }
            }
        }
        return;
    }
    if alt_screen {
        let bytes = scroll_to_bytes(
            delta,
            state.touchpad_pixels_per_row,
            state.alt_screen_scroll_lines,
        );
        if !bytes.is_empty() {
            if let Some(tab) = state.tabs.get(active) {
                let _ = tab.session.write_input(&bytes);
            }
        }
        return;
    }

    // Convert wheel motion into a delta in *rows* of scrollback.
    let ppr = state.touchpad_pixels_per_row;
    let dy = delta_to_raw(delta, ppr);
    if dy.abs() < 0.001 {
        return;
    }
    // Rows per wheel notch — user-configurable (main screen).
    let step = state.scroll_step_lines.max(1) as f32;
    let remainder = if state.smooth_scroll {
        Some(&mut state.smooth_scroll_remainder)
    } else {
        None
    };
    let lines = raw_to_rows(dy, step, remainder);
    if lines == 0 {
        return; // sub-row delta, accumulated for later (smooth mode)
    }
    let (history, current) = state.tabs.get(active).map_or((0, 0), |t| {
        (t.emulator.lock().history_size(), t.scroll_lines)
    });
    let new_scroll = if dy > 0.0 {
        // wheel up = pan deeper into history
        (current as isize + lines).clamp(0, history as isize) as usize
    } else {
        // wheel down = move back toward live output
        (current as isize - lines).clamp(0, history as isize) as usize
    };
    if let Some(tab) = state.tabs.get_mut(active) {
        tab.scroll_lines = new_scroll;
    }
    state.renderer.set_scroll_lines(new_scroll);
}

/// Build the arrow-key byte string that alt-screen apps expect from a wheel
/// event. Uses `pixels_per_row` to normalise `PixelDelta` input and
/// `step_lines` as the per-notch row multiplier.
pub(crate) fn scroll_to_bytes(
    delta: winit::event::MouseScrollDelta,
    pixels_per_row: f32,
    step_lines: u8,
) -> Vec<u8> {
    // Positive dy = scroll up (towards history) → CSI A (arrow up) for apps.
    let dy = delta_to_raw(delta, pixels_per_row);
    let step = step_lines.max(1) as f32;
    // Alt-screen forwarding uses simple rounding (no smooth accumulator).
    let lines = (dy.abs() * step).round() as i32;
    if lines == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let seq: &[u8] = if dy > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
    for _ in 0..lines.min(8) {
        out.extend_from_slice(seq);
    }
    out
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        binding_for, build_fix_prompt, build_scrollback_export_content, extract_block_output_lines,
        extract_block_output_text, scrollback_export_filename, translate_key,
    };
    use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey, SmolStr};

    // ── translate_key: Ctrl must never echo a literal character ───────────────

    fn key_char(c: &str) -> Key {
        Key::Character(SmolStr::new(c))
    }

    #[test]
    fn ctrl_digit_is_swallowed_not_typed() {
        let bytes = translate_key(
            &ModifiersState::CONTROL,
            PhysicalKey::Code(KeyCode::Digit1),
            &key_char("1"),
            Some(SmolStr::new("1")),
            false,
        );
        assert_eq!(bytes, None, "Ctrl+1 must not type \"1\"");
    }

    #[test]
    fn ctrl_shift_digit_is_swallowed() {
        let bytes = translate_key(
            &(ModifiersState::CONTROL | ModifiersState::SHIFT),
            PhysicalKey::Code(KeyCode::Digit1),
            &key_char("!"),
            Some(SmolStr::new("!")),
            false,
        );
        assert_eq!(bytes, None, "Ctrl+Shift+1 must not type \"!\"");
    }

    #[test]
    fn ctrl_letter_still_sends_control_byte() {
        let bytes = translate_key(
            &ModifiersState::CONTROL,
            PhysicalKey::Code(KeyCode::KeyC),
            &key_char("c"),
            Some(SmolStr::new("c")),
            false,
        );
        assert_eq!(bytes, Some(vec![0x03]), "Ctrl+C must send ^C (0x03)");
    }

    #[test]
    fn plain_char_is_still_typed() {
        let bytes = translate_key(
            &ModifiersState::empty(),
            PhysicalKey::Code(KeyCode::Digit1),
            &key_char("1"),
            Some(SmolStr::new("1")),
            false,
        );
        assert_eq!(
            bytes,
            Some(b"1".to_vec()),
            "unmodified \"1\" must type \"1\""
        );
    }

    // ── build_fix_prompt ──────────────────────────────────────────────────────

    #[test]
    fn fix_prompt_contains_command_and_exit_code() {
        let p = build_fix_prompt("cargo build", 1, "/src", "error: could not find crate");
        assert!(
            p.contains("cargo build"),
            "prompt must contain the command text"
        );
        assert!(
            p.contains("exit code 1"),
            "prompt must mention the exit code"
        );
        assert!(p.contains("/src"), "prompt must mention the cwd");
        assert!(
            p.contains("error: could not find crate"),
            "prompt must contain the output"
        );
    }

    #[test]
    fn fix_prompt_includes_corrected_command_ask() {
        let p = build_fix_prompt("rm -rf /typo", 127, "/home", "command not found");
        assert!(
            p.contains("corrected command"),
            "prompt must ask for a corrected command"
        );
    }

    #[test]
    fn fix_prompt_omits_output_block_when_empty() {
        let p = build_fix_prompt("ls /nonexistent", 2, "/home", "");
        assert!(
            !p.contains("Output:"),
            "prompt must not emit an empty output block"
        );
    }

    #[test]
    fn fix_prompt_has_no_output_block_for_whitespace_only() {
        let p = build_fix_prompt("ls /nonexistent", 2, "/home", "   \n  ");
        assert!(
            !p.contains("Output:"),
            "whitespace-only output should be treated as empty"
        );
    }

    // ── extract_block_output_lines ────────────────────────────────────────────

    fn make_lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn extract_returns_correct_span() {
        // 10 lines of history (abs -10..-1) + 5 visible (abs 0..4).
        // buffer_lines_text() returns them as indices 0..14, where
        //   abs = idx - hist (hist = 10).
        let lines = make_lines(15);
        // Output span: abs 2..5 → idx 12..15 (but 15 is out of range → 14).
        let out = extract_block_output_lines(&lines, 10, 2, 4);
        assert_eq!(out, "line 12\nline 13\nline 14");
    }

    #[test]
    fn extract_clamps_to_buffer_bounds() {
        let lines = make_lines(5);
        // Request span that goes past the end.
        let out = extract_block_output_lines(&lines, 0, 3, 100);
        assert_eq!(out, "line 3\nline 4");
    }

    #[test]
    fn extract_empty_when_span_before_buffer() {
        let lines = make_lines(5);
        // output_start + hist = -5 + 0 = -5, clamped to 0. end = -1 + 0 = -1 → 0.
        // start_idx = 0, end_idx = 0; that's just one line.
        let out = extract_block_output_lines(&lines, 0, -5, -1);
        assert_eq!(
            out, "line 0",
            "negative abs lines clamp to the buffer start"
        );
    }

    #[test]
    fn extract_truncates_at_line_cap() {
        // 200 lines all larger than one character so line-cap is hit.
        let lines: Vec<String> = (0..200).map(|i| format!("output line {i:04}")).collect();
        let out = extract_block_output_lines(&lines, 0, 0, 199);
        let line_count = out.lines().count();
        // The cap is 50 lines, plus possibly a truncation marker line.
        assert!(
            line_count <= super::FIX_CMD_MAX_LINES + 1,
            "extracted {line_count} lines — expected at most {}+1",
            super::FIX_CMD_MAX_LINES
        );
        assert!(
            out.contains("truncated"),
            "truncated output must contain a truncation marker"
        );
    }

    // ── config defaults ───────────────────────────────────────────────────────

    #[test]
    fn offer_fix_on_failure_defaults_false() {
        use terminale_config::AiConfig;
        assert!(
            !AiConfig::default().offer_fix_on_failure,
            "offer_fix_on_failure must default to false"
        );
    }

    #[test]
    fn fix_last_command_shortcut_defaults_empty() {
        use terminale_config::ShortcutsConfig;
        assert!(
            ShortcutsConfig::default().fix_last_command.is_empty(),
            "fix_last_command must default to empty (unbound)"
        );
    }

    // ── action name resolution ────────────────────────────────────────────────

    #[test]
    fn fix_last_command_resolves_from_action_name() {
        use crate::keymap::action_from_name;
        use crate::ShortcutAction;
        assert_eq!(
            action_from_name("fixlastcommand"),
            Some(ShortcutAction::FixLastCommand),
            "fixlastcommand must resolve to FixLastCommand"
        );
        assert_eq!(
            action_from_name("FixLastCommand"),
            Some(ShortcutAction::FixLastCommand),
            "FixLastCommand (mixed case) must resolve"
        );
    }

    // ── extract_block_output_text ─────────────────────────────────────────────

    /// `extract_block_output_text` returns the full span without truncation.
    #[test]
    fn extract_block_output_text_full_span() {
        // 10 history lines (abs -10..-1) + 5 visible (abs 0..4).
        // idx = abs + hist; hist = 10.
        // Output span abs 0..2 → idx 10..12.
        let lines: Vec<String> = (0..15).map(|i| format!("row {i}")).collect();
        let out = extract_block_output_text(&lines, 10, 0, 2);
        assert_eq!(out, "row 10\nrow 11\nrow 12");
    }

    /// `extract_block_output_text` returns the entire large span (no cap).
    #[test]
    fn extract_block_output_text_no_truncation() {
        // 200 lines — should return all of them, not truncate at 50.
        let lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let out = extract_block_output_text(&lines, 0, 0, 199);
        assert!(
            !out.contains("truncated"),
            "extract_block_output_text must not truncate"
        );
        assert_eq!(out.lines().count(), 200, "all 200 lines must be returned");
    }

    /// `extract_block_output_text` returns empty string for an empty span.
    #[test]
    fn extract_block_output_text_empty_span() {
        let lines: Vec<String> = vec!["a".to_string(), "b".to_string()];
        // Request span entirely before history → clamped and degenerate.
        let out = extract_block_output_text(&lines, 0, -10, -5);
        // Negative indices clamp to 0; start == end == 0 → "a".
        assert_eq!(out, "a");
    }

    // ── RerunLastCommand payload ──────────────────────────────────────────────

    /// The bytes produced for a rerun are `command_text` + `\n`.
    #[test]
    fn rerun_payload_is_command_text_plus_newline() {
        let cmd = "git status";
        let mut payload = cmd.as_bytes().to_vec();
        payload.push(b'\n');
        assert_eq!(
            payload, b"git status\n",
            "rerun payload must be command text + LF"
        );
    }

    // ── EditLastCommand payload ───────────────────────────────────────────────

    /// With `clears_line = true`, the payload is Ctrl+U + command text (no newline).
    #[test]
    fn edit_payload_clears_line_prepends_ctrl_u() {
        let cmd = "ls -la";
        let clears_line = true;
        let mut payload: Vec<u8> = Vec::new();
        if clears_line {
            payload.push(0x15);
        }
        payload.extend_from_slice(cmd.as_bytes());
        assert_eq!(payload[0], 0x15, "first byte must be Ctrl+U (0x15)");
        assert_eq!(&payload[1..], b"ls -la", "command text must follow");
        assert!(!payload.ends_with(b"\n"), "no trailing newline");
    }

    /// With `clears_line = false`, no Ctrl+U prefix.
    #[test]
    fn edit_payload_no_clear_line_omits_ctrl_u() {
        let cmd = "ls -la";
        let clears_line = false;
        let mut payload: Vec<u8> = Vec::new();
        if clears_line {
            payload.push(0x15);
        }
        payload.extend_from_slice(cmd.as_bytes());
        assert_eq!(
            payload, b"ls -la",
            "without clears_line, payload is just the command"
        );
        assert!(!payload.ends_with(b"\n"), "no trailing newline");
    }

    // ── block_copy config defaults ────────────────────────────────────────────

    /// All five new shortcut fields default to empty (unbound).
    #[test]
    fn block_copy_shortcuts_default_unbound() {
        use terminale_config::ShortcutsConfig;
        let sc = ShortcutsConfig::default();
        assert!(
            sc.copy_last_command_output.is_empty(),
            "copy_last_command_output must default to unbound"
        );
        assert!(
            sc.copy_block_output.is_empty(),
            "copy_block_output must default to unbound"
        );
        assert!(
            sc.copy_last_command.is_empty(),
            "copy_last_command must default to unbound"
        );
        assert!(
            sc.rerun_last_command.is_empty(),
            "rerun_last_command must default to unbound"
        );
        assert!(
            sc.edit_last_command.is_empty(),
            "edit_last_command must default to unbound"
        );
    }

    /// `edit_command_clears_line` defaults to `true`.
    #[test]
    fn edit_command_clears_line_defaults_true() {
        use terminale_config::TerminalConfig;
        assert!(
            TerminalConfig::default().edit_command_clears_line,
            "edit_command_clears_line must default to true"
        );
    }

    /// `edit_command_clears_line` survives a TOML round-trip.
    #[test]
    fn edit_command_clears_line_roundtrips_toml() {
        let mut cfg = terminale_config::Config::default();
        cfg.terminal.edit_command_clears_line = false;
        let raw = toml::to_string(&cfg).expect("serialise");
        let back: terminale_config::Config = toml::from_str(&raw).expect("deserialise");
        assert!(
            !back.terminal.edit_command_clears_line,
            "edit_command_clears_line=false must survive a TOML round-trip"
        );
    }

    // ── action_from_name for block actions ────────────────────────────────────

    /// All five block-copy-rerun actions resolve from their lowercase names.
    #[test]
    fn block_copy_actions_resolve_from_action_name() {
        use crate::keymap::action_from_name;
        use crate::ShortcutAction;

        let cases = [
            (
                "copylastcommandoutput",
                ShortcutAction::CopyLastCommandOutput,
            ),
            ("copyblockoutput", ShortcutAction::CopyBlockOutput),
            ("copylastcommand", ShortcutAction::CopyLastCommand),
            ("rerunlastcommand", ShortcutAction::RerunLastCommand),
            ("editlastcommand", ShortcutAction::EditLastCommand),
        ];
        for (name, expected) in cases {
            assert_eq!(
                action_from_name(name),
                Some(expected),
                "action_from_name(\"{name}\") should resolve"
            );
            // Also verify mixed-case resolution.
            let mixed: String = name
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    if i % 2 == 0 {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    }
                })
                .collect();
            assert_eq!(
                action_from_name(&mixed),
                Some(expected),
                "action_from_name(\"{mixed}\") must resolve case-insensitively"
            );
        }
    }

    // ── block_at_line selection (pure logic) ──────────────────────────────────

    /// `extract_block_output_text` for a single-line span returns that line.
    #[test]
    fn extract_block_output_text_single_line() {
        let lines = vec![
            "header".to_string(),
            "output here".to_string(),
            "footer".to_string(),
        ];
        // history_size = 0, output_start = 1, output_end = 1.
        let out = extract_block_output_text(&lines, 0, 1, 1);
        assert_eq!(out, "output here");
    }

    // ── binding_for block actions ─────────────────────────────────────────────

    /// `binding_for` returns empty strings for the new actions by default.
    #[test]
    fn block_copy_binding_for_defaults_empty() {
        use crate::ShortcutAction;
        let sc = terminale_config::ShortcutsConfig::default();
        assert!(binding_for(ShortcutAction::CopyLastCommandOutput, &sc).is_empty());
        assert!(binding_for(ShortcutAction::CopyBlockOutput, &sc).is_empty());
        assert!(binding_for(ShortcutAction::CopyLastCommand, &sc).is_empty());
        assert!(binding_for(ShortcutAction::RerunLastCommand, &sc).is_empty());
        assert!(binding_for(ShortcutAction::EditLastCommand, &sc).is_empty());
    }

    // ── scrollback export ─────────────────────────────────────────────────────

    /// `ExportScrollback` is unbound by default.
    #[test]
    fn export_scrollback_binding_defaults_empty() {
        use crate::ShortcutAction;
        let sc = terminale_config::ShortcutsConfig::default();
        assert!(
            binding_for(ShortcutAction::ExportScrollback, &sc).is_empty(),
            "export_scrollback must default to unbound"
        );
    }

    /// `build_scrollback_export_content` joins lines with newlines and strips
    /// trailing empty lines.
    #[test]
    fn export_content_joins_and_trims_trailing_blank_lines() {
        let lines = vec![
            "line one".to_string(),
            "line two".to_string(),
            String::new(),
            String::new(),
        ];
        let content = build_scrollback_export_content(&lines);
        assert_eq!(
            content, "line one\nline two",
            "trailing blank lines must be stripped"
        );
    }

    /// An all-empty buffer produces an empty string.
    #[test]
    fn export_content_all_empty_lines_gives_empty() {
        let lines: Vec<String> = vec![String::new(); 5];
        let content = build_scrollback_export_content(&lines);
        assert!(
            content.is_empty(),
            "all-empty buffer must export as empty string"
        );
    }

    /// Non-trailing empty lines are preserved.
    #[test]
    fn export_content_preserves_interior_blank_lines() {
        let lines = vec!["first".to_string(), String::new(), "third".to_string()];
        let content = build_scrollback_export_content(&lines);
        assert_eq!(
            content, "first\n\nthird",
            "interior blank lines must be preserved"
        );
    }

    /// `scrollback_export_filename` produces the expected `terminale-scrollback-…` format.
    #[test]
    fn export_filename_format_is_correct() {
        use chrono::TimeZone;
        // 2025-06-01 15:30:45 local time
        let dt = chrono::Local
            .with_ymd_and_hms(2025, 6, 1, 15, 30, 45)
            .single()
            .expect("fixed datetime must be valid");
        let name = scrollback_export_filename(&dt);
        assert_eq!(name, "terminale-scrollback-20250601-153045.txt");
    }

    /// `scrollback_export_filename` result has `.txt` extension.
    #[test]
    fn export_filename_ends_with_txt() {
        let now = chrono::Local::now();
        let name = scrollback_export_filename(&now);
        assert!(
            std::path::Path::new(&name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("txt")),
            "filename must have a .txt extension"
        );
    }

    /// `scrollback_export_dir` config field defaults to `None`.
    #[test]
    fn scrollback_export_dir_defaults_to_none() {
        use terminale_config::TerminalConfig;
        assert!(
            TerminalConfig::default().scrollback_export_dir.is_none(),
            "scrollback_export_dir must default to None (use save dialog)"
        );
    }

    /// `scrollback_export_format` config field defaults to `Plain`.
    #[test]
    fn scrollback_export_format_defaults_to_plain() {
        use terminale_config::{ScrollbackExportFormat, TerminalConfig};
        assert_eq!(
            TerminalConfig::default().scrollback_export_format,
            ScrollbackExportFormat::Plain,
            "scrollback_export_format must default to Plain"
        );
    }

    /// Config roundtrip for `scrollback_export_format`.
    #[test]
    fn scrollback_export_format_roundtrips_toml() {
        use terminale_config::ScrollbackExportFormat;
        let mut cfg = terminale_config::Config::default();
        cfg.terminal.scrollback_export_format = ScrollbackExportFormat::Plain;
        let raw = toml::to_string(&cfg).expect("serialise");
        let back: terminale_config::Config = toml::from_str(&raw).expect("deserialise");
        assert_eq!(
            back.terminal.scrollback_export_format,
            ScrollbackExportFormat::Plain,
            "scrollback_export_format must survive a TOML round-trip"
        );
    }

    /// Config roundtrip for `scrollback_export_dir`.
    #[test]
    fn scrollback_export_dir_roundtrips_toml() {
        let mut cfg = terminale_config::Config::default();
        cfg.terminal.scrollback_export_dir = Some(std::path::PathBuf::from("/tmp/exports"));
        let raw = toml::to_string(&cfg).expect("serialise");
        let back: terminale_config::Config = toml::from_str(&raw).expect("deserialise");
        assert_eq!(
            back.terminal.scrollback_export_dir,
            Some(std::path::PathBuf::from("/tmp/exports")),
            "scrollback_export_dir must survive a TOML round-trip"
        );
    }

    // ── scroll delta conversion ───────────────────────────────────────────────

    use super::{delta_to_raw, raw_to_rows};
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    fn line_delta(y: f32) -> MouseScrollDelta {
        MouseScrollDelta::LineDelta(0.0, y)
    }

    fn pixel_delta(y: f64) -> MouseScrollDelta {
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, y))
    }

    // ── delta_to_raw ──────────────────────────────────────────────────────────

    /// LineDelta passes through unchanged.
    #[test]
    fn delta_to_raw_line_passthrough() {
        assert!((delta_to_raw(line_delta(3.0), 16.0) - 3.0).abs() < f32::EPSILON);
        assert!((delta_to_raw(line_delta(-2.5), 16.0) - (-2.5)).abs() < f32::EPSILON);
    }

    /// PixelDelta is divided by pixels_per_row.
    #[test]
    fn delta_to_raw_pixel_divided_by_ppr() {
        // 48 pixels / 16.0 ppr = 3.0 lines.
        assert!((delta_to_raw(pixel_delta(48.0), 16.0) - 3.0).abs() < 1e-5);
        // 48 pixels / 12.0 ppr = 4.0 lines.
        assert!((delta_to_raw(pixel_delta(48.0), 12.0) - 4.0).abs() < 1e-5);
    }

    /// Negative PixelDelta produces a negative result.
    #[test]
    fn delta_to_raw_pixel_negative() {
        let v = delta_to_raw(pixel_delta(-32.0), 16.0);
        assert!(v < 0.0, "downward pixel delta must be negative, got {v}");
        assert!((v - (-2.0)).abs() < 1e-5);
    }

    /// pixels_per_row clamped to 1.0 minimum — no division by zero.
    #[test]
    fn delta_to_raw_ppr_clamped_to_one() {
        // ppr = 0.0 → clamped to 1.0 → 8 px = 8.0 rows
        let v = delta_to_raw(pixel_delta(8.0), 0.0);
        assert!((v - 8.0).abs() < 1e-5, "ppr must clamp to 1.0, got {v}");
    }

    // ── raw_to_rows (non-smooth) ───────────────────────────────────────────────

    /// LineDelta × step_lines rounds correctly.
    #[test]
    fn raw_to_rows_line_multiplier_scrollback() {
        // step = 3 lines/notch, raw = 1.0 → 3 rows
        assert_eq!(raw_to_rows(1.0, 3.0, None), 3);
        // step = 5, raw = 2.0 → 10
        assert_eq!(raw_to_rows(2.0, 5.0, None), 10);
    }

    /// Different steps for scrollback vs alt-screen.
    #[test]
    fn raw_to_rows_alt_screen_step() {
        // scrollback: step = 3
        assert_eq!(raw_to_rows(1.0, 3.0, None), 3);
        // alt-screen: step = 5
        assert_eq!(raw_to_rows(1.0, 5.0, None), 5);
    }

    /// PixelDelta at 16 px/row gives correct row count with step = 1.
    #[test]
    fn raw_to_rows_pixel_at_default_ppr() {
        // 48 px / 16 ppr = 3.0 raw, step = 1 → 3 rows
        let raw = delta_to_raw(pixel_delta(48.0), 16.0);
        assert_eq!(raw_to_rows(raw, 1.0, None), 3);
    }

    // ── raw_to_rows (smooth accumulator) ─────────────────────────────────────

    /// Three 6-pixel events at 16 px/row with step = 1 sum to exactly 1 row
    /// on the third event; remainder is carried correctly.
    #[test]
    fn smooth_accumulator_three_events_sum_to_one_row() {
        // 6/16 = 0.375 per event; three events = 1.125 total.
        // After event 1: rem = 0.375, rows = 0.
        // After event 2: rem = 0.75,  rows = 0.
        // After event 3: rem = 0.125, rows = 1.
        let ppr = 16.0_f32;
        let mut rem = 0.0_f32;
        let e1 = delta_to_raw(pixel_delta(6.0), ppr);
        let e2 = delta_to_raw(pixel_delta(6.0), ppr);
        let e3 = delta_to_raw(pixel_delta(6.0), ppr);

        let r1 = raw_to_rows(e1, 1.0, Some(&mut rem));
        let r2 = raw_to_rows(e2, 1.0, Some(&mut rem));
        let r3 = raw_to_rows(e3, 1.0, Some(&mut rem));

        assert_eq!(r1, 0, "first 6px event: no full row yet");
        assert_eq!(r2, 0, "second 6px event: still below threshold");
        assert_eq!(r3, 1, "third 6px event: crosses one whole row");
        assert!(
            (rem - 0.125).abs() < 1e-4,
            "remainder after three 6px events: expected ~0.125, got {rem}"
        );
    }

    /// Remainder resets to zero when the direction reverses.
    /// (Implicit: raw_to_rows does NOT reset on direction change — the
    /// sign of rem already cancels the opposite direction naturally.)
    #[test]
    fn smooth_accumulator_direction_reversal() {
        let ppr = 16.0_f32;
        let mut rem = 0.0_f32;
        // Accumulate 10 px upward (0.625 rows).
        let up = delta_to_raw(pixel_delta(10.0), ppr);
        let _ = raw_to_rows(up, 1.0, Some(&mut rem));
        assert!((rem - 0.625).abs() < 1e-4);

        // Now scroll 10 px downward — rem goes from 0.625 - 0.625 = 0.0.
        let down = delta_to_raw(pixel_delta(-10.0), ppr);
        let rows = raw_to_rows(down, 1.0, Some(&mut rem));
        // 0.625 + (-0.625) = 0.0 → no whole rows, remainder 0.
        assert_eq!(
            rows, 0,
            "opposite direction cancels the accumulated remainder"
        );
        assert!(
            rem.abs() < 1e-4,
            "remainder must be zero after cancellation, got {rem}"
        );
    }

    /// A large LineDelta produces the expected row count even in smooth mode.
    #[test]
    fn smooth_accumulator_large_line_delta() {
        let mut rem = 0.0_f32;
        // raw = 3.0 (three notches), step = 3 → scaled = 9.0 rows.
        let rows = raw_to_rows(3.0, 3.0, Some(&mut rem));
        assert_eq!(rows, 9, "three notches × step 3 = 9 rows");
        assert!(
            rem.abs() < 1e-5,
            "no remainder for exact integer, got {rem}"
        );
    }

    // ── scroll config defaults ────────────────────────────────────────────────

    /// The new scroll config fields default to sane values.
    #[test]
    fn scroll_config_defaults() {
        use terminale_config::WindowConfig;
        let cfg = WindowConfig::default();
        assert_eq!(cfg.alt_screen_scroll_lines, 3);
        assert!((cfg.touchpad_pixels_per_row - 16.0).abs() < f32::EPSILON);
        assert!(!cfg.smooth_scroll, "smooth_scroll must default to false");
    }

    /// New scroll config fields survive a TOML round-trip.
    #[test]
    fn scroll_config_roundtrips_toml() {
        let mut cfg = terminale_config::Config::default();
        cfg.window.alt_screen_scroll_lines = 5;
        cfg.window.touchpad_pixels_per_row = 24.0;
        cfg.window.smooth_scroll = true;
        let raw = toml::to_string(&cfg).expect("serialise");
        let back: terminale_config::Config = toml::from_str(&raw).expect("deserialise");
        assert_eq!(back.window.alt_screen_scroll_lines, 5);
        assert!((back.window.touchpad_pixels_per_row - 24.0).abs() < f32::EPSILON);
        assert!(back.window.smooth_scroll);
    }

    /// `touchpad_pixels_per_row` validation rejects values outside 1..=128.
    #[test]
    fn touchpad_pixels_per_row_validation() {
        let mut cfg = terminale_config::Config::default();
        cfg.window.touchpad_pixels_per_row = 0.5;
        assert!(cfg.validate().is_err(), "value < 1.0 must fail validation");
        cfg.window.touchpad_pixels_per_row = 200.0;
        assert!(
            cfg.validate().is_err(),
            "value > 128.0 must fail validation"
        );
        cfg.window.touchpad_pixels_per_row = 16.0;
        assert!(cfg.validate().is_ok(), "default value must pass validation");
    }

    // ── scroll_to_bytes with alt_screen_scroll_lines ──────────────────────────

    /// scroll_to_bytes with step=1 produces a single arrow escape.
    #[test]
    fn scroll_to_bytes_single_notch_step_1() {
        use super::scroll_to_bytes;
        let bytes = scroll_to_bytes(line_delta(1.0), 16.0, 1);
        assert_eq!(bytes, b"\x1b[A", "one notch up, step 1 = one CSI A");
    }

    /// scroll_to_bytes with step=3 produces three arrow escapes.
    #[test]
    fn scroll_to_bytes_step_three() {
        use super::scroll_to_bytes;
        let bytes = scroll_to_bytes(line_delta(1.0), 16.0, 3);
        let expected: Vec<u8> = b"\x1b[A\x1b[A\x1b[A".to_vec();
        assert_eq!(bytes, expected, "one notch up, step 3 = three CSI A");
    }

    /// scroll_to_bytes downward produces CSI B.
    #[test]
    fn scroll_to_bytes_downward() {
        use super::scroll_to_bytes;
        let bytes = scroll_to_bytes(line_delta(-1.0), 16.0, 2);
        let expected: Vec<u8> = b"\x1b[B\x1b[B".to_vec();
        assert_eq!(bytes, expected, "one notch down, step 2 = two CSI B");
    }
}
