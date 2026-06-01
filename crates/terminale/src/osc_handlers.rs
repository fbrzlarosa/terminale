//! Emulator-event handling: OSC clipboard, title, bell, PtyWrite,
//! notifications, palette changes, and link autodetection.
//! Also: PTY drain (`drain_pty_output`), `advance_caught`, `scroll_after_output`.
//! Exit-behavior: handles `terminal.exit_behavior` (Close/Hold/CloseOnCleanExit)
//! when a pane's PTY EOF is detected.

use crate::{DetectedLink, Pane, RunningState};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use terminale_term::Emulator;

// ── Exit-behavior runtime state ───────────────────────────────────────────────

/// Encoding: 0 = Close (default), 1 = Hold, 2 = CloseOnCleanExit.
/// Updated via [`update_exit_behavior`]; read in [`drain_pty_output`].
static EXIT_BEHAVIOR_CODE: AtomicU8 = AtomicU8::new(0);

/// Update the runtime exit-behavior setting. Call this from the settings
/// window (or any config-apply path) whenever
/// `config.terminal.exit_behavior` changes.
pub(crate) fn update_exit_behavior(behavior: terminale_config::ExitBehavior) {
    let code: u8 = match behavior {
        terminale_config::ExitBehavior::Close => 0,
        terminale_config::ExitBehavior::Hold => 1,
        terminale_config::ExitBehavior::CloseOnCleanExit => 2,
    };
    EXIT_BEHAVIOR_CODE.store(code, Ordering::Relaxed);
}

fn current_exit_behavior() -> terminale_config::ExitBehavior {
    match EXIT_BEHAVIOR_CODE.load(Ordering::Relaxed) {
        1 => terminale_config::ExitBehavior::Hold,
        2 => terminale_config::ExitBehavior::CloseOnCleanExit,
        _ => terminale_config::ExitBehavior::Close,
    }
}

// ── scroll_after_output ───────────────────────────────────────────────────────

/// Where the scroll offset should land after new output arrives. Follows the
/// live edge only when the user was already at the bottom (`scroll_before ==
/// 0`); otherwise it keeps the *same content* in view by advancing the offset
/// by however many lines just spilled into the scrollback — so reading
/// history isn't interrupted by a prompt repaint or a background command.
pub(crate) fn scroll_after_output(scroll_before: usize, history_before: usize, history_after: usize) -> usize {
    if scroll_before == 0 {
        0
    } else {
        let new_lines = history_after.saturating_sub(history_before);
        (scroll_before + new_lines).min(history_after)
    }
}

// ── advance_caught ────────────────────────────────────────────────────────────

/// Push `chunk` into the emulator, catching any panic so a malformed
/// escape sequence (or a bug in alacritty's parser) can only kill one
/// tab — not the entire window. Returns `false` when the emulator
/// panicked; the caller is expected to mark the tab as crashed.
pub(crate) fn advance_caught(emulator: &Arc<Mutex<Emulator>>, chunk: &[u8]) -> bool {
    let emu = Arc::clone(emulator);
    let chunk = chunk.to_vec();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        emu.lock().advance(&chunk);
    }));
    result.is_ok()
}

// ── pane_is_busy ──────────────────────────────────────────────────────────────

/// Returns `true` when `pane` is considered "busy" — i.e. a command is
/// running or PTY output arrived recently.
///
/// Two signals are checked:
///
/// 1. **OSC 133**: `emulator.semantic().is_command_running()` is `true` when
///    a `C` mark was seen but the matching `D` has not yet arrived. This is
///    the authoritative signal for shells with shell integration.
///
/// 2. **Output activity fallback**: for shells without shell integration, the
///    pane is considered busy when it received non-trivial PTY output within
///    the last 250 ms (`last_output_at` is `Some` and the elapsed time is
///    below the threshold). The 250 ms window is long enough to cover typical
///    command output bursts without making the spinner linger noticeably at
///    idle prompts.
///
/// # Lock ordering
///
/// Acquires the emulator mutex briefly to read the semantic state, then
/// releases it before returning. The caller must **not** hold any emulator
/// lock when calling this function.
pub(crate) fn pane_is_busy(pane: &Pane) -> bool {
    // OSC 133 path — most accurate.
    if pane.emulator.lock().semantic().is_command_running() {
        return true;
    }
    // Fallback: recent output activity.
    pane.last_output_at
        .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
}

// ── drain_pty_output ──────────────────────────────────────────────────────────

pub(crate) fn drain_pty_output(state: &mut RunningState) -> bool {
    let mut any = false;
    let active = state.active_tab;
    let mut active_events: Vec<terminale_term::EmulatorEvent> = Vec::new();
    let mut scroll_target: Option<usize> = None;
    // Panes whose PTY channel just closed (EOF). Collected here and processed
    // after the tab-borrow ends so we can call close_tab / close_focused_pane
    // without conflicting borrows. Format: (tab_idx, pane_id, is_focused).
    let mut exited_panes: Vec<(usize, crate::PaneId, bool)> = Vec::new();
    // Set to true when the focused pane processes at least one byte — used
    // to reset the suggestion-bar idle timer after the tabs borrow ends.
    let mut focused_got_bytes = false;
    // Iterate every tab + every pane inside each tab so split-pane
    // siblings (and background tabs) drain their PTYs too — otherwise
    // a non-focused pane's bytes pile up in its `output_rx` channel
    // and stale-out the rendered grid.
    if let Some(tab) = state.tabs.get_mut(active) {
        let focused_id = tab.focused;
        let mut active_pane_changed = false;
        let mut pane_ids: Vec<crate::PaneId> = tab.panes.keys().copied().collect();
        // Drain the FOCUSED pane first so `scroll_target` reflects its
        // before/after history sizes (the scroll-follow logic only
        // applies to the focused pane).
        pane_ids.sort_by_key(|id| i32::from(*id != focused_id));
        for pane_id in pane_ids {
            let is_focused = pane_id == focused_id;
            let Some(pane) = tab.panes.get_mut(&pane_id) else {
                continue;
            };
            if pane.crashed {
                continue;
            }
            let scroll_before = pane.scroll_lines;
            let history_before = pane.emulator.lock().history_size();
            let mut pane_got_bytes = false;
            // Drain the channel, distinguishing empty from disconnected.
            loop {
                match pane.output_rx.try_recv() {
                    Ok(chunk) => {
                        if !advance_caught(&pane.emulator, &chunk) {
                            pane.crashed = true;
                            tracing::error!(
                                profile = %pane.profile_name,
                                "pane emulator panicked while processing PTY data; pane marked as crashed"
                            );
                            break;
                        }
                        pane_got_bytes = true;
                        // Track output-activity timestamp for the busy-spinner
                        // fallback (shells without OSC 133 shell integration).
                        // Only count non-trivial chunks to avoid spinning on
                        // single-byte keystroke echo at an idle prompt.
                        if chunk.len() > 1 || chunk.contains(&b'\n') {
                            pane.last_output_at = Some(Instant::now());
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        // PTY EOF — child process exited. Apply exit_behavior.
                        tracing::debug!(
                            profile = %pane.profile_name,
                            tab = active,
                            pane = pane_id,
                            "PTY EOF detected"
                        );
                        exited_panes.push((active, pane_id, is_focused));
                        // Mark crashed so we don't re-enter this branch on
                        // future drain calls (Disconnected keeps returning
                        // Disconnected). The actual hold-vs-close logic runs
                        // below after the borrow ends.
                        pane.crashed = true;
                        break;
                    }
                }
            }
            if pane_got_bytes && is_focused {
                any = true;
                active_pane_changed = true;
                focused_got_bytes = true;
                let history_after = pane.emulator.lock().history_size();
                scroll_target = Some(scroll_after_output(
                    scroll_before,
                    history_before,
                    history_after,
                ));
            }
            // Collect events from the focused pane only (bell etc.
            // come from the foreground program).
            if is_focused {
                active_events = pane.emulator.lock().drain_events();
            } else {
                let _ = pane.emulator.lock().drain_events();
            }
        }
        let _ = active_pane_changed;
    }
    // Reset the suggestion-bar idle timer when the focused pane received bytes.
    // Done here (after the tabs borrow ends) to satisfy the borrow checker.
    if focused_got_bytes {
        state.suggestions.note_output();
    }
    // Follow the live edge when at the bottom; otherwise hold the user's
    // scrollback position steady as new lines push into history.
    if let Some(target) = scroll_target {
        if let Some(tab) = state.tabs.get_mut(active) {
            tab.scroll_lines = target;
        }
        state.renderer.set_scroll_lines(target);
    }
    // Also drain background tabs (so when user switches back they see
    // the accumulated state rather than empty buffers). Discard their
    // emulator events for now — bell from a backgrounded tab is too
    // noisy without per-tab dimming, OSC 52 has to come from the
    // focused app anyway.
    for (idx, tab) in state.tabs.iter_mut().enumerate() {
        if idx == active {
            continue;
        }
        let mut got_bytes = false;
        let pane_ids: Vec<crate::PaneId> = tab.panes.keys().copied().collect();
        for pane_id in pane_ids {
            let Some(pane) = tab.panes.get_mut(&pane_id) else {
                continue;
            };
            if pane.crashed {
                continue;
            }
            loop {
                match pane.output_rx.try_recv() {
                    Ok(chunk) => {
                        if !advance_caught(&pane.emulator, &chunk) {
                            pane.crashed = true;
                            tracing::error!(
                                profile = %pane.profile_name,
                                "background pane emulator panicked; pane marked as crashed"
                            );
                            break;
                        }
                        got_bytes = true;
                        // Track output-activity for the busy-spinner fallback —
                        // applies to background panes too so their headers show
                        // the spinner when the tab is switched to.
                        if chunk.len() > 1 || chunk.contains(&b'\n') {
                            pane.last_output_at = Some(Instant::now());
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        tracing::debug!(
                            profile = %pane.profile_name,
                            tab = idx,
                            pane = pane_id,
                            "background PTY EOF detected"
                        );
                        exited_panes.push((idx, pane_id, false));
                        pane.crashed = true;
                        break;
                    }
                }
            }
            let _ = pane.emulator.lock().drain_events();
        }
        if got_bytes {
            tab.unread = true;
        }
    }
    // Apply events from the focused pane to host state.
    for event in active_events {
        handle_emulator_event(state, event);
    }
    // ── Handle PTY exits ────────────────────────────────────────────────────
    // Process collected exits now that no tab/pane borrow is held. The
    // behavior (Close / Hold / CloseOnCleanExit) is read from the module-level
    // atomic so it doesn't need to live on RunningState.
    if !exited_panes.is_empty() {
        handle_pane_exits(state, &exited_panes);
        any = true; // need a repaint regardless of behavior
    }
    // Refresh URL autodetection on the focused pane when anything changed.
    if any {
        refresh_autodetect_links(state);
        // Re-evaluate context rules for all tabs; rebuild the tab bar when
        // any tab's auto_color or auto_badge changed (cwd update via OSC 7
        // is the primary trigger since it arrives with PTY data).
        if refresh_context_rules(state) {
            crate::tabs::refresh_tab_bar(state);
        }
        // Track the current working directory of the focused pane in the
        // directory-jump frecency store. OSC 7 delivers the cwd via the
        // emulator's `current_dir()` method, so we read it here rather
        // than hooking into the EmulatorEvent path.
        update_dir_jump(state);
    }
    any
}

/// Process PTY-exited panes according to the configured [`ExitBehavior`].
///
/// - **Close**: closes the pane immediately (and the whole tab if it was the
///   only pane), using the same code path as the user's "close pane" action.
///   Only auto-closes the active tab's focused pane; background-tab exits
///   always stay in hold mode (less disruptive to the user).
/// - **Hold / CloseOnCleanExit with non-0 exit**: injects a dim status line
///   into the pane's emulator buffer so the user can read the last output,
///   then leaves the pane open (`crashed = true` already prevents re-drain).
///
/// `exited` is a list of `(tab_idx, pane_id, is_focused)` triples.
fn handle_pane_exits(state: &mut RunningState, exited: &[(usize, crate::PaneId, bool)]) {
    let behavior = current_exit_behavior();
    for &(tab_idx, pane_id, _is_focused) in exited {
        // Poll the child's exit status from the session handle.  The PTY
        // reader thread has already seen EOF, so by the time we get here the
        // child has almost certainly exited; try_wait is non-blocking and
        // returns Some immediately in the normal case.  If for any reason the
        // code isn't available yet (race on some platforms), we fall back to
        // None — which is the conservative "unknown" case: Close still
        // closes, Hold still holds, CloseOnCleanExit stays open (correct,
        // since we can't confirm it was clean).
        let exit_code: Option<i32> = state
            .tabs
            .get(tab_idx)
            .and_then(|t| t.panes.get(&pane_id))
            .and_then(|p| p.session.try_exit_status());

        // Enqueue session_exit hook before closing or holding the pane so
        // plugins see the event regardless of the exit behavior.
        state.pending_hook_session_exit.push((pane_id, exit_code.unwrap_or(0)));
        // Clean up the command-end counter for this pane so a future pane
        // that gets the same id (after the old one is removed) starts fresh.
        state.hook_cmd_end_fired.remove(&pane_id);

        let should_close = behavior.should_close(exit_code);

        if should_close && tab_idx == state.active_tab {
            // Auto-close only the active tab's pane — background exits stay
            // held so the unread dot signals them without disrupting the user.
            crate::tabs::close_exited_pane(state, pane_id);
        } else {
            // Hold: pane is already marked `crashed = true` (prevents re-drain).
            // Inject a dim status line so the user knows the process ended.
            inject_exit_message(state, tab_idx, pane_id);
        }
    }
}

/// Inject a dim "[process exited]" banner directly into the pane's emulator
/// buffer. Called in Hold mode after PTY EOF.
fn inject_exit_message(state: &mut RunningState, tab_idx: usize, pane_id: crate::PaneId) {
    if let Some(tab) = state.tabs.get_mut(tab_idx) {
        if let Some(pane) = tab.panes.get_mut(&pane_id) {
            // Dim ANSI escape (\x1b[2m…\x1b[0m) + CR+LF.
            // The em-dash is encoded as UTF-8 bytes (U+2014 = 0xE2 0x80 0x94).
            let msg = b"\r\n\x1b[2m[process exited \xe2\x80\x94 close this pane to dismiss]\x1b[0m\r\n";
            let _ = advance_caught(&pane.emulator, msg);
        }
    }
    state.window.request_redraw();
}

// ── handle_bell / system_beep ─────────────────────────────────────────────────

pub(crate) fn handle_bell(state: &mut RunningState) {
    use terminale_config::BellMode;
    match state.bell_mode {
        BellMode::None => {}
        BellMode::Visual => {
            state.renderer.trigger_visual_bell();
            state.window.request_redraw();
        }
        BellMode::Audio => {
            system_beep();
            state
                .window
                .request_user_attention(Some(winit::window::UserAttentionType::Informational));
        }
        BellMode::Both => {
            state.renderer.trigger_visual_bell();
            system_beep();
            state
                .window
                .request_user_attention(Some(winit::window::UserAttentionType::Informational));
            state.window.request_redraw();
        }
    }
}

/// Play the system "default beep" so an Audio/Both bell is actually
/// audible — `request_user_attention` only flashes the taskbar, it makes
/// no sound. Asynchronous (returns immediately), so safe on the UI thread.
pub(crate) fn system_beep() {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBeep(u_type: u32) -> i32;
        }
        // 0xFFFFFFFF = the standard simple beep.
        unsafe {
            MessageBeep(0xFFFF_FFFF);
        }
    }
}

// ── handle_emulator_event ─────────────────────────────────────────────────────

pub(crate) fn handle_emulator_event(state: &mut RunningState, event: terminale_term::EmulatorEvent) {
    use terminale_term::EmulatorEvent;
    match event {
        EmulatorEvent::ClipboardStore { kind: _, text } => {
            // OSC 52: app writes to the system clipboard. We accept both
            // primary-selection and clipboard requests since on Windows /
            // macOS they're the same destination anyway.
            if state.clipboard_history_capture_osc52 {
                crate::push_clipboard_history(state, text.clone());
            }
            if let Some(cb) = state.clipboard.as_mut() {
                if let Err(e) = cb.set_text(text) {
                    tracing::warn!(?e, "OSC 52 clipboard write failed");
                }
            }
        }
        EmulatorEvent::ClipboardRead { selection } => {
            // OSC 52 READ query (payload `?`). Policy-gated: only reply when
            // the user has explicitly opted in. The default (`deny`) ignores
            // the request entirely — clipboard reads are an exfiltration vector
            // and must not happen silently.
            handle_clipboard_read(state, &selection);
        }
        EmulatorEvent::Title(t) => {
            // Update the OS window title and the active tab's label so a
            // program-announced title (vim, ssh, …) shows in both places.
            // The shell's own `…\powershell.exe` title is filtered as noise.
            match crate::useful_program_title(&t) {
                Some(title) => state.window.set_title(&format!("terminale — {title}")),
                None => state.window.set_title("terminale"),
            }
            let title = t.trim();
            if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                tab.custom_title = if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                };
            }
            state.window.request_redraw();
        }
        EmulatorEvent::Bell => {
            // Bell behaviour is user-configurable — visual flash, system
            // attention beep, both, or fully silenced.
            handle_bell(state);
        }
        EmulatorEvent::PtyWrite(bytes) => {
            // Response generated by alacritty's parser (color queries,
            // device-attribute reports, cursor-position reports). Send
            // straight back into the PTY so the app sees it on stdin.
            if let Some(tab) = state.tabs.get(state.active_tab) {
                let _ = tab.session.write_input(bytes.as_bytes());
            }
        }
        EmulatorEvent::Notification { title, body } => {
            // OSC 9 / OSC 777 desktop notification. Only fire when the
            // window is unfocused AND the user has enabled OS notifications.
            if state.os_notifications && !state.window_focused {
                fire_os_notification(&title, &body);
            }
        }
        EmulatorEvent::PaletteChanged => {
            // A dynamic-colour OSC (4/10/11/12/104/110–112) was processed.
            // The active tab's emulator already holds the new overrides; we
            // just need to repaint so the next frame picks them up.
            state.window.request_redraw();
        }
    }
}

/// Raise an OS desktop notification with `title` and `body`. The call is
/// best-effort: failures are logged at `warn` level and never surface to the
/// user (a notification failing to appear is not a fatal error).
pub(crate) fn fire_os_notification(title: &str, body: &str) {
    let summary = if title.is_empty() { "terminale" } else { title };
    // `notify_rust::Notification::show()` is synchronous on Windows and
    // macOS; on Linux it dispatches via DBus.
    let result = notify_rust::Notification::new()
        .summary(summary)
        .body(body)
        .appname("terminale")
        .show();
    if let Err(e) = result {
        tracing::warn!(error = ?e, "OS notification send failed");
    }
}

// ── refresh_context_rules ─────────────────────────────────────────────────────

/// Re-evaluate `state.context_rules` against every tab's SSH host name and
/// cwd. Updates `tab.auto_color` / `tab.auto_badge` and returns `true` when
/// any tab changed (so the caller knows to rebuild the tab bar).
///
/// Called from `drain_pty_output` (once per frame that produced PTY data)
/// and explicitly after SSH tab creation so the tint appears immediately.
pub(crate) fn refresh_context_rules(state: &mut RunningState) -> bool {
    if state.context_rules.is_empty() {
        // Fast path: no rules configured — ensure all tabs are cleared.
        let mut changed = false;
        for tab in &mut state.tabs {
            if tab.auto_color.is_some() || tab.auto_badge.is_some() {
                tab.auto_color = None;
                tab.auto_badge = None;
                changed = true;
            }
        }
        return changed;
    }

    let mut changed = false;
    // Borrow the rules as a snapshot so we can also borrow state.tabs mutably.
    let rules = state.context_rules.clone();
    for tab in &mut state.tabs {
        let cwd = tab.emulator.lock().current_dir().unwrap_or_default().to_string();
        let host = tab.ssh_host_name.as_str();
        let matched = terminale_config::evaluate_context_rules(&rules, host, &cwd);
        let new_color = matched.and_then(|r| r.tab_color);
        let new_badge = matched.and_then(|r| r.badge.clone());
        if tab.auto_color != new_color || tab.auto_badge != new_badge {
            tab.auto_color = new_color;
            tab.auto_badge = new_badge;
            changed = true;
        }
    }
    changed
}

// ── refresh_autodetect_links ──────────────────────────────────────────────────

/// Walk every visible row of the active tab, run the URL scanner, and
/// push the resulting cell ranges into the renderer's "extra underline"
/// list. Also stashes them in the tab so click handling can resolve a
/// click position back to a URL.
pub(crate) fn refresh_autodetect_links(state: &mut RunningState) {
    let active = state.active_tab;
    let Some(tab) = state.tabs.get_mut(active) else {
        return;
    };
    let (cols, rows) = (tab.cols, tab.rows);
    if cols == 0 || rows == 0 {
        tab.autodetect_links.clear();
        state.renderer.set_extra_underlines(Vec::new());
        return;
    }
    let mut detected: Vec<DetectedLink> = Vec::new();
    let emu = tab.emulator.lock();
    // The shell's announced working directory (OSC 7 / OSC 9;9), used to
    // resolve relative file paths in command output. `None` when the shell
    // hasn't announced one — absolute paths still resolve without it.
    let cwd: Option<std::path::PathBuf> = emu.current_dir().map(std::path::PathBuf::from);
    // Byte offset → cell column for a row (ASCII = 1 cell/char; matches the
    // URL path below). Clamps into the visible grid.
    let to_col = |row_text: &str, byte: usize, last: u16| {
        (row_text[..byte].chars().count() as u16).min(last)
    };
    for r in 0..rows {
        // Autodetect scans the live viewport (scroll 0); hyperlink_under
        // bails when the user is panned into history, so this stays simple.
        let row_text = emu.text_in_range((0, r), (cols.saturating_sub(1), r), 0);
        if row_text.is_empty() {
            continue;
        }
        let last_col = cols.saturating_sub(1);
        // URLs first; remember their byte ranges so a file path inside a
        // `file://` URL isn't double-detected.
        let mut url_ranges: Vec<(usize, usize)> = Vec::new();
        for m in crate::links::scan(&row_text) {
            url_ranges.push((m.start, m.end));
            // Translate byte offsets back to cell columns. Our terminal
            // cells are 1-per-codepoint for ASCII (which URLs are by
            // RFC), so `chars().count()` over the prefix is enough.
            let col_start = to_col(&row_text, m.start, last_col);
            // -1 because the URL ends just before `m.end`.
            let col_end = to_col(&row_text, m.end, last_col).saturating_sub(1);
            detected.push(DetectedLink {
                col_start,
                col_end,
                row: r,
                url: m.url,
                is_path: false,
                line: None,
                column: None,
            });
        }
        // User-configured hyperlink rules (compiled once via
        // `links::update_hyperlink_rules`). Returns empty when the rule list
        // is empty so the built-in scan above is the sole source.
        for m in crate::links::scan_with_rules(&row_text, &url_ranges) {
            url_ranges.push((m.start, m.end));
            let col_start = to_col(&row_text, m.start, last_col);
            let col_end = to_col(&row_text, m.end, last_col).saturating_sub(1);
            detected.push(DetectedLink {
                col_start,
                col_end,
                row: r,
                url: m.url,
                is_path: false,
                line: None,
                column: None,
            });
        }
        // Then existing-on-disk file paths (resolved against the shell cwd).
        for pm in crate::links::scan_paths(&row_text, cwd.as_deref()) {
            if url_ranges.iter().any(|&(s, e)| pm.start < e && s < pm.end) {
                continue; // overlaps a URL we already linked
            }
            let col_start = to_col(&row_text, pm.start, last_col);
            let col_end = to_col(&row_text, pm.end, last_col).saturating_sub(1);
            detected.push(DetectedLink {
                col_start,
                col_end,
                row: r,
                url: pm.path.display().to_string(),
                is_path: true,
                line: pm.line,
                column: pm.column,
            });
        }
    }
    drop(emu);
    tab.autodetect_links = detected;
    // Underline behaviour is controlled by `terminal.link_underline`:
    //   * `Always` → every detected URL gets a persistent accent underline
    //     (paths are excluded — they'd clutter `ls` output / prompts; they
    //     rely on the hover tooltip + pointer cursor instead).
    //   * `Hover`  → no persistent underlines here; the hover handler
    //     underlines just the link under the pointer.
    //   * `Never`  → no underlines at all.
    // `Hover` is the default and avoids leaving a stray accent line under
    // banner URLs printed before any output scrolls.
    match state.link_underline {
        terminale_config::LinkUnderline::Always => {
            let ranges: Vec<(u16, u16, u16)> = state
                .tabs
                .get(active)
                .map(|t| {
                    t.autodetect_links
                        .iter()
                        .filter(|d| !d.is_path)
                        .map(|d| (d.col_start, d.col_end, d.row))
                        .collect()
                })
                .unwrap_or_default();
            state.renderer.set_extra_underlines(ranges);
        }
        terminale_config::LinkUnderline::Hover | terminale_config::LinkUnderline::Never => {
            state.renderer.set_extra_underlines(Vec::new());
        }
    }
}

// ── update_status_bar ────────────────────────────────────────────────────────

/// Build `StatusBarContent` from the active pane's emulator and push it to
/// the renderer. Clears the renderer bar when `config.status_bar.enabled`
/// is `false`.
pub(crate) fn update_status_bar(state: &mut RunningState, config: &terminale_config::Config) {
    if !config.status_bar.enabled {
        state.renderer.set_status_bar(None);
        return;
    }

    // Resolve active tab / pane.
    let Some(tab) = state.tabs.get(state.active_tab) else {
        state.renderer.set_status_bar(None);
        return;
    };
    let emu_arc = &tab.emulator;
    let emu = emu_arc.lock();

    // Resolve profile name.
    let profile_name = if tab.profile_name.is_empty() {
        config.profiles.default.as_deref().unwrap_or_default()
    } else {
        tab.profile_name.as_str()
    };

    // Resolve cwd.
    let cwd_str = emu.current_dir();
    let cwd_path_buf = cwd_str.map(std::path::PathBuf::from);
    let cwd: Option<&std::path::Path> = cwd_path_buf.as_deref();

    // Build user_vars by collecting only the names referenced in the segment
    // lists and looking them up via Emulator::user_var (the public API).
    let mut user_vars = std::collections::HashMap::new();
    let seg_names: Vec<&str> = config
        .status_bar
        .left_segments
        .iter()
        .chain(config.status_bar.right_segments.iter())
        .filter_map(|s| {
            if let terminale_config::StatusSegment::UserVar { name } = s {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();
    for name in seg_names {
        if let Some(val) = emu.user_var(name) {
            user_vars.insert(name.to_string(), val.to_string());
        }
    }

    let ctx = crate::status_bar::StatusContext {
        cwd,
        profile_name,
        tab_index: state.active_tab + 1,
        tab_count: state.tabs.len(),
        user_vars: &user_vars,
        now: chrono::Local::now(),
    };

    let sb_cfg = &config.status_bar;
    let mut left = crate::status_bar::compose(&sb_cfg.left_segments, &ctx);
    let right = crate::status_bar::compose(&sb_cfg.right_segments, &ctx);
    let at_bottom = sb_cfg.position == terminale_config::StatusBarPosition::Bottom;

    // Prepend the leader-mode indicator when a key-table is active so the
    // user can see they are inside a modal key sequence.
    if let Some(ref akt) = state.active_key_table {
        if let Some(table) = config.keybinds.key_tables.get(akt.table_idx) {
            let indicator = format!("[{}] ... ", table.name);
            if left.is_empty() {
                left = indicator;
            } else {
                left = format!("{indicator}  {left}");
            }
        }
    }

    state
        .renderer
        .set_status_bar(Some(terminale_render::StatusBarContent { left, right, at_bottom }));
}

// ── hyperlink helpers ─────────────────────────────────────────────────────────

/// Resolve a clickable URL underneath physical pixel `pos_px`. Tries
/// OSC 8 hyperlinks first (authoritative) then falls back to the
/// autodetected URL ranges scanned from the visible buffer.
pub(crate) fn hyperlink_under(state: &RunningState, pos_px: (f32, f32)) -> Option<String> {
    let (col, row) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1)?;
    let scroll = state.renderer.scroll_lines();
    let tab = state.tabs.get(state.active_tab)?;
    if let Some(uri) = tab.emulator.lock().cell_hyperlink(col, row, scroll) {
        return Some(uri);
    }
    // Autodetect ranges are stored in viewport-coords with `scroll=0`. If
    // the user is panning into history, that bookkeeping doesn't apply —
    // bail out for now.
    if scroll != 0 {
        return None;
    }
    tab.autodetect_links
        .iter()
        .find(|d| d.row == row && col >= d.col_start && col <= d.col_end)
        .map(|d| d.url.clone())
}

/// OSC 8 hyperlink under the pointer (authoritative, app-declared). `None`
/// when there isn't one — callers then fall back to autodetected links.
pub(crate) fn osc8_under(state: &RunningState, pos_px: (f32, f32)) -> Option<String> {
    let (col, row) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1)?;
    let scroll = state.renderer.scroll_lines();
    let tab = state.tabs.get(state.active_tab)?;
    tab.emulator.lock().cell_hyperlink(col, row, scroll)
}

/// The autodetected link (URL or file path) under the pointer, cloned so
/// the caller can act on it without holding a borrow of `state`.
pub(crate) fn autodetect_link_under(state: &RunningState, pos_px: (f32, f32)) -> Option<DetectedLink> {
    let (col, row) = state.renderer.cell_at_pixel(pos_px.0, pos_px.1)?;
    if state.renderer.scroll_lines() != 0 {
        return None;
    }
    let tab = state.tabs.get(state.active_tab)?;
    tab.autodetect_links
        .iter()
        .find(|d| d.row == row && col >= d.col_start && col <= d.col_end)
        .cloned()
}

/// Open a detected link. File paths with a configured `editor.command`
/// launch the editor at the parsed `line:col`; everything else (URLs, or
/// paths with no editor configured) opens via the OS default handler.
pub(crate) fn open_detected_link(state: &RunningState, link: &DetectedLink) {
    if link.is_path && !state.editor_command.is_empty() {
        if let Some((prog, args)) = crate::links::build_editor_invocation(
            &state.editor_command,
            std::path::Path::new(&link.url),
            link.line,
            link.column,
        ) {
            match std::process::Command::new(&prog).args(&args).spawn() {
                Ok(_) => return,
                Err(e) => tracing::warn!(
                    ?e,
                    %prog,
                    "editor launch failed; falling back to default open"
                ),
            }
        }
    }
    if let Err(e) = open::that(&link.url) {
        tracing::warn!(?e, url = %link.url, "open link failed");
    }
}

// ── osc52_base64_encode ───────────────────────────────────────────────────────

/// Encode `bytes` as standard Base64 (RFC 4648 alphabet, with `=` padding).
///
/// We use an inline encoder rather than pulling in the `base64` crate as a
/// new direct dependency, since this single call-site does not justify a new
/// dep. The implementation follows the standard MIME/URL-safe alphabet and
/// is correct for all inputs.
fn osc52_base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in chunks.by_ref() {
        let b0 = chunk[0] as u32;
        let b1 = chunk[1] as u32;
        let b2 = chunk[2] as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize]);
        out.push(TABLE[((n >> 12) & 0x3f) as usize]);
        out.push(TABLE[((n >> 6) & 0x3f) as usize]);
        out.push(TABLE[(n & 0x3f) as usize]);
    }
    match chunks.remainder() {
        [b0] => {
            let n = (*b0 as u32) << 16;
            out.push(TABLE[((n >> 18) & 0x3f) as usize]);
            out.push(TABLE[((n >> 12) & 0x3f) as usize]);
            out.push(b'=');
            out.push(b'=');
        }
        [b0, b1] => {
            let n = ((*b0 as u32) << 16) | ((*b1 as u32) << 8);
            out.push(TABLE[((n >> 18) & 0x3f) as usize]);
            out.push(TABLE[((n >> 12) & 0x3f) as usize]);
            out.push(TABLE[((n >> 6) & 0x3f) as usize]);
            out.push(b'=');
        }
        _ => {}
    }
    // SAFETY: TABLE contains only ASCII bytes; `=` is ASCII.
    String::from_utf8(out).expect("base64 output is always valid UTF-8")
}

// ── handle_clipboard_read ─────────────────────────────────────────────────────

/// Handle an OSC 52 clipboard READ query (`? ` payload) from the active pane.
///
/// Respects the `terminal.clipboard_read` policy:
/// - `deny` (default): no reply — the query is silently dropped.
/// - `allow`: read the system clipboard, base64-encode the UTF-8 text, and
///   write `ESC ] 52 ; <selection> ; <base64> ST` back to the focused pane's
///   PTY stdin.
///
/// The `selection` string is echoed verbatim so the requesting program can
/// match the response to its own query.
fn handle_clipboard_read(state: &mut RunningState, selection: &str) {
    use terminale_config::ClipboardReadPolicy;

    match state.clipboard_read_policy {
        ClipboardReadPolicy::Deny => {
            // Default: ignore silently. No reply means the program gets
            // nothing — correct and safe behaviour.
            tracing::debug!(
                selection,
                "OSC 52 clipboard read query denied (policy=deny)"
            );
        }
        ClipboardReadPolicy::Allow => {
            // Read the system clipboard, base64-encode it, and send the OSC 52
            // response back to the focused pane's PTY.
            let text = if let Some(cb) = state.clipboard.as_mut() {
                match cb.get_text() {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(
                            ?e,
                            selection,
                            "OSC 52 clipboard read failed (policy=allow)"
                        );
                        return;
                    }
                }
            } else {
                tracing::debug!(
                    selection,
                    "OSC 52 clipboard read: no clipboard backend available"
                );
                return;
            };

            // Encode payload and build the OSC 52 response sequence.
            let encoded = osc52_base64_encode(text.as_bytes());
            // OSC 52 ; <selection> ; <base64> ST  (ST = ESC \)
            let response = format!("\x1b]52;{selection};{encoded}\x1b\\");

            if let Some(tab) = state.tabs.get(state.active_tab) {
                if let Err(e) = tab.session.write_input(response.as_bytes()) {
                    tracing::warn!(
                        ?e,
                        selection,
                        "OSC 52 clipboard read: PTY write failed"
                    );
                }
            }
        }
    }
}

// ── update_dir_jump ───────────────────────────────────────────────────────────

/// Track the focused pane's current working directory in the directory-jump
/// frecency store. Called from [`drain_pty_output`] after any PTY data arrives
/// (which may carry an OSC 7 cwd update).
///
/// Does nothing when `state.dir_jump_enabled` is `false` or when the focused
/// pane has not yet reported a working directory via OSC 7.
pub(crate) fn update_dir_jump(state: &mut RunningState) {
    if !state.dir_jump_enabled {
        return;
    }
    let active = state.active_tab;
    let cwd = state
        .tabs
        .get(active)
        .and_then(|t| t.emulator.lock().current_dir().map(std::string::ToString::to_string));
    let Some(cwd) = cwd else {
        return;
    };
    let now_unix = chrono::Utc::now().timestamp();
    let changed = state
        .dir_jump_store
        .record(&cwd, now_unix, state.dir_jump_max_tracked);
    if changed && state.dir_jump_persist {
        if let Some(path) = crate::dir_jump::history_path() {
            state.dir_jump_store.save(&path);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::osc52_base64_encode;

    // ── osc52_base64_encode ───────────────────────────────────────────────────

    /// The empty byte slice must encode to an empty string (no padding needed).
    #[test]
    fn base64_empty_input() {
        assert_eq!(osc52_base64_encode(b""), "");
    }

    /// Single-byte input pads with two `=` characters.
    #[test]
    fn base64_one_byte_pads_two() {
        // 'M' = 0x4d = 0b01001101
        // 01001101 00 → N T = =
        assert_eq!(osc52_base64_encode(b"M"), "TQ==");
    }

    /// Two-byte input pads with one `=` character.
    #[test]
    fn base64_two_bytes_pads_one() {
        // "Ma" = 0x4d 0x61
        assert_eq!(osc52_base64_encode(b"Ma"), "TWE=");
    }

    /// Three-byte input produces exactly four characters with no padding.
    #[test]
    fn base64_three_bytes_no_padding() {
        assert_eq!(osc52_base64_encode(b"Man"), "TWFu");
    }

    /// "hello" → standard RFC 4648 encoding.
    #[test]
    fn base64_hello() {
        assert_eq!(osc52_base64_encode(b"hello"), "aGVsbG8=");
    }

    /// "test" → base64 used by the existing OSC 52 demo in main.rs.
    #[test]
    fn base64_test_word() {
        assert_eq!(osc52_base64_encode(b"test"), "dGVzdA==");
    }

    /// All zero bytes encode correctly.
    #[test]
    fn base64_all_zeros() {
        assert_eq!(osc52_base64_encode(b"\x00\x00\x00"), "AAAA");
    }

    /// All 0xFF bytes encode correctly.
    #[test]
    fn base64_all_ff() {
        assert_eq!(osc52_base64_encode(b"\xff\xff\xff"), "////");
    }

    /// Round-trip: encode then manually verify against known-good output.
    #[test]
    fn base64_longer_string() {
        // "The quick brown fox" — cross-check against well-known value.
        let encoded = osc52_base64_encode(b"The quick brown fox");
        assert_eq!(encoded, "VGhlIHF1aWNrIGJyb3duIGZveA==");
    }

    // ── OSC 52 reply format ───────────────────────────────────────────────────

    /// Verify the OSC 52 response format: ESC ] 52 ; <sel> ; <b64> ESC \
    #[test]
    fn osc52_reply_format_correct() {
        let selection = "c";
        let text = "hello";
        let encoded = osc52_base64_encode(text.as_bytes());
        let response = format!("\x1b]52;{selection};{encoded}\x1b\\");
        assert!(response.starts_with("\x1b]52;c;"), "must start with OSC 52 sequence");
        assert!(response.ends_with("\x1b\\"), "must end with ST (ESC \\)");
        assert!(response.contains("aGVsbG8="), "must contain base64-encoded text");
    }
}
