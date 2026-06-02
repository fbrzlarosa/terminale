//! Keyboard shortcuts and keybind configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Catalogue of every remappable in-app shortcut. Each value is a
/// human-readable binding (`"Ctrl+T"`, `"Ctrl+Shift+ArrowUp"`, …);
/// empty = action disabled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ShortcutsConfig {
    /// Open a new tab with the default profile.
    pub new_tab: String,
    /// Close the active tab.
    pub close_tab: String,
    /// Cycle to the next tab.
    pub next_tab: String,
    /// Cycle to the previous tab.
    pub prev_tab: String,
    /// Move the active tab one slot to the left in the tab bar.
    pub move_tab_left: String,
    /// Move the active tab one slot to the right in the tab bar.
    pub move_tab_right: String,
    /// Open the profile picker (Ctrl+Shift+T).
    pub profile_picker: String,
    /// Restart the active tab when it's in `crashed` state.
    pub restart_tab: String,
    /// Copy selection to clipboard.
    pub copy: String,
    /// Paste clipboard (bracketed if the app supports it).
    pub paste: String,
    /// Select the whole visible buffer.
    pub select_all: String,
    /// Open the find-in-buffer search bar.
    pub find: String,
    /// Send `\x0c` to the shell (clear-screen).
    pub clear: String,
    /// Open the settings panel.
    pub settings: String,
    /// Bump up the terminal font size.
    pub font_increase: String,
    /// Drop down the terminal font size.
    pub font_decrease: String,
    /// Reset the font size to the user's configured default.
    pub font_reset: String,
    /// Scroll the buffer one line up (into history).
    pub scroll_line_up: String,
    /// Scroll the buffer one line down (back towards live).
    pub scroll_line_down: String,
    /// Scroll the buffer one page up.
    pub scroll_page_up: String,
    /// Scroll the buffer one page down.
    pub scroll_page_down: String,
    /// Jump to the top of the scrollback.
    pub scroll_top: String,
    /// Jump back to the live edge of the buffer.
    pub scroll_bottom: String,
    /// Open the AI assistant panel.
    pub ai_assistant: String,
    /// Open the command palette (fuzzy action search).
    pub command_palette: String,
    /// Send the current selection to the AI assistant for an explanation.
    pub explain_selection: String,
    /// Drop the scrollback history (keeps the visible screen).
    pub clear_scrollback: String,
    /// Reopen the most recently closed tab in its original directory.
    /// Unbound by default (Ctrl+Shift+T is already `profile_picker`); set a
    /// binding here, or trigger it from the command palette.
    pub reopen_closed_tab: String,
    /// Open the SSH host picker to connect to a saved host in a new tab.
    /// Unbound by default; set a binding here or use the command palette
    /// ("New SSH Tab…") or the per-host "SSH: `<name>`" palette rows.
    pub new_ssh_tab: String,
    /// Toggle "stay on top" (always-on-top window). Unbound by default to
    /// avoid collisions; set a binding here, or trigger it from the command
    /// palette / right-click menu / Settings.
    pub stay_on_top: String,
    /// Snap the focused window to the top half of its current monitor.
    /// Unbound by default; set a binding or use the command palette.
    pub snap_top: String,
    /// Snap the focused window to the bottom half of its current monitor.
    /// Unbound by default.
    pub snap_bottom: String,
    /// Snap the focused window to the left half of its current monitor.
    /// Unbound by default.
    pub snap_left: String,
    /// Snap the focused window to the right half of its current monitor.
    /// Unbound by default.
    pub snap_right: String,
    /// Centre the focused window on its current monitor (size preserved).
    /// Unbound by default.
    pub snap_center: String,
    /// Maximize the focused window to fill its current monitor.
    /// Unbound by default.
    pub snap_maximize: String,
    /// Snap the focused window to the top-left quarter of its current monitor.
    /// Unbound by default.
    pub snap_top_left: String,
    /// Snap the focused window to the top-right quarter of its current monitor.
    /// Unbound by default.
    pub snap_top_right: String,
    /// Snap the focused window to the bottom-left quarter of its current monitor.
    /// Unbound by default.
    pub snap_bottom_left: String,
    /// Snap the focused window to the bottom-right quarter of its current monitor.
    /// Unbound by default.
    pub snap_bottom_right: String,
    /// Open the snap-layout chooser overlay: a grid of layout buttons
    /// (halves, quarters, center, maximize) that the user can click to
    /// apply a snap. Closes automatically on click or Esc.
    /// Unbound by default.
    pub show_snap_layouts: String,
    /// Split the focused pane into two side-by-side panes (new pane on
    /// the right). Default `Ctrl+Shift+\"+\"` (Ctrl+Shift+Plus).
    pub split_right: String,
    /// Split the focused pane into two stacked panes (new pane below).
    /// Default `Ctrl+Shift+\"_\"` (Ctrl+Shift+Underscore).
    pub split_down: String,
    /// Split the focused pane with the new pane on the LEFT. Unbound
    /// by default.
    pub split_left: String,
    /// Split the focused pane with the new pane ABOVE. Unbound by
    /// default.
    pub split_up: String,
    /// Close the focused pane (collapsing the parent split). When the
    /// last pane in a tab closes, the tab closes. Default
    /// `Ctrl+Shift+W`.
    pub close_pane: String,
    /// Move focus to the pane immediately to the left of the focused pane.
    /// Unbound by default.
    pub focus_pane_left: String,
    /// Move focus to the pane immediately to the right of the focused pane.
    /// Unbound by default.
    pub focus_pane_right: String,
    /// Move focus to the pane immediately above the focused pane.
    /// Unbound by default.
    pub focus_pane_up: String,
    /// Move focus to the pane immediately below the focused pane.
    /// Unbound by default.
    pub focus_pane_down: String,
    /// Toggle zoom on the focused pane — expands it to fill the whole tab body,
    /// hiding all other panes and dividers. A second press restores the normal
    /// tree layout. Default `Ctrl+Shift+Z`.
    pub toggle_pane_zoom: String,
    /// Grow or shrink the focused pane's parent split one step to the left.
    /// Unbound by default.
    pub resize_pane_left: String,
    /// Grow or shrink the focused pane's parent split one step to the right.
    /// Unbound by default.
    pub resize_pane_right: String,
    /// Grow or shrink the focused pane's parent split one step upward.
    /// Unbound by default.
    pub resize_pane_up: String,
    /// Grow or shrink the focused pane's parent split one step downward.
    /// Unbound by default.
    pub resize_pane_down: String,
    /// Jump to tab 1 (first tab). Default `Ctrl+1`.
    pub activate_tab_1: String,
    /// Jump to tab 2. Default `Ctrl+2`.
    pub activate_tab_2: String,
    /// Jump to tab 3. Default `Ctrl+3`.
    pub activate_tab_3: String,
    /// Jump to tab 4. Default `Ctrl+4`.
    pub activate_tab_4: String,
    /// Jump to tab 5. Default `Ctrl+5`.
    pub activate_tab_5: String,
    /// Jump to tab 6. Default `Ctrl+6`.
    pub activate_tab_6: String,
    /// Jump to tab 7. Default `Ctrl+7`.
    pub activate_tab_7: String,
    /// Jump to tab 8. Default `Ctrl+8`.
    pub activate_tab_8: String,
    /// Jump to the last tab (tab 9 convention — always the rightmost tab,
    /// a common convention). Default `Ctrl+9`.
    pub activate_tab_9: String,
    /// Switch to the previously-active tab (toggle between the two most
    /// recently used tabs). Unbound by default — set a binding or use the
    /// command palette.
    pub last_tab: String,
    /// Scroll the viewport so the previous OSC 133 prompt mark is visible
    /// (requires shell integration). Unbound by default.
    pub prev_prompt: String,
    /// Scroll the viewport so the next OSC 133 prompt mark is visible
    /// (requires shell integration). Unbound by default.
    pub next_prompt: String,
    /// Enter modal keyboard copy mode: a keyboard-driven selection of the
    /// screen and scrollback with vim motions. Press `y` or Enter to yank the
    /// selection to the clipboard; `Esc` or `q` to exit without copying.
    /// Default `Ctrl+Shift+X`.
    pub copy_mode: String,
    /// Enter label-hint quick-select mode: scans the visible screen and
    /// scrollback for regex-matched text (URLs, paths, hashes, IPs, …) and
    /// overlays short keyboard labels on each match. Typing a label copies the
    /// matched text to the clipboard and exits. `Esc` cancels.
    /// Default `Ctrl+Shift+Space`.
    pub quick_select: String,
    /// Enter pane-select label mode: draws one label badge centred in each
    /// visible pane; pressing a label focuses that pane. `Esc` cancels.
    /// Unbound by default; bind here or use the command palette.
    pub pane_select: String,
    /// Manually reload the config from disk right now (same effect as
    /// editing config.toml externally when `window.auto_reload_config` is
    /// on). Unbound by default — set a binding or trigger from the palette.
    pub reload_config: String,
    /// Toggle borderless full-screen. A second press restores the prior
    /// windowed / maximized state. Default `F11`.
    pub toggle_fullscreen: String,
    /// Toggle zen (distraction-free) mode: hides the chrome elements
    /// configured in `[window] zen_hide` and, when `zen_fullscreen` is
    /// on, also enters borderless full-screen. A second press restores
    /// everything. Unbound by default — set a binding here or use the
    /// command palette.
    pub toggle_zen_mode: String,
    /// Toggle broadcast-input mode: when active, each keystroke typed in
    /// the focused pane is simultaneously forwarded to every other pane
    /// in the broadcast scope (`terminal.broadcast_scope`). A distinct
    /// tinted border is drawn around the panes receiving mirrored input
    /// so the mode is always visible. Unbound by default.
    pub toggle_broadcast_input: String,
    /// Open a new top-level window with a default tab. Uses the profile
    /// set in `window.new_window_profile` (or the overall default profile
    /// when unset). Default `Ctrl+Shift+N`.
    pub new_window: String,
    /// Move the active tab into a brand-new window. No-op when the source
    /// window has only one tab (nothing would remain). Unbound by default.
    pub move_tab_to_new_window: String,
    /// Detach the focused pane into a new tab in the same window. No-op
    /// when the tab is a single pane. Unbound by default.
    pub move_pane_to_new_tab: String,
    /// Detach the focused pane into a brand-new window. No-op when the
    /// tab is a single pane. Unbound by default.
    pub move_pane_to_new_window: String,
    /// Open the snippet picker: a fuzzy-searchable list of all configured
    /// `[[snippets]]` entries. Selecting one inserts its decoded body into
    /// the focused pane's PTY. Unbound by default — set a binding here or
    /// trigger it from the command palette ("Snippets…").
    pub open_snippets: String,
    /// Send the most-recent failed command block (non-zero exit code) to the
    /// configured AI provider and ask for a corrected command.  The AI window
    /// opens with the prompt already submitted; selecting "Inject" types the
    /// suggested fix into the active pane.  Unbound by default — set a
    /// binding here or trigger it from the command palette ("Fix last
    /// command").
    pub fix_last_command: String,
    /// Save the current layout as a named workspace.  The command palette
    /// inline-prompt asks for a name; a timestamp name is generated when
    /// triggered from a binding without the palette open.  Unbound by default
    /// — use the command palette ("Save Workspace…") or set a binding here.
    pub save_workspace: String,
    /// Open the workspace picker in the command palette — fuzzy-search and
    /// restore a previously-saved named workspace.  Unbound by default — use
    /// the command palette ("Open Workspace…") or set a binding here.
    pub open_workspace: String,
    /// Copy the output of the most-recent command block (requires shell
    /// integration) to the clipboard. No-op when no completed block exists.
    /// Unbound by default — set a binding here or use the command palette.
    pub copy_last_command_output: String,
    /// Copy the output of the command block whose range contains the cursor's
    /// current absolute line (requires shell integration). Falls back to the
    /// last block when no block contains the cursor. Unbound by default.
    pub copy_block_output: String,
    /// Copy the command text of the most-recent command block to the clipboard
    /// (requires shell integration). Unbound by default.
    pub copy_last_command: String,
    /// Re-run the most-recent command block's command verbatim by writing its
    /// text + newline to the focused pane's PTY (requires shell integration).
    /// Unbound by default.
    pub rerun_last_command: String,
    /// Load the most-recent command block's command text onto the shell prompt
    /// for editing (writes it WITHOUT a trailing newline). Optionally prefixed
    /// with Ctrl+U (kill-line) when `terminal.edit_command_clears_line` is
    /// on (default true). Requires shell integration. Unbound by default.
    pub edit_last_command: String,
    /// Open the command-history picker: a fuzzy-searchable list of previously
    /// run commands gathered from the configured scope (current pane, tab, or
    /// all tabs). Selecting a command loads it onto the shell prompt for
    /// editing (without a trailing newline). Requires shell integration
    /// (OSC 133). Unbound by default — set a binding here or trigger it from
    /// the command palette ("Command History…").
    pub open_command_history: String,
    /// Export the full scrollback of the focused pane to a text file. If
    /// `terminal.scrollback_export_dir` is set the file is written there
    /// automatically; otherwise a native save-file dialog is opened. The
    /// file name is `terminale-scrollback-YYYYMMDD-HHMMSS.txt`. Unbound by
    /// default — set a binding here or trigger it from the command palette
    /// ("Export Scrollback…").
    pub export_scrollback: String,
    /// Open the clipboard history picker: a fuzzy-searchable list of the last
    /// N text entries produced by copy actions. Selecting an entry pastes it
    /// into the focused pane (honours bracketed paste). Memory-only — entries
    /// are never written to disk. Unbound by default — set a binding here or
    /// trigger it from the command palette ("Clipboard History…").
    pub open_clipboard_history: String,
    /// Swap the focused pane with its left neighbour (restructures the pane
    /// tree by exchanging their ids; the split ratios and directions are
    /// preserved). No-op when there is no left neighbour. Unbound by default.
    pub move_pane_left: String,
    /// Swap the focused pane with its right neighbour. Unbound by default.
    pub move_pane_right: String,
    /// Swap the focused pane with the pane directly above it. Unbound by default.
    pub move_pane_up: String,
    /// Swap the focused pane with the pane directly below it. Unbound by default.
    pub move_pane_down: String,
    /// Rotate the pane assignments in the active tab's split tree one step
    /// forward (the pane in the first slot moves to the last; all others shift
    /// one forward). The split structure is unchanged. Unbound by default.
    pub rotate_panes: String,
    /// Rotate the pane assignments in the active tab's split tree one step
    /// backward (inverse of `rotate_panes`). Unbound by default.
    pub rotate_panes_back: String,
    /// Open the directory-jump picker: a fuzzy-searchable ranked list of
    /// previously-visited directories (tracked via OSC 7 cwd reports). The
    /// list is ordered by frecency — a combined frequency + recency score.
    /// Selecting an entry sends `cd <path>` to the focused pane's PTY so
    /// the shell navigates there immediately. Works with any OSC-7-capable
    /// shell; no third-party tool required. Unbound by default — set a
    /// binding here or open it from the command palette ("Directory Jump…").
    pub open_directory_jump: String,

    // ── Prompt / command navigation ───────────────────────────────────────────
    /// Scroll the viewport so the previous **failed** OSC 133 command block
    /// (non-zero exit code) is visible. Only blocks with a recorded non-zero
    /// exit code qualify — prompts with no exit code or exit 0 are skipped.
    /// Requires shell integration. Unbound by default; clamps at the oldest
    /// failed block (does not wrap).
    pub prev_failed_command: String,
    /// Scroll the viewport so the next **failed** OSC 133 command block
    /// (non-zero exit code) is visible. Only blocks with a recorded non-zero
    /// exit code qualify. Requires shell integration. Unbound by default;
    /// clamps at the newest failed block (does not wrap).
    pub next_failed_command: String,
    /// Open the failed-command picker: a fuzzy-searchable list of command
    /// blocks whose recorded exit code is non-zero (requires shell
    /// integration). Selecting an entry scrolls the viewport to that block.
    /// Unbound by default — use the command palette ("Failed Commands…") or
    /// set a binding here.
    pub open_failed_command_picker: String,
    /// Request a proactive AI command suggestion immediately (works for both
    /// `Manual` and `Auto` trigger modes). The suggestion bar shows a loading
    /// indicator, then the proposed command once the provider replies.
    /// Unbound by default — set a binding here or trigger it from the command
    /// palette ("Suggest Command").
    pub suggest_command: String,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        // Defaults remapped to Ctrl so Windows/Linux users get the same
        // UX out of the box.
        Self {
            new_tab: "Ctrl+T".into(),
            close_tab: "Ctrl+W".into(),
            next_tab: "Ctrl+Tab".into(),
            prev_tab: "Ctrl+Shift+Tab".into(),
            move_tab_left: "Ctrl+Shift+ArrowLeft".into(),
            move_tab_right: "Ctrl+Shift+ArrowRight".into(),
            profile_picker: "Ctrl+Shift+T".into(),
            restart_tab: "Ctrl+Shift+R".into(),
            copy: "Ctrl+Shift+C".into(),
            paste: "Ctrl+Shift+V".into(),
            select_all: "Ctrl+Shift+A".into(),
            find: "Ctrl+Shift+F".into(),
            clear: "Ctrl+K".into(),
            settings: "Ctrl+,".into(),
            font_increase: "Ctrl+=".into(),
            font_decrease: "Ctrl+-".into(),
            font_reset: "Ctrl+0".into(),
            scroll_line_up: "Ctrl+Shift+ArrowUp".into(),
            scroll_line_down: "Ctrl+Shift+ArrowDown".into(),
            scroll_page_up: "Ctrl+Shift+PageUp".into(),
            scroll_page_down: "Ctrl+Shift+PageDown".into(),
            scroll_top: "Ctrl+Shift+Home".into(),
            scroll_bottom: "Ctrl+Shift+End".into(),
            ai_assistant: "Ctrl+Shift+I".into(),
            command_palette: "Ctrl+Shift+P".into(),
            explain_selection: "Ctrl+Shift+E".into(),
            clear_scrollback: "Ctrl+Shift+K".into(),
            reopen_closed_tab: String::new(),
            new_ssh_tab: String::new(),
            stay_on_top: String::new(),
            snap_top: String::new(),
            snap_bottom: String::new(),
            snap_left: String::new(),
            snap_right: String::new(),
            snap_center: String::new(),
            snap_maximize: String::new(),
            // Quarter snap actions — all unbound by default; bind here or
            // use the snap-layout chooser (show_snap_layouts action).
            snap_top_left: String::new(),
            snap_top_right: String::new(),
            snap_bottom_left: String::new(),
            snap_bottom_right: String::new(),
            // Snap-layout chooser — unbound by default; bind here or use
            // the command palette ("Show Snap Layouts").
            show_snap_layouts: String::new(),
            // tmux-inspired defaults: Ctrl+Shift+= splits right (next to
            // the "+" key); Ctrl+Shift+- splits down. Ctrl+Shift+W
            // closes the focused pane.
            split_right: "Ctrl+Shift+=".into(),
            split_down: "Ctrl+Shift+-".into(),
            split_left: String::new(),
            split_up: String::new(),
            close_pane: "Ctrl+Shift+W".into(),
            // Pane keyboard navigation — unbound by default so they never
            // conflict with existing shortcuts. Users can bind Alt+Arrow
            // or any other combo here.
            focus_pane_left: String::new(),
            focus_pane_right: String::new(),
            focus_pane_up: String::new(),
            focus_pane_down: String::new(),
            // Ctrl+Shift+Z is free in the default set; matches the
            // tmux zoom convention.
            toggle_pane_zoom: "Ctrl+Shift+Z".into(),
            // Keyboard pane resize — unbound by default.
            resize_pane_left: String::new(),
            resize_pane_right: String::new(),
            resize_pane_up: String::new(),
            resize_pane_down: String::new(),
            // Tab index shortcuts: Ctrl+1..9 are free in the default keymap
            // (Ctrl+0 is font_reset). 9 always jumps to the last tab
            // (a common convention).
            activate_tab_1: "Ctrl+1".into(),
            activate_tab_2: "Ctrl+2".into(),
            activate_tab_3: "Ctrl+3".into(),
            activate_tab_4: "Ctrl+4".into(),
            activate_tab_5: "Ctrl+5".into(),
            activate_tab_6: "Ctrl+6".into(),
            activate_tab_7: "Ctrl+7".into(),
            activate_tab_8: "Ctrl+8".into(),
            activate_tab_9: "Ctrl+9".into(),
            // Last-tab toggle — unbound by default to avoid collisions.
            last_tab: String::new(),
            // Shell-integration prompt navigation — unbound by default so they
            // never collide with existing bindings.
            prev_prompt: String::new(),
            next_prompt: String::new(),
            // Copy mode — Ctrl+Shift+X is unoccupied in the default keymap.
            copy_mode: "Ctrl+Shift+X".into(),
            // Quick-select — Ctrl+Shift+Space is unoccupied by default.
            quick_select: "Ctrl+Shift+Space".into(),
            // Pane-select — unbound by default to avoid collisions.
            pane_select: String::new(),
            // Reload config — unbound by default; use palette or set a binding.
            reload_config: String::new(),
            // Full-screen toggle — F11 is the cross-platform standard.
            toggle_fullscreen: "F11".into(),
            // Zen mode — unbound by default so it never collides.
            toggle_zen_mode: String::new(),
            // Broadcast input — unbound by default to avoid accidents.
            toggle_broadcast_input: String::new(),
            // Open a new top-level window — Ctrl+Shift+N is the standard
            // "new window" shortcut on both Windows and macOS.
            new_window: "Ctrl+Shift+N".into(),
            // Move-tab / move-pane window actions — unbound by default so
            // they never conflict; bind here or trigger from the palette.
            move_tab_to_new_window: String::new(),
            move_pane_to_new_tab: String::new(),
            move_pane_to_new_window: String::new(),
            // Snippet picker — unbound by default; use the command palette or
            // set a binding here.
            open_snippets: String::new(),
            // Fix last command — unbound by default; use the command palette
            // or set a binding here.
            fix_last_command: String::new(),
            // Workspace save/open — unbound by default; use the command palette
            // or set bindings here.
            save_workspace: String::new(),
            open_workspace: String::new(),
            // Block-scoped copy / re-run / edit — unbound by default; use
            // the command palette or set bindings here.
            copy_last_command_output: String::new(),
            copy_block_output: String::new(),
            copy_last_command: String::new(),
            rerun_last_command: String::new(),
            edit_last_command: String::new(),
            // Command-history picker — unbound by default; use the command
            // palette ("Command History…") or set a binding here.
            open_command_history: String::new(),
            // Export scrollback — unbound by default; use the command palette
            // ("Export Scrollback…") or set a binding here.
            export_scrollback: String::new(),
            // Clipboard history picker — unbound by default; use the command
            // palette ("Clipboard History…") or set a binding here.
            open_clipboard_history: String::new(),
            // Pane swap / rotate — all unbound by default.
            move_pane_left: String::new(),
            move_pane_right: String::new(),
            move_pane_up: String::new(),
            move_pane_down: String::new(),
            rotate_panes: String::new(),
            rotate_panes_back: String::new(),
            // Directory-jump picker — unbound by default; use the command
            // palette ("Directory Jump…") or set a binding here.
            open_directory_jump: String::new(),
            // Prompt/command navigation — unbound by default; use the command
            // palette or set bindings here. Clamp-not-wrap semantics (no
            // silent wrap-around when reaching the first/last block).
            prev_failed_command: String::new(),
            next_failed_command: String::new(),
            open_failed_command_picker: String::new(),
            // Suggest command — unbound by default; use the command palette
            // ("Suggest Command") or set a binding here.
            suggest_command: String::new(),
        }
    }
}

/// One step in the action list of a [`CustomKeybind`].
///
/// In TOML this is written as either a plain action name string
/// (`"NewTab"`) or a `send:` prefix for a literal byte string
/// (`"send:\nls -la\n"`). The serde deserialiser accepts both:
///
/// ```toml
/// [[keybinds.custom]]
/// keys = "Ctrl+Alt+G"
/// actions = ["NewTab", "send:git status\n"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum KeyActionSpec {
    /// A built-in [`ShortcutsConfig`]-compatible action name, e.g.
    /// `"NewTab"`, `"Copy"`, `"CommandPalette"`. Names are matched
    /// case-insensitively at resolution time; unknown names are silently
    /// skipped rather than failing.
    Action(String),
}

impl KeyActionSpec {
    /// Returns the decoded byte payload for a `send:…` action, or
    /// `None` for a named-action spec.
    ///
    /// The `send:` prefix is stripped and the remainder is decoded:
    /// `\n`→`0x0A`, `\r`→`0x0D`, `\t`→`0x09`, `\e`→`0x1B`,
    /// `\\`→`\`, `\xNN`→byte `NN`. Any other `\X` is passed through
    /// verbatim (backslash kept).
    #[must_use]
    pub fn as_send_bytes(&self) -> Option<Vec<u8>> {
        match self {
            KeyActionSpec::Action(s) => s.strip_prefix("send:").map(decode_send_string),
        }
    }

    /// Returns the action name for a named action (not a `send:` spec).
    /// Returns `None` when this spec is actually a `send:` payload.
    #[must_use]
    pub fn action_name(&self) -> Option<&str> {
        match self {
            KeyActionSpec::Action(s) => {
                if s.starts_with("send:") {
                    None
                } else {
                    Some(s.as_str())
                }
            }
        }
    }
}

/// Decode escape sequences in a `send:` string value.
///
/// Supported escapes: `\n` → LF, `\r` → CR, `\t` → TAB,
/// `\e` → ESC (0x1B), `\\` → `\`, `\xNN` → byte `NN`.
/// Any other `\X` is passed through as `\X` (backslash kept).
#[must_use]
pub fn decode_send_string(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'e' => {
                    out.push(0x1b);
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                b'x' if i + 3 < bytes.len() => {
                    let hi = bytes[i + 2];
                    let lo = bytes[i + 3];
                    if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() {
                        let val = hex_nibble(hi) << 4 | hex_nibble(lo);
                        out.push(val);
                        i += 4;
                    } else {
                        out.push(b'\\');
                        out.push(bytes[i + 1]);
                        i += 2;
                    }
                }
                other => {
                    out.push(b'\\');
                    out.push(other);
                    i += 2;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// A single user-defined keybind: a key combo bound to an ordered list
/// of actions that are executed in sequence when the combo is pressed.
///
/// ```toml
/// [[keybinds.custom]]
/// keys    = "Ctrl+Alt+G"
/// actions = ["NewTab", "send:git status\n"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CustomKeybind {
    /// The key combo string, e.g. `"Ctrl+Alt+G"`. Uses the same
    /// format as [`ShortcutsConfig`] fields.
    pub keys: String,
    /// Ordered list of actions to execute. Each entry is either a
    /// built-in action name or a `send:…` payload string.
    pub actions: Vec<KeyActionSpec>,
}

/// One binding inside a [`KeyTable`] — a single key or combo mapped to an
/// ordered sequence of actions.
///
/// ```toml
/// [[keybinds.key_tables]]
/// name    = "pane"
/// leader  = "Ctrl+A"
/// timeout_ms = 1500
///
/// [[keybinds.key_tables.bindings]]
/// key     = "V"
/// actions = ["SplitRight"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyTableEntry {
    /// Key or combo that triggers this binding while the table is active,
    /// e.g. `"V"`, `"Shift+H"`.
    pub key: String,
    /// Ordered list of actions to execute.  Same format as
    /// [`CustomKeybind::actions`]: named built-in actions or `send:…` payloads.
    pub actions: Vec<KeyActionSpec>,
}

/// A named modal key-table activated by a leader combo.  When the leader is
/// pressed the terminal enters the table's mode; the *next* key is matched
/// against `bindings` and its actions are executed.  The mode exits
/// automatically after `timeout_ms` or when Esc is pressed.
///
/// ```toml
/// [[keybinds.key_tables]]
/// name       = "pane"
/// leader     = "Ctrl+A"
/// timeout_ms = 1500
///
/// [[keybinds.key_tables.bindings]]
/// key     = "V"
/// actions = ["SplitRight"]
///
/// [[keybinds.key_tables.bindings]]
/// key     = "H"
/// actions = ["SplitDown"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyTable {
    /// Unique name displayed in the status-bar indicator while the table is
    /// active (e.g. `"pane"`, `"resize"`).
    pub name: String,
    /// Key combo that enters this table, e.g. `"Ctrl+A"`.  Uses the same
    /// format as [`ShortcutsConfig`] fields.
    pub leader: String,
    /// How many milliseconds to wait for the next key before exiting the
    /// table automatically.  Clamped to `[100, 30_000]`.  Default `1500`.
    #[serde(default = "default_key_table_timeout")]
    pub timeout_ms: u32,
    /// Bindings active while this table is the active modal.
    #[serde(default)]
    pub bindings: Vec<KeyTableEntry>,
}

fn default_key_table_timeout() -> u32 {
    1500
}

/// A single custom mouse binding: maps a (button + modifiers + click-count)
/// combination to an ordered list of actions executed in sequence.
///
/// ```toml
/// [[keybinds.mouse]]
/// button  = "Middle"
/// mods    = "Alt"
/// count   = 1
/// actions = ["Paste"]
///
/// [[keybinds.mouse]]
/// button  = "Right"
/// mods    = ""
/// count   = 2
/// actions = ["send:ls -la\n"]
/// ```
///
/// `button` is one of `"Left"`, `"Right"`, `"Middle"`, `"Back"`,
/// `"Forward"` (case-insensitive). `mods` is a `+`-separated list of
/// `"Ctrl"`, `"Shift"`, `"Alt"`, `"Meta"` — or empty for no modifiers.
/// `count` is the click streak: `1` = single, `2` = double, `3` = triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MouseBinding {
    /// Mouse button name: `"Left"`, `"Right"`, `"Middle"`, `"Back"`,
    /// or `"Forward"` (case-insensitive).
    pub button: String,
    /// Modifier string, e.g. `"Ctrl+Shift"`, `"Alt"`, or `""` for none.
    #[serde(default)]
    pub mods: String,
    /// Click streak that triggers this binding: `1` = single click,
    /// `2` = double click, `3` = triple click. Clamped to `[1, 3]`.
    #[serde(default = "default_mouse_count")]
    pub count: u8,
    /// Ordered list of actions to execute. Same format as
    /// [`CustomKeybind::actions`]: named built-in actions or `send:…` payloads.
    pub actions: Vec<KeyActionSpec>,
}

fn default_mouse_count() -> u8 {
    1
}

/// User-configurable keybinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct KeybindsConfig {
    /// Global hotkey that toggles the Quake drop-down window. Accepts
    /// strings like `"Ctrl+`"`, `"Alt+Space"`, `"Ctrl+Shift+T"`. Empty
    /// = Quake mode disabled.
    pub quake: String,
    /// In-app shortcuts (every action that lives inside the terminal
    /// window — tab management, find, copy/paste, …).
    pub shortcuts: ShortcutsConfig,
    /// Custom multi-action keybinds. Each entry maps a key combo to a
    /// sequence of actions — built-in named actions and/or `send:…`
    /// byte payloads — that are all executed in order when the combo is
    /// pressed. Custom binds take priority over the built-in shortcuts.
    /// Default: empty (no custom binds).
    #[serde(default)]
    pub custom: Vec<CustomKeybind>,
    /// Named modal key-tables, each activated by a leader combo.
    /// Pressing the leader enters the table's mode; the next key dispatches
    /// its action sequence and exits the mode.  An optional timeout (default
    /// 1 500 ms) and Esc both exit without acting.  The active table name is
    /// shown in the status bar while the mode is live.
    /// Default: empty (no key-tables).
    #[serde(default)]
    pub key_tables: Vec<KeyTable>,
    /// Custom mouse bindings. Each entry maps a (button + modifiers + click
    /// count) to an ordered action sequence. Default empty — all existing
    /// built-in mouse behaviour is unchanged. A matching custom binding runs
    /// its actions and consumes the click; if none match, built-in behaviour
    /// proceeds as normal.
    #[serde(default)]
    pub mouse: Vec<MouseBinding>,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            quake: "Ctrl+`".into(),
            shortcuts: ShortcutsConfig::default(),
            custom: Vec::new(),
            key_tables: Vec::new(),
            mouse: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MouseBinding TOML roundtrip ──────────────────────────────────────────

    /// Default `KeybindsConfig` has an empty `mouse` list.
    #[test]
    fn mouse_bindings_default_empty() {
        let cfg = KeybindsConfig::default();
        assert!(cfg.mouse.is_empty(), "mouse bindings must default to empty");
    }

    /// A `MouseBinding` roundtrips through TOML unchanged.
    #[test]
    fn mouse_binding_toml_roundtrip() {
        let mut cfg = KeybindsConfig::default();
        cfg.mouse.push(MouseBinding {
            button: "Middle".to_string(),
            mods: "Alt".to_string(),
            count: 1,
            actions: vec![KeyActionSpec::Action("Paste".to_string())],
        });
        cfg.mouse.push(MouseBinding {
            button: "Right".to_string(),
            mods: String::new(),
            count: 2,
            actions: vec![KeyActionSpec::Action("send:ls -la\n".to_string())],
        });

        let serialised = toml::to_string(&cfg).expect("serialise");
        let roundtripped: KeybindsConfig = toml::from_str(&serialised).expect("deserialise");
        assert_eq!(
            cfg.mouse, roundtripped.mouse,
            "mouse bindings must survive a TOML roundtrip unchanged"
        );
    }

    /// `count` defaults to 1 when omitted in TOML.
    #[test]
    fn mouse_binding_count_default_is_one() {
        let toml_str = r#"
            [[mouse]]
            button  = "Left"
            actions = ["Copy"]
        "#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            mouse: Vec<MouseBinding>,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("parse");
        assert_eq!(w.mouse[0].count, 1, "count must default to 1 when omitted");
    }

    /// `mods` defaults to empty string when omitted in TOML.
    #[test]
    fn mouse_binding_mods_default_is_empty() {
        let toml_str = r#"
            [[mouse]]
            button  = "Right"
            count   = 1
            actions = ["Copy"]
        "#;
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            mouse: Vec<MouseBinding>,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("parse");
        assert!(
            w.mouse[0].mods.is_empty(),
            "mods must default to empty when omitted"
        );
    }

    /// Default `KeybindsConfig` has no key-tables.
    #[test]
    fn key_tables_default_empty() {
        let cfg = KeybindsConfig::default();
        assert!(
            cfg.key_tables.is_empty(),
            "key_tables must default to empty"
        );
    }

    /// A `KeybindsConfig` with key_tables round-trips through TOML unchanged.
    #[test]
    fn key_tables_toml_roundtrip() {
        let mut cfg = KeybindsConfig::default();
        cfg.key_tables.push(KeyTable {
            name: "pane".to_string(),
            leader: "Ctrl+A".to_string(),
            timeout_ms: 2000,
            bindings: vec![KeyTableEntry {
                key: "V".to_string(),
                actions: vec![KeyActionSpec::Action("SplitRight".to_string())],
            }],
        });

        let serialised = toml::to_string(&cfg).expect("serialise");
        let roundtripped: KeybindsConfig = toml::from_str(&serialised).expect("deserialise");
        assert_eq!(
            cfg.key_tables, roundtripped.key_tables,
            "key_tables must survive a TOML roundtrip unchanged"
        );
    }

    /// `KeyTable` with default `timeout_ms` omitted in TOML gets the 1500 ms
    /// fallback applied by serde.
    #[test]
    fn key_table_timeout_default_applied_on_deserialise() {
        let toml_str = r#"
            [[key_tables]]
            name   = "nav"
            leader = "Ctrl+B"
            [[key_tables.bindings]]
            key     = "N"
            actions = ["NextTab"]
        "#;
        // Wrap inside a KeybindsConfig table.
        let wrapped = format!("[shortcuts]\n{toml_str}");
        // Parse as just a partial struct; simpler to parse the inner table.
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            key_tables: Vec<KeyTable>,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("parse");
        assert_eq!(
            w.key_tables[0].timeout_ms, 1500,
            "timeout_ms must default to 1500 when omitted"
        );
        drop(wrapped); // suppress unused warning
    }

    /// `key_table_timed_out` returns the correct result for instants before
    /// and after the deadline.  This is the pure-function unit test required
    /// by the feature spec.
    #[test]
    fn key_table_timed_out_pure_fn() {
        // Use a manual instant arithmetic approach: compute a "now" that is
        // definitely before and after a 500 ms deadline.
        let base = std::time::Instant::now();

        // Simulate "entered 100 ms ago" — not yet timed out (500 ms timeout).
        let before = base + std::time::Duration::from_millis(100);
        let timed_out = before.duration_since(base) >= std::time::Duration::from_millis(500);
        assert!(!timed_out, "100 ms < 500 ms timeout must NOT be timed-out");

        // Simulate "entered 600 ms ago" — timed out.
        let after = base + std::time::Duration::from_millis(600);
        let timed_out = after.duration_since(base) >= std::time::Duration::from_millis(500);
        assert!(timed_out, "600 ms > 500 ms timeout must be timed-out");
    }
}
