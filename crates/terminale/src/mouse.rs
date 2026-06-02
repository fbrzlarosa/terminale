//! Mouse input handling: hit-testing, SGR mouse reporting, button handling,
//! resize-edge detection.

use crate::RunningState;
use terminale_config::{KeyActionSpec, MouseBinding};
use winit::event::{ElementState, MouseButton};
use winit::keyboard::ModifiersState;
use winit::window::Window;

/// Border thickness (logical px) used for window edge-resize hit-testing
/// on custom-decoration windows.
pub(crate) const RESIZE_BORDER: f32 = 5.0;

/// If the cursor (in logical px) is within [`RESIZE_BORDER`] of one of the
/// window edges, return which resize direction the user is starting.
pub(crate) fn detect_resize_edge(
    logical_x: f32,
    logical_y: f32,
    window: &Window,
) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection::*;
    if window.is_maximized() || window.fullscreen().is_some() {
        return None;
    }
    let size = window.inner_size();
    let scale = window.scale_factor() as f32;
    let w = size.width as f32 / scale;
    let h = size.height as f32 / scale;
    let b = RESIZE_BORDER;
    let on_left = logical_x <= b;
    let on_right = logical_x >= w - b;
    let on_top = logical_y <= b;
    let on_bot = logical_y >= h - b;
    match (on_top, on_bot, on_left, on_right) {
        (true, _, true, _) => Some(NorthWest),
        (true, _, _, true) => Some(NorthEast),
        (_, true, true, _) => Some(SouthWest),
        (_, true, _, true) => Some(SouthEast),
        (true, _, _, _) => Some(North),
        (_, true, _, _) => Some(South),
        (_, _, true, _) => Some(West),
        (_, _, _, true) => Some(East),
        _ => None,
    }
}

pub(crate) fn cursor_icon_for_resize(
    dir: winit::window::ResizeDirection,
) -> winit::window::CursorIcon {
    use winit::window::CursorIcon;
    use winit::window::ResizeDirection::*;
    match dir {
        North | South => CursorIcon::NsResize,
        East | West => CursorIcon::EwResize,
        NorthEast | SouthWest => CursorIcon::NeswResize,
        NorthWest | SouthEast => CursorIcon::NwseResize,
    }
}

// ── SGR mouse reporting ───────────────────────────────────────────────────────

/// Encode a mouse event as an SGR-1006 escape sequence and send it to
/// the active PTY when the focused app has enabled mouse reporting.
/// Returns `true` when the event was forwarded — the caller should then
/// skip its local handling (selection, tab clicks, etc.).
pub(crate) fn maybe_report_mouse(
    state: &mut RunningState,
    pos_px: (f32, f32),
    button: MouseButton,
    pressed: bool,
) -> bool {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get(active) else {
        return false;
    };
    let mode = tab.emulator.lock().mouse_mode();
    if !mode.enabled() {
        return false;
    }
    // Tab-bar area is *not* inside the terminal grid — don't smuggle
    // clicks on our own UI through to the app.
    let Some((col, row)) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1) else {
        return false;
    };
    let base = match button {
        MouseButton::Left => 0u32,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => return false,
    };
    let modifiers = mouse_modifier_bits(state);
    let action = if pressed { 'M' } else { 'm' };
    let seq = format!(
        "\x1b[<{};{};{}{}",
        base + modifiers,
        col + 1,
        row + 1,
        action
    );
    let _ = tab.session.write_input(seq.as_bytes());
    true
}

/// Report a mouse-motion event to the PTY (SGR 1006 with bit 32 for
/// motion). Handles both MOUSE_DRAG (1002 — only while a button is held)
/// and ANY_MOTION (1003 — even with no button, encoded as base 3). Reports
/// once per *cell* change to avoid flooding the PTY with per-pixel events.
/// Returns `true` when the app owns this motion (caller skips local
/// selection / hover).
pub(crate) fn report_mouse_motion(state: &mut RunningState, pos_px: (f32, f32)) -> bool {
    let active = state.active_tab;
    let mode = match state.tabs.get(active) {
        Some(tab) => tab.emulator.lock().mouse_mode(),
        None => return false,
    };
    if !mode.drag && !mode.motion {
        return false;
    }
    let Some((col, row)) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1) else {
        // Off the grid (tab bar / padding) — reset so re-entry reports.
        state.last_motion_cell = None;
        return false;
    };
    // Button base: a held button → 0/1/2; no button → only report under
    // any-motion (1003), encoded as base 3 ("no button").
    let base = match state.held_button {
        Some(MouseButton::Left) => 0u32,
        Some(MouseButton::Middle) => 1,
        Some(MouseButton::Right) => 2,
        Some(_) => return false,
        None => {
            if !mode.motion {
                return false; // pure 1002 drag needs a held button
            }
            3
        }
    };
    // Emit only when the cell changes (xterm reports per-cell, not per-pixel).
    if state.last_motion_cell != Some((col, row)) {
        state.last_motion_cell = Some((col, row));
        let modifiers = mouse_modifier_bits(state);
        // Bit 5 (32) flags the event as a motion event.
        let code = base + modifiers + 32;
        let seq = format!("\x1b[<{};{};{}M", code, col + 1, row + 1);
        if let Some(tab) = state.tabs.get(active) {
            let _ = tab.session.write_input(seq.as_bytes());
        }
    }
    true
}

/// Shift / alt / control bit-flags for SGR mouse reporting (rfc/xterm).
pub(crate) fn mouse_modifier_bits(state: &RunningState) -> u32 {
    let mut bits = 0;
    if state.modifiers.shift_key() {
        bits |= 4;
    }
    if state.modifiers.alt_key() {
        bits |= 8;
    }
    if state.modifiers.control_key() {
        bits |= 16;
    }
    bits
}

// ── Custom mouse binding resolution ──────────────────────────────────────────

/// Parse a mouse button name string (case-insensitive) into a
/// [`MouseButton`]. Returns `None` for unrecognised names.
#[must_use]
fn parse_mouse_button(s: &str) -> Option<MouseButton> {
    match s.to_lowercase().as_str() {
        "left" => Some(MouseButton::Left),
        "right" => Some(MouseButton::Right),
        "middle" => Some(MouseButton::Middle),
        "back" => Some(MouseButton::Back),
        "forward" => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Parse a modifiers string (e.g. `"Ctrl+Shift"`, `""`) into a
/// [`crate::shortcuts::ModFlags`].
#[must_use]
fn parse_mouse_mods(s: &str) -> crate::shortcuts::ModFlags {
    let mut m = crate::shortcuts::ModFlags::default();
    for token in s.split('+') {
        match token.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => m.ctrl = true,
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            "cmd" | "super" | "meta" | "win" => m.meta = true,
            _ => {}
        }
    }
    m
}

/// Try to match `(button, mods, click_count)` against every entry in
/// `bindings`, returning the resolved action list for the **first** match.
///
/// Returns `None` when no custom binding matches the combination.
/// This function is pure and unit-testable.
#[must_use]
pub(crate) fn resolve_mouse_binding<'a>(
    button: MouseButton,
    mods: &ModifiersState,
    click_count: u8,
    bindings: &'a [MouseBinding],
) -> Option<&'a [KeyActionSpec]> {
    let pressed = crate::shortcuts::ModFlags {
        ctrl: mods.control_key(),
        shift: mods.shift_key(),
        alt: mods.alt_key(),
        meta: mods.super_key(),
    };
    for bind in bindings {
        let Some(btn) = parse_mouse_button(&bind.button) else {
            continue;
        };
        if btn != button {
            continue;
        }
        let bind_mods = parse_mouse_mods(&bind.mods);
        if bind_mods != pressed {
            continue;
        }
        // count 0 is treated as 1 (sane default for malformed configs).
        let bind_count = bind.count.max(1);
        if bind_count != click_count {
            continue;
        }
        if bind.actions.is_empty() {
            continue;
        }
        return Some(&bind.actions);
    }
    None
}

/// Execute a list of [`KeyActionSpec`] actions against the running state
/// (the same dispatch used for custom keyboard bindings).
pub(crate) fn dispatch_mouse_actions(state: &mut RunningState, actions: &[KeyActionSpec]) {
    for spec in actions {
        if let Some(bytes) = spec.as_send_bytes() {
            if let Some(tab) = state.tabs.get(state.active_tab) {
                if let Err(e) = tab.session.write_input(&bytes) {
                    tracing::warn!(?e, "custom mouse binding PTY write failed");
                }
            }
        } else if let Some(name) = spec.action_name() {
            if let Some(action) = crate::keymap::action_from_name(name) {
                crate::shortcuts::dispatch_shortcut(state, action);
            }
        }
    }
}

// ── handle_mouse ─────────────────────────────────────────────────────────────

/// Handle clicks. Left = start/extend selection. Right = open context menu.
pub(crate) fn handle_mouse(state: &mut RunningState, button: MouseButton, btn_state: ElementState) {
    use terminale_render::{CellRect, TabHit};

    let scale = state.window.scale_factor() as f32;
    let pos_px = (
        state.pointer_logical.0 * scale,
        state.pointer_logical.1 * scale,
    );

    // Track currently-held button so CursorMoved can synthesise drag
    // motion events for the PTY when MOUSE_DRAG is active.
    match btn_state {
        ElementState::Pressed => {
            state.held_button = Some(button);
            // A press tells us the user is actively interacting with this
            // window. Refresh the Quake monitor snapshot so that
            // `QuakeDisplay::Current` reflects the monitor the user just
            // clicked on (important when the window spans monitors).
            crate::refresh_quake_last_monitor(state);
        }
        ElementState::Released if state.held_button == Some(button) => {
            state.held_button = None;
        }
        _ => {}
    }
    // Button state changed → let the next motion report even if same cell.
    state.last_motion_cell = None;

    // App requested mouse reporting? Forward the event and bail before
    // any local handling kicks in. Only kicks in when the cursor is
    // over the terminal grid (`cell_at_pixel` returns Some).
    if maybe_report_mouse(
        state,
        pos_px,
        button,
        matches!(btn_state, ElementState::Pressed),
    ) {
        return;
    }

    // ── Custom mouse bindings (additive layer, Pressed only) ─────────────────
    // Check BEFORE any default handling so a matching binding can consume
    // the press. We compute the prospective click count from `last_click`
    // (same timing window as the selection multi-click logic). For buttons
    // other than Left, `last_click` is never updated by the default code, so
    // their count is always 1 — which is correct: multi-click semantics for
    // non-left buttons require an explicit binding.
    if matches!(btn_state, ElementState::Pressed) && !state.mouse_bindings.is_empty() {
        let click_count = if button == MouseButton::Left {
            let now = std::time::Instant::now();
            match state.last_click {
                Some((t, p, c))
                    if now.duration_since(t) <= std::time::Duration::from_millis(400)
                        && (p.0 - pos_px.0).abs() < 4.0
                        && (p.1 - pos_px.1).abs() < 4.0 =>
                {
                    c.saturating_add(1).min(3)
                }
                _ => 1,
            }
        } else {
            1
        };

        let bindings = state.mouse_bindings.clone();
        if let Some(actions) =
            resolve_mouse_binding(button, &state.modifiers, click_count, &bindings)
        {
            let actions: Vec<KeyActionSpec> = actions.to_vec();
            dispatch_mouse_actions(state, &actions);
            return; // binding consumed the press; skip all default handling
        }
    }

    // Tab bar always wins, regardless of mode.
    let tab_hit = state.renderer.tab_hit(pos_px.0, pos_px.1);

    match (button, btn_state) {
        (MouseButton::Left, ElementState::Pressed) => {
            // Resize edges win over everything else — tab bar and title-drag
            // areas don't live within the few px next to the window border.
            if let Some(dir) = detect_resize_edge(
                state.pointer_logical.0,
                state.pointer_logical.1,
                &state.window,
            ) {
                let _ = state.window.drag_resize_window(dir);
                return;
            }
            // Command palette is a modal overlay: while open it owns clicks.
            // Click a result row → select + activate it; click outside the
            // panel → dismiss.
            if state.renderer.command_palette_open() {
                if let Some(idx) = state.renderer.command_palette_row_at(pos_px.0, pos_px.1) {
                    if let Some(p) = state.command_palette.as_mut() {
                        p.selected = idx;
                    }
                    crate::refresh_palette(state);
                    crate::activate_palette_selection(state);
                } else {
                    crate::close_palette(state);
                }
                return;
            }
            // Save-host toast wins clicks while shown — it's drawn on top.
            if let Some(hit) = state.renderer.save_prompt_hit(pos_px.0, pos_px.1) {
                crate::handle_save_prompt_click(state, hit);
                return;
            }
            // Suggestion bar hit-test: inject/fix or dismiss.
            if let Some(hit) = state.renderer.suggestion_bar_hit(pos_px.0, pos_px.1) {
                match hit {
                    terminale_render::SuggestionBarHit::Inject => {
                        match &state.suggestions.state {
                            crate::suggestions::SuggestionState::Ready(cmd) => {
                                if let Some(tab) = state.tabs.get(state.active_tab) {
                                    let _ = tab.session.write_input(cmd.as_bytes());
                                }
                                state.suggestions.state =
                                    crate::suggestions::SuggestionState::Hidden;
                            }
                            crate::suggestions::SuggestionState::Hint(_) => {
                                // [Fix] — same flow as the FixLastCommand
                                // shortcut: seed the AI assistant with the
                                // failed block's command/output/exit code.
                                state.suggestions.state =
                                    crate::suggestions::SuggestionState::Hidden;
                                crate::shortcuts::fix_last_command(state);
                            }
                            _ => {
                                state.suggestions.state =
                                    crate::suggestions::SuggestionState::Hidden;
                            }
                        }
                    }
                    terminale_render::SuggestionBarHit::Dismiss => {
                        state.suggestions.state = crate::suggestions::SuggestionState::Hidden;
                    }
                }
                state.window.request_redraw();
                return;
            }
            // Ctrl+click on a hyperlinked cell → open the URI in the
            // system browser. Cleanest UX is requiring Ctrl so accidental
            // clicks on URLs in shell prompts don't navigate.
            if state.modifiers.control_key() {
                // OSC 8 hyperlinks are authoritative → always system-open.
                if let Some(uri) = crate::osc8_under(state, pos_px) {
                    if let Err(e) = open::that(&uri) {
                        tracing::warn!(?e, %uri, "open hyperlink failed");
                    }
                    return;
                }
                // Autodetected URL or file path. Paths with a configured
                // editor + line jump straight to the line; everything else
                // opens with the OS default handler.
                if let Some(link) = crate::autodetect_link_under(state, pos_px) {
                    crate::open_detected_link(state, &link);
                    return;
                }
            }
            match tab_hit {
                Some(TabHit::Tab(idx)) => {
                    // Double-click on the same tab → inline rename.
                    let now = std::time::Instant::now();
                    let is_double = matches!(
                        state.last_tab_click,
                        Some((t, i))
                            if i == idx
                                && now.duration_since(t)
                                    <= std::time::Duration::from_millis(400)
                    );
                    if is_double {
                        state.last_tab_click = None;
                        crate::switch_tab(state, idx);
                        crate::start_rename(state);
                        return;
                    }
                    // First click: record for potential double-click and arm
                    // a pending tab-drag (the App promotes it to a real drag
                    // once the cursor moves past the arm threshold).
                    state.last_tab_click = Some((now, idx));
                    crate::switch_tab(state, idx);
                    state.tab_press = Some((idx, pos_px));
                    return;
                }
                Some(TabHit::Close(idx)) => {
                    crate::request_close_tab(state, idx);
                    return;
                }
                Some(TabHit::Plus) => {
                    crate::new_tab(state);
                    return;
                }
                Some(TabHit::Minimize) => {
                    state.window.set_minimized(true);
                    return;
                }
                Some(TabHit::Maximize) => {
                    // Toggle maximise state.
                    state.window.set_maximized(!state.window.is_maximized());
                    return;
                }
                Some(TabHit::CloseWindow) => {
                    // Honour confirm_close like the OS close button: queue
                    // the request so the App opens the confirmation dialog.
                    if state.confirm_close {
                        state.pending_close_confirm =
                            Some(crate::confirm_close::CloseTarget::Window);
                        state.window.request_redraw();
                    } else {
                        // signal to the App to close this window
                        state.window.set_visible(false);
                        // A visibility-false window is reaped by
                        // `reap_empty_windows`; clear tabs so it qualifies.
                        state.tabs.clear();
                    }
                    return;
                }
                Some(TabHit::GroupLabel(first_idx)) => {
                    // Left-press on a group pill: arm the pending drag.  A
                    // plain click that never moves past the threshold will
                    // trigger the inline rename on release (see the
                    // `Left Released` arm below).  If the cursor moves far
                    // enough first, `promote_group_drag` fires and clears
                    // `group_press` before the release handler sees it.
                    if let Some(gid) = state.tabs.get(first_idx).and_then(|t| t.group) {
                        state.group_press = Some((gid, first_idx, pos_px));
                    }
                    return;
                }
                Some(TabHit::DragHandle) => {
                    // Title-bar area: double-click toggles maximize; single
                    // click starts a drag. Distinguish via `last_titlebar_click`.
                    let now = std::time::Instant::now();
                    let is_double = matches!(
                        state.last_titlebar_click,
                        Some((t, p))
                            if now.duration_since(t) <= std::time::Duration::from_millis(400)
                                && (p.0 - pos_px.0).abs() < 4.0
                                && (p.1 - pos_px.1).abs() < 4.0
                    );
                    if is_double {
                        state.window.set_maximized(!state.window.is_maximized());
                        state.last_titlebar_click = None;
                    } else {
                        state.last_titlebar_click = Some((now, pos_px));
                        // Docked Quake window: dragging the title bar un-docks
                        // it — shrink back to the pre-dock floating size
                        // before the OS drag takes over (Chrome-style).
                        crate::maybe_undock_quake_on_drag(state, pos_px);
                        let _ = state.window.drag_window();
                    }
                    return;
                }
                None => {}
            }

            // Otherwise hide any menu, clear any selection from a previous
            // drag, and arm a *potential* selection. We don't actually paint
            // anything until the user moves the cursor — single click stays
            // a single click.
            if state.menu_visible {
                state.menu_visible = false;
                state.renderer.set_overlay(None);
            }

            // Multi-click detection: if this click is close in time AND
            // space to the previous one, bump the count. Otherwise reset.
            let now = std::time::Instant::now();
            let count = match state.last_click {
                Some((t, p, c))
                    if now.duration_since(t) <= std::time::Duration::from_millis(400)
                        && (p.0 - pos_px.0).abs() < 4.0
                        && (p.1 - pos_px.1).abs() < 4.0 =>
                {
                    c.saturating_add(1)
                }
                _ => 1,
            };
            state.last_click = Some((now, pos_px, count));

            // Double-click → word, triple-click → line. Any further
            // click cycles back to "start a fresh selection".
            if count >= 2 {
                if let Some((col, row)) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1) {
                    let scroll = state.renderer.scroll_lines();
                    let tab = &state.tabs[state.active_tab];
                    let emu = tab.emulator.lock();
                    let (a, c) = if count == 2 {
                        emu.word_at(col, row, scroll, &state.word_separators)
                    } else {
                        emu.line_at(row)
                    };
                    drop(emu);
                    state.renderer.set_selection(Some(CellRect {
                        anchor: a,
                        cursor: c,
                        block: false,
                    }));
                    state.selection_anchor = None;
                    state.selection_press_px = None;
                    state.selecting = false;
                    // Reset on triple so the 4th click starts fresh.
                    if count >= 3 {
                        state.last_click = None;
                    }
                    return;
                }
            }

            // A left-click inside the terminal body exits copy mode, quick-select,
            // and pane-select so normal mouse selection can take over.
            if state.copy_mode.active {
                state.copy_mode.exit();
            }
            if state.quick_select.is_some() {
                state.quick_select = None;
            }
            if state.pane_select.is_some() {
                state.pane_select = None;
            }
            state.renderer.set_selection(None);
            if let Some(anchor) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1) {
                state.selection_anchor = Some(anchor);
                state.selection_press_px = Some(pos_px);
                state.selecting = false; // not yet — wait for drag
            }
        }
        (MouseButton::Left, ElementState::Released) => {
            // Copy-on-select: finishing a mouse selection (drag, or
            // double/triple-click word/line) auto-copies it. `copy_selection`
            // no-ops when there's no selection, so plain clicks (which clear
            // the selection on press) don't clobber the clipboard.
            if state.copy_on_select {
                crate::copy_selection(state);
            }
            state.selecting = false;
            state.selection_press_px = None;
            // A drag that actually armed is resolved by the App-level
            // intercept before `handle_mouse` runs; a still-set `tab_press`
            // here only means "pressed but never dragged" (a plain switch).
            state.tab_press = None;
            // A group_press that was never promoted into a drag (cursor
            // stayed within the arm threshold) is a plain click → rename.
            if let Some((gid, _, _)) = state.group_press.take() {
                crate::tab_groups::start_rename_group(state, gid);
            }
        }
        (MouseButton::Right, ElementState::Pressed) => {
            // Pick which menu to build: right-clicking a tab shows tab + group
            // management; right-clicking the terminal body shows terminal
            // actions only. Selecting the clicked tab first makes the tab/group
            // actions (which target the active tab) act on the right one.
            match tab_hit {
                Some(TabHit::Tab(idx)) => {
                    crate::tabs::switch_tab(state, idx);
                    state.menu_context = crate::MenuContext::Tab(idx);
                }
                _ => state.menu_context = crate::MenuContext::Terminal,
            }
            // Spawn a native popup window for the menu — App will create
            // the actual window on the next event loop tick (when an
            // `ActiveEventLoop` is in scope).
            let win_pos = state.window.outer_position().unwrap_or_default();
            let menu_x = win_pos.x + pos_px.0 as i32;
            let menu_y = win_pos.y + pos_px.1 as i32;
            state.open_menu_at = Some(winit::dpi::PhysicalPosition::new(menu_x, menu_y));
            // Also keep the legacy in-window overlay hidden.
            state.menu_visible = false;
            state.renderer.set_overlay(None);
        }
        (MouseButton::Middle, ElementState::Pressed) => {
            if let Some(TabHit::Tab(idx)) = tab_hit {
                crate::request_close_tab(state, idx);
                return;
            }
            // Linux-style middle-click paste.
            crate::paste_clipboard(state);
        }
        _ => {}
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use terminale_config::{KeyActionSpec, MouseBinding};

    fn no_mods() -> ModifiersState {
        ModifiersState::empty()
    }

    fn ctrl_mods() -> ModifiersState {
        ModifiersState::CONTROL
    }

    fn shift_mods() -> ModifiersState {
        ModifiersState::SHIFT
    }

    fn make_binding(button: &str, mods: &str, count: u8, action: &str) -> MouseBinding {
        MouseBinding {
            button: button.to_string(),
            mods: mods.to_string(),
            count,
            actions: vec![KeyActionSpec::Action(action.to_string())],
        }
    }

    // ── resolve_mouse_binding: positive match cases ───────────────────────────

    #[test]
    fn resolve_right_no_mods_single_matches() {
        let bindings = vec![make_binding("Right", "", 1, "Copy")];
        let result = resolve_mouse_binding(MouseButton::Right, &no_mods(), 1, &bindings);
        assert!(result.is_some(), "Right + no mods + 1 must match");
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        let KeyActionSpec::Action(ref s) = actions[0];
        assert_eq!(s, "Copy");
    }

    #[test]
    fn resolve_middle_double_click_matches() {
        let bindings = vec![make_binding("Middle", "", 2, "Paste")];
        let result = resolve_mouse_binding(MouseButton::Middle, &no_mods(), 2, &bindings);
        assert!(result.is_some(), "Middle + no mods + 2 must match");
    }

    #[test]
    fn resolve_left_ctrl_single_matches() {
        let bindings = vec![make_binding("Left", "Ctrl", 1, "NewTab")];
        let result = resolve_mouse_binding(MouseButton::Left, &ctrl_mods(), 1, &bindings);
        assert!(result.is_some(), "Left + Ctrl + 1 must match");
    }

    #[test]
    fn resolve_button_name_case_insensitive() {
        // "right" (lowercase) in config should still match MouseButton::Right.
        let bindings = vec![make_binding("right", "", 1, "Copy")];
        let result = resolve_mouse_binding(MouseButton::Right, &no_mods(), 1, &bindings);
        assert!(
            result.is_some(),
            "button name must be matched case-insensitively"
        );
    }

    // ── resolve_mouse_binding: negative / non-match cases ────────────────────

    #[test]
    fn resolve_wrong_button_returns_none() {
        let bindings = vec![make_binding("Right", "", 1, "Copy")];
        let result = resolve_mouse_binding(MouseButton::Left, &no_mods(), 1, &bindings);
        assert!(result.is_none(), "Left must NOT match a Right binding");
    }

    #[test]
    fn resolve_wrong_mods_returns_none() {
        let bindings = vec![make_binding("Right", "Ctrl", 1, "Copy")];
        // no modifiers pressed → must not match a Ctrl binding
        let result = resolve_mouse_binding(MouseButton::Right, &no_mods(), 1, &bindings);
        assert!(result.is_none(), "no-mods must NOT match a Ctrl binding");
    }

    #[test]
    fn resolve_wrong_count_returns_none() {
        let bindings = vec![make_binding("Right", "", 2, "Copy")];
        // single click — must not match a double-click binding
        let result = resolve_mouse_binding(MouseButton::Right, &no_mods(), 1, &bindings);
        assert!(
            result.is_none(),
            "single click must NOT match a double-click binding"
        );
    }

    #[test]
    fn resolve_empty_bindings_returns_none() {
        let result = resolve_mouse_binding(MouseButton::Right, &no_mods(), 1, &[]);
        assert!(result.is_none(), "empty binding list must return None");
    }

    #[test]
    fn resolve_shift_mods_matches() {
        let bindings = vec![make_binding("Middle", "Shift", 1, "SelectAll")];
        let result = resolve_mouse_binding(MouseButton::Middle, &shift_mods(), 1, &bindings);
        assert!(result.is_some(), "Middle + Shift + 1 must match");
    }

    // ── send: action bytes ────────────────────────────────────────────────────

    #[test]
    fn send_action_bytes_decoded_correctly() {
        let spec = KeyActionSpec::Action("send:ls -la\\n".to_string());
        let bytes = spec.as_send_bytes().expect("must decode send: prefix");
        // "ls -la\n" — the \\n in the Rust string literal is a literal backslash+n
        // which decode_send_string should turn into LF.
        assert!(bytes.ends_with(b"\n"), "trailing \\n must decode to LF");
        assert!(
            bytes.starts_with(b"ls -la"),
            "text prefix must be preserved"
        );
    }

    #[test]
    fn action_name_returns_none_for_send_spec() {
        let spec = KeyActionSpec::Action("send:hello".to_string());
        assert!(
            spec.action_name().is_none(),
            "action_name() must return None for a send: spec"
        );
    }

    #[test]
    fn as_send_bytes_returns_none_for_named_action() {
        let spec = KeyActionSpec::Action("Copy".to_string());
        assert!(
            spec.as_send_bytes().is_none(),
            "as_send_bytes() must return None for a named-action spec"
        );
    }

    // ── first-match wins ──────────────────────────────────────────────────────

    #[test]
    fn first_matching_binding_wins() {
        let bindings = vec![
            make_binding("Right", "", 1, "Copy"),
            make_binding("Right", "", 1, "Paste"),
        ];
        let result = resolve_mouse_binding(MouseButton::Right, &no_mods(), 1, &bindings);
        assert!(result.is_some());
        // The first binding's action must be returned.
        let KeyActionSpec::Action(ref s) = result.unwrap()[0];
        assert_eq!(s, "Copy", "first matching binding must win");
    }
}
