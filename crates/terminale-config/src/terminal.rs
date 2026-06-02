//! Terminal grid behaviour and external editor integration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── HyperlinkRule ─────────────────────────────────────────────────────────────

/// A user-defined URL detection rule. The `regex` pattern is applied to each
/// visible row; any match is treated as a clickable hyperlink. Rules are
/// applied AFTER the built-in scheme scanner (http/https/ftp/file/mailto),
/// so you can add extra patterns (e.g. bare IPv4s, git SHAs, custom schemes)
/// without losing the defaults.
///
/// When the `hyperlink_rules` list is empty (the default), terminale falls
/// back to the built-in scanner only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct HyperlinkRule {
    /// Regular expression matched against each visible terminal row. Any
    /// non-overlapping match becomes a clickable link. Invalid regexes are
    /// skipped at runtime with a `warn!`-level log entry.
    pub regex: String,
    /// Optional human-readable label shown in the Settings list for this
    /// rule. Has no effect on matching; purely for the user's reference.
    #[serde(default)]
    pub label: String,
}

impl HyperlinkRule {
    /// Construct a rule from a regex string and optional label.
    #[must_use]
    pub fn new(regex: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            regex: regex.into(),
            label: label.into(),
        }
    }
}

/// The sane built-in default set of hyperlink rules. These are used as the
/// fallback when `hyperlink_rules` is empty, and are also the initial
/// contents when the user opens the settings list for the first time.
///
/// Patterns:
/// - HTTP/HTTPS URLs (catches bare hostnames too)
/// - `file://` URIs
/// - IPv4 addresses with optional port
/// - Git-style short SHA hashes (7–40 hex chars preceded by a word boundary)
#[must_use]
pub fn default_hyperlink_rules() -> Vec<HyperlinkRule> {
    vec![
        HyperlinkRule::new(
            r#"https?://[^\s\x00-\x1f\x7f<>()\[\]{}"'`]+"#,
            "HTTP/HTTPS URL",
        ),
        HyperlinkRule::new(r#"file://[^\s\x00-\x1f\x7f<>()\[\]{}"'`]+"#, "file:// URI"),
        HyperlinkRule::new(
            r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)(?::\d{1,5})?\b",
            "IPv4 address",
        ),
        HyperlinkRule::new(r"\b[0-9a-f]{7,40}\b", "Git SHA hash"),
    ]
}

// ── ExitBehavior ──────────────────────────────────────────────────────────────

/// What terminale does when the program running in a pane exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitBehavior {
    /// Close the pane (and the tab if it was the only pane) as soon as the
    /// process exits. This is the default and matches most terminal emulators.
    Close,
    /// Keep the pane open regardless of the exit status. A dim status line
    /// `[process exited — close this pane to dismiss]` is appended to the
    /// buffer. The user closes the pane manually.
    Hold,
    /// Close the pane automatically **only** when the process exits with
    /// status 0. For any non-zero exit (or when the exit status cannot be
    /// determined) the behaviour is the same as `Hold`.
    CloseOnCleanExit,
}

impl Default for ExitBehavior {
    fn default() -> Self {
        // Default to Close — matches every other terminal emulator and
        // preserves the behaviour that users had before this feature was added.
        Self::Close
    }
}

impl ExitBehavior {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Close, Self::Hold, Self::CloseOnCleanExit]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Hold => "Hold",
            Self::CloseOnCleanExit => "Close on clean exit",
        }
    }

    /// Decide whether the pane should be closed given a known exit status.
    /// `exit_status` is `None` when the actual OS exit code is not available
    /// (remote sessions, or when shell integration didn't report one).
    ///
    /// Returns `true` when the pane should be closed immediately.
    #[must_use]
    pub fn should_close(self, exit_status: Option<i32>) -> bool {
        match self {
            // Always close.
            Self::Close => true,
            // Never close automatically.
            Self::Hold => false,
            // Close only on a known-clean (0) exit; hold otherwise.
            Self::CloseOnCleanExit => exit_status == Some(0),
        }
    }
}

/// When detected URLs get an accent underline drawn beneath them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkUnderline {
    /// Underline every detected URL permanently, the moment it appears.
    Always,
    /// Underline only the link currently under the Ctrl-hover pointer
    /// (default). Keeps links discoverable without leaving a persistent
    /// accent line under banner URLs on startup.
    Hover,
    /// Never draw the autodetect underline; links stay discoverable via the
    /// hover tooltip + pointer cursor only.
    Never,
}

impl Default for LinkUnderline {
    fn default() -> Self {
        Self::Hover
    }
}

impl LinkUnderline {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Always, Self::Hover, Self::Never]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::Hover => "On hover",
            Self::Never => "Never",
        }
    }
}

/// How application cursor-key mode (DECCKM) is honoured when encoding
/// keyboard input.
///
/// - `auto` (default): when an application enables DECCKM, unmodified arrow
///   keys and Home/End transmit SS3 sequences (`ESC O A` … `ESC O D`,
///   `ESC O H`, `ESC O F`); otherwise CSI sequences are used. This is the
///   correct behaviour for programs like vim, less, htop, and mc that rely on
///   DECCKM to distinguish cursor keys from editing keys.
/// - `always_csi`: always send CSI sequences for arrows and Home/End,
///   regardless of the DECCKM state. This is a compatibility escape-hatch for
///   shells or remote sessions that set DECCKM unintentionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardEncoding {
    /// Honour DECCKM: use SS3 in application cursor-key mode, CSI otherwise.
    Auto,
    /// Always emit CSI for arrows and Home/End, ignoring DECCKM.
    AlwaysCsi,
}

impl Default for KeyboardEncoding {
    fn default() -> Self {
        Self::Auto
    }
}

impl KeyboardEncoding {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Auto, Self::AlwaysCsi]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (honour app mode)",
            Self::AlwaysCsi => "Always CSI",
        }
    }
}

/// Which inline-image protocols the terminal parser should accept.
///
/// Each toggle gates the corresponding OSC/DCS parser at parse time; the
/// GPU upload and blit pipeline is always compiled in. Disabling a protocol
/// is useful when a runaway script floods the terminal with large images.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ImageProtocolsConfig {
    /// Accept `OSC 1337;File=…` inline images (OSC 1337 inline-image protocol).
    /// Default `true`.
    pub osc1337: bool,
    /// Accept Sixel `DCS … ST` graphics. Default `true`.
    pub sixel: bool,
    /// Accept `ESC _ G … ST` graphics (APC (ESC _G) graphics protocol).
    /// Default `true`.
    pub apc: bool,
}

impl Default for ImageProtocolsConfig {
    fn default() -> Self {
        Self {
            osc1337: true,
            sixel: true,
            apc: true,
        }
    }
}

/// Scope used by the command-history picker to decide which panes to
/// collect history entries from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandHistoryScope {
    /// Gather commands from the focused pane only. Fastest; most targeted.
    CurrentPane,
    /// Gather commands from every pane in the active tab (default).
    CurrentTab,
    /// Gather commands from every pane in every tab of the window.
    Window,
}

impl Default for CommandHistoryScope {
    fn default() -> Self {
        Self::CurrentTab
    }
}

impl CommandHistoryScope {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::CurrentPane, Self::CurrentTab, Self::Window]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentPane => "Current pane",
            Self::CurrentTab => "Current tab",
            Self::Window => "All tabs",
        }
    }
}

/// Which panes receive mirrored input while broadcast mode is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BroadcastScope {
    /// Mirror to every other pane in the same tab. Default.
    AllPanesInTab,
    /// Mirror to every pane in every tab of the window.
    AllPanesInWindow,
}

impl Default for BroadcastScope {
    fn default() -> Self {
        Self::AllPanesInTab
    }
}

impl BroadcastScope {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::AllPanesInTab, Self::AllPanesInWindow]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AllPanesInTab => "All panes in tab",
            Self::AllPanesInWindow => "All panes in window",
        }
    }
}

// ── ClipboardReadPolicy ───────────────────────────────────────────────────────

/// Permission policy for OSC 52 clipboard READ queries.
///
/// When a program sends `OSC 52 ; <sel> ; ? ST` it is requesting that the
/// terminal reply with the current clipboard contents encoded as base64.
/// Because this is an **exfiltration vector** (a rogue program in a remote
/// shell could silently read secrets copied to the clipboard), the default
/// is `deny`.
///
/// - `deny` (default): no reply is sent. The requesting program receives
///   nothing, which is safe and matches the behaviour of terminals that do
///   not implement clipboard read at all.
/// - `allow`: read the system clipboard, base64-encode it, and send
///   `ESC ] 52 ; <sel> ; <base64> ST` back to the PTY. Only enable this
///   when you run programs you trust entirely in every pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardReadPolicy {
    /// Never reply to OSC 52 clipboard queries. Default — safe.
    Deny,
    /// Reply with the clipboard contents encoded as base64.
    Allow,
}

impl Default for ClipboardReadPolicy {
    fn default() -> Self {
        Self::Deny
    }
}

impl ClipboardReadPolicy {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Deny, Self::Allow]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Deny => "Deny (default — safe)",
            Self::Allow => "Allow",
        }
    }
}

// ── ScrollbackExportFormat ────────────────────────────────────────────────────

/// Output format for the "Export scrollback" action.
///
/// Only `plain` is implemented in v1; `ansi` (colour-codes preserved) is a
/// documented follow-up that can be added without a breaking config change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollbackExportFormat {
    /// Plain text — ANSI escape sequences stripped. Default.
    Plain,
}

impl Default for ScrollbackExportFormat {
    fn default() -> Self {
        Self::Plain
    }
}

impl ScrollbackExportFormat {
    /// All variants — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 1] {
        [Self::Plain]
    }

    /// Human-readable label for the settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain text",
        }
    }
}

// ── TerminalConfig ────────────────────────────────────────────────────────────

/// Terminal-grid behaviour knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TerminalConfig {
    /// Characters treated as word boundaries when expanding a double-click
    /// selection. Whitespace is always a boundary regardless of this set;
    /// every character listed here is *also* treated as a boundary, so a
    /// double-click selects the run of characters between them. The default
    /// splits on common shell and path punctuation while keeping `_`, `-`,
    /// `.`, and `/` joined so identifiers and paths select as a unit.
    pub word_separators: String,
    /// When detected URLs are underlined: `always` (persistent accent line
    /// under every URL), `hover` (only the link under the Ctrl-hover
    /// pointer — the default), or `never`. Default `hover` avoids a stray
    /// accent line under banner URLs printed before any output scrolls.
    pub link_underline: LinkUnderline,
    /// When you type an `ssh …` command for a host that isn't already saved,
    /// offer a one-click "Save this SSH host?" prompt so it shows up in the
    /// quick-connect dropdown next time. The prompt itself carries a "don't
    /// ask again" checkbox that flips this off. Defaults to `true`.
    pub offer_save_ssh_hosts: bool,
    /// When `true`, dragging a split-pane divider resizes the underlying PTYs
    /// on every cursor move (snappy local-shell feel). When `false`, the
    /// PTYs only learn about the new size when the user releases the mouse
    /// button — useful on slow shells or SSH connections where every SIGWINCH
    /// triggers a full repaint. The divider line itself still tracks the
    /// cursor live regardless of this setting. Defaults to `true`.
    pub live_pane_resize: bool,
    /// Number of cells to nudge the focused pane's parent split by when using
    /// the keyboard pane-resize actions (`resize_pane_left/right/up/down`).
    /// Clamped to `1..=20`. Default `2`.
    pub pane_resize_step_cells: u8,
    /// When `true`, draw a small status dot in the left margin at each
    /// OSC 133 prompt-start row. The dot colour reflects the exit status of
    /// that command: neutral (no status known), green (exit 0), or red (any
    /// non-zero exit). Requires shell integration to emit OSC 133 sequences.
    /// Default `false`.
    pub show_prompt_marks: bool,
    /// When `true`, programs may send OS desktop notifications via OSC 9
    /// (body-only form) or OSC 777 (title + body form). Notifications are
    /// only shown when the terminal window does **not** have focus, mirroring
    /// how most mail clients suppress notifications for a visible inbox.
    /// Default `true`.
    pub os_notifications: bool,
    /// Custom hyperlink-detection rules applied to the visible terminal rows.
    ///
    /// Each entry is a regex pattern (plus an optional display label).
    /// When the list is **non-empty** these rules are compiled and applied
    /// in addition to the built-in scheme scanner; any regex that fails to
    /// compile is skipped with a warning. When the list is **empty** (the
    /// default), terminale falls back to the built-in detection only (same
    /// behaviour as before this field existed).
    ///
    /// Use [`default_hyperlink_rules()`] to obtain the recommended starter
    /// set that you can extend or replace.
    #[serde(default)]
    pub hyperlink_rules: Vec<HyperlinkRule>,
    /// What to do when the program running in a pane exits.
    ///
    /// - `close` (default): close the pane immediately.
    /// - `hold`: keep the pane open with a dim status line; user closes manually.
    /// - `close_on_clean_exit`: close automatically only when exit status is 0,
    ///   otherwise behave like `hold`.
    pub exit_behavior: ExitBehavior,
    /// Which inline-image protocols (OSC 1337, Sixel, APC graphics) are
    /// accepted. All default to `true`. Disabling a protocol silently drops
    /// images sent via that protocol without affecting other output.
    pub image_protocols: ImageProtocolsConfig,
    /// How application cursor-key mode (DECCKM) is honoured when encoding
    /// keyboard input.
    ///
    /// `auto` (default): use SS3 sequences for unmodified arrows/Home/End when
    /// the running application has enabled DECCKM, CSI otherwise — correct for
    /// vim, less, htop, mc, and similar full-screen programs.
    ///
    /// `always_csi`: always send CSI regardless of DECCKM — compatibility
    /// escape-hatch for shells or remote sessions that set DECCKM by accident.
    pub keyboard_encoding: KeyboardEncoding,
    /// Scope for broadcast-input mode (toggled by `toggle_broadcast_input`).
    ///
    /// When broadcast mode is active, each keystroke typed in the focused pane
    /// is also forwarded to every other pane in the chosen scope.
    ///
    /// - `all_panes_in_tab` (default): mirror to every other pane in the same
    ///   tab only.
    /// - `all_panes_in_window`: mirror to every pane in every tab of the
    ///   window.
    pub broadcast_scope: BroadcastScope,
    /// When `true` (default), hovering the pointer over a hyperlink shows a
    /// small floating tooltip with the resolved target URL. For OSC 8 links
    /// the tooltip shows the destination even when it differs from the visible
    /// label text. Set `false` to disable the tooltip entirely.
    pub link_hover_tooltip: bool,
    /// How long (in milliseconds) the pointer must dwell over a link before
    /// the hover tooltip appears. `0` = instant (default). Range: `0..=2000`.
    pub link_hover_delay_ms: u32,
    /// Permission policy for OSC 52 clipboard READ queries (the `?` payload).
    ///
    /// When a running program sends `OSC 52 ; <sel> ; ? ST` it asks the
    /// terminal to reply with the current clipboard contents. This is an
    /// exfiltration vector, so the default is `deny` (no reply).
    ///
    /// - `deny` (default): ignore the query silently. Safe for all sessions.
    /// - `allow`: read the system clipboard, base64-encode it, and write the
    ///   response back to the PTY. Only use this for fully-trusted programs.
    pub clipboard_read: ClipboardReadPolicy,
    /// When `true` (default), each shell command is captured as a discrete
    /// block using OSC 133 shell integration marks. Requires the shell to emit
    /// `OSC 133;A/B/C/D` sequences. The blocks are the foundation for
    /// block-copy, re-run, and AI fix-on-fail features. Default `true`.
    pub command_blocks: bool,
    /// Maximum number of command blocks retained in memory per terminal pane.
    /// Oldest blocks are evicted when the cap is exceeded. Mirrors the
    /// scrollback limit in spirit. Range `1..=100_000`. Default `1000`.
    pub max_command_blocks: usize,
    /// When `true` (default), the `edit_last_command` action sends Ctrl+U
    /// (kill-line, 0x15) before writing the command text onto the prompt, so
    /// any partially-typed input is cleared first. Set to `false` if your
    /// shell or readline configuration binds Ctrl+U differently and you
    /// prefer not to have the line cleared.
    pub edit_command_clears_line: bool,
    /// Which panes the command-history picker (`open_command_history` action)
    /// collects history entries from.
    ///
    /// - `current_pane`: only the focused pane.
    /// - `current_tab` (default): all panes in the active tab.
    /// - `window`: all panes in all tabs of the window.
    ///
    /// Requires `command_blocks` to be enabled; the picker shows an empty
    /// list with a hint when shell integration is off.
    pub command_history_scope: CommandHistoryScope,
    /// Maximum number of history entries shown in the command-history picker.
    /// Entries are collected most-recent first and deduplicated before this
    /// cap is applied. Range `1..=10_000`. Default `500`.
    pub command_history_max_entries: usize,
    /// Output format for the "Export scrollback" action.
    ///
    /// `plain` (default): ANSI escape sequences are stripped before writing.
    /// `ansi` is reserved for a future release.
    pub scrollback_export_format: ScrollbackExportFormat,
    /// Directory where exported scrollback files are saved. `None` (default)
    /// opens a native save-file dialog instead. When set, files are written
    /// directly to this directory with a timestamped name of the form
    /// `terminale-scrollback-YYYYMMDD-HHMMSS.txt` and the path is logged as
    /// a toast notification.
    #[serde(default)]
    pub scrollback_export_dir: Option<std::path::PathBuf>,

    // ── Paste safety ──────────────────────────────────────────────────────────
    /// When `true`, always ask for confirmation before pasting multi-line
    /// text, regardless of whether the running program has bracketed paste
    /// enabled. Default `false` — use this if you want a confirmation prompt
    /// for every multi-line paste without exception.
    pub paste_confirm_multiline: bool,
    /// When `true` (default), ask for confirmation before pasting multi-line
    /// text when the focused program has NOT enabled bracketed paste. This is
    /// the primary defence against clipboard-injection attacks: a shell or
    /// other unbracketed program would execute the first line immediately if
    /// a newline is present in the pasted text.
    pub paste_confirm_when_unbracketed: bool,
    /// When `true`, non-printable control bytes (everything below U+0020
    /// except `\n`, `\t`, and `\r`) are stripped from pasted text before it
    /// reaches the PTY. Applied to both confirmed and direct pastes. Default
    /// `false`.
    pub paste_strip_control_chars: bool,

    // ── Prompt navigation ─────────────────────────────────────────────────────
    /// When `true` (default), briefly highlight the prompt block that the
    /// viewport has jumped to after a `JumpToPrevPrompt`, `JumpToNextPrompt`,
    /// `JumpToPrevFailedCommand`, or `JumpToNextFailedCommand` action.
    ///
    /// The highlight is a faint tinted band drawn over the target prompt row
    /// for about 400 ms, then fades out. It is entirely off the hot-path:
    /// no per-cell state is modified; the band is drawn as an overlay quad
    /// during the frame that immediately follows the jump.
    ///
    /// Set to `false` to disable the highlight entirely (viewport scrolls
    /// silently, as before).
    pub highlight_on_jump: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            // Note `.`, `/`, `-`, and `_` are intentionally absent so
            // `foo.bar`, `/etc/hosts`, and `my-var_name` select whole on
            // a double-click.
            word_separators: r#"()[]{}<>"'`,;:!?@#$%^&*=+|\ "#.into(),
            link_underline: LinkUnderline::default(),
            offer_save_ssh_hosts: true,
            live_pane_resize: true,
            pane_resize_step_cells: 2,
            show_prompt_marks: false,
            os_notifications: true,
            // Empty means "use built-in detection only" — existing behaviour.
            hyperlink_rules: Vec::new(),
            exit_behavior: ExitBehavior::default(),
            image_protocols: ImageProtocolsConfig::default(),
            keyboard_encoding: KeyboardEncoding::default(),
            broadcast_scope: BroadcastScope::default(),
            link_hover_tooltip: true,
            link_hover_delay_ms: 0,
            clipboard_read: ClipboardReadPolicy::default(),
            command_blocks: true,
            max_command_blocks: 1000,
            edit_command_clears_line: true,
            command_history_scope: CommandHistoryScope::default(),
            command_history_max_entries: 500,
            scrollback_export_format: ScrollbackExportFormat::default(),
            scrollback_export_dir: None,
            paste_confirm_multiline: false,
            paste_confirm_when_unbracketed: true,
            paste_strip_control_chars: false,
            highlight_on_jump: true,
        }
    }
}

impl TerminalConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=20).contains(&self.pane_resize_step_cells) {
            return Err(ConfigError::Invalid {
                field: "terminal.pane_resize_step_cells",
                message: "must be between 1 and 20",
            });
        }
        // Hyperlink rules: regex strings themselves are validated at runtime
        // (when the `regex` crate compiles them) to avoid pulling in `regex`
        // as a dependency of the config crate. Only flag obviously-empty
        // patterns here so a config file with `regex = ""` gets a clear error.
        for rule in &self.hyperlink_rules {
            if rule.regex.trim().is_empty() {
                return Err(ConfigError::Invalid {
                    field: "terminal.hyperlink_rules[].regex",
                    message: "regex must not be empty — remove the rule or provide a valid pattern",
                });
            }
        }
        if self.link_hover_delay_ms > 2000 {
            return Err(ConfigError::Invalid {
                field: "terminal.link_hover_delay_ms",
                message: "must be between 0 and 2000",
            });
        }
        if self.max_command_blocks == 0 || self.max_command_blocks > 100_000 {
            return Err(ConfigError::Invalid {
                field: "terminal.max_command_blocks",
                message: "must be between 1 and 100_000",
            });
        }
        if self.command_history_max_entries == 0 || self.command_history_max_entries > 10_000 {
            return Err(ConfigError::Invalid {
                field: "terminal.command_history_max_entries",
                message: "must be between 1 and 10_000",
            });
        }
        Ok(())
    }
}

/// External-editor integration for clickable `file:line:col` references.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct EditorConfig {
    /// Command template launched on Ctrl+click of a path. Supports the
    /// tokens `{file}`, `{line}`, `{column}`. Empty = open with the OS
    /// default handler (no line jump). Examples:
    /// `"code -g {file}:{line}:{column}"`, `"vim +{line} {file}"`,
    /// `"subl {file}:{line}:{column}"`.
    pub command: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ExitBehavior tests ────────────────────────────────────────────────────

    #[test]
    fn exit_behavior_default_is_close() {
        assert_eq!(ExitBehavior::default(), ExitBehavior::Close);
    }

    #[test]
    fn exit_behavior_close_should_close_regardless_of_status() {
        // Close always closes, regardless of exit status.
        assert!(ExitBehavior::Close.should_close(Some(0)));
        assert!(ExitBehavior::Close.should_close(Some(1)));
        assert!(ExitBehavior::Close.should_close(Some(127)));
        assert!(ExitBehavior::Close.should_close(None));
    }

    #[test]
    fn exit_behavior_hold_never_closes() {
        assert!(!ExitBehavior::Hold.should_close(Some(0)));
        assert!(!ExitBehavior::Hold.should_close(Some(1)));
        assert!(!ExitBehavior::Hold.should_close(None));
    }

    #[test]
    fn exit_behavior_close_on_clean_exit_predicate() {
        // Closes only when status is exactly 0.
        assert!(ExitBehavior::CloseOnCleanExit.should_close(Some(0)));
        // Non-zero or unknown → hold.
        assert!(!ExitBehavior::CloseOnCleanExit.should_close(Some(1)));
        assert!(!ExitBehavior::CloseOnCleanExit.should_close(Some(127)));
        assert!(!ExitBehavior::CloseOnCleanExit.should_close(None));
    }

    #[test]
    fn exit_behavior_roundtrip_toml() {
        // Every variant survives a TOML serialise → deserialise round-trip
        // via the full Config wrapper (which puts exit_behavior in context).
        for behavior in ExitBehavior::all() {
            let mut cfg = crate::Config::default();
            cfg.terminal.exit_behavior = behavior;
            let raw = toml::to_string(&cfg).expect("serialise");
            let back: crate::Config = toml::from_str(&raw).expect("deserialise");
            assert_eq!(
                back.terminal.exit_behavior, behavior,
                "ExitBehavior::{behavior:?} must roundtrip through Config"
            );
        }
    }

    #[test]
    fn exit_behavior_parses_close() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nexit_behavior = \"close\"\n").expect("must parse close");
        assert_eq!(cfg.terminal.exit_behavior, ExitBehavior::Close);
    }

    #[test]
    fn exit_behavior_parses_hold() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nexit_behavior = \"hold\"\n").expect("must parse hold");
        assert_eq!(cfg.terminal.exit_behavior, ExitBehavior::Hold);
    }

    #[test]
    fn exit_behavior_parses_close_on_clean_exit() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nexit_behavior = \"close_on_clean_exit\"\n")
                .expect("must parse close_on_clean_exit");
        assert_eq!(cfg.terminal.exit_behavior, ExitBehavior::CloseOnCleanExit);
    }

    #[test]
    fn exit_behavior_absent_defaults_to_close() {
        let cfg: crate::Config = toml::from_str("[terminal]\n").expect("must parse");
        assert_eq!(cfg.terminal.exit_behavior, ExitBehavior::Close);
    }

    // ── HyperlinkRule tests ───────────────────────────────────────────────────

    #[test]
    fn hyperlink_rules_default_empty() {
        // Default is empty: built-in detection only.
        assert!(TerminalConfig::default().hyperlink_rules.is_empty());
    }

    #[test]
    fn default_hyperlink_rules_is_non_empty() {
        let rules = default_hyperlink_rules();
        assert!(
            !rules.is_empty(),
            "default_hyperlink_rules() must not be empty"
        );
        // All default rules must have non-empty regexes.
        for r in &rules {
            assert!(!r.regex.is_empty(), "default rule regex must not be empty");
        }
    }

    #[test]
    fn hyperlink_rules_roundtrip_toml() {
        use crate::Config;
        let mut cfg = Config::default();
        cfg.terminal.hyperlink_rules = default_hyperlink_rules();
        let s = toml::to_string(&cfg).expect("serialize");
        let back: Config = toml::from_str(&s).expect("deserialize");
        assert_eq!(
            back.terminal.hyperlink_rules.len(),
            cfg.terminal.hyperlink_rules.len(),
            "hyperlink_rules roundtrip must preserve rule count"
        );
        for (a, b) in cfg
            .terminal
            .hyperlink_rules
            .iter()
            .zip(back.terminal.hyperlink_rules.iter())
        {
            assert_eq!(a.regex, b.regex, "regex must survive roundtrip");
            assert_eq!(a.label, b.label, "label must survive roundtrip");
        }
    }

    #[test]
    fn hyperlink_rules_empty_regex_fails_validate() {
        let mut cfg = TerminalConfig::default();
        cfg.hyperlink_rules.push(HyperlinkRule {
            regex: "  ".to_string(), // blank → should be rejected
            label: "bad".to_string(),
        });
        assert!(
            cfg.validate().is_err(),
            "empty/blank regex must fail validate()"
        );
    }

    #[test]
    fn hyperlink_rules_valid_regex_passes_validate() {
        let mut cfg = TerminalConfig::default();
        cfg.hyperlink_rules
            .push(HyperlinkRule::new(r"https?://\S+", "HTTP/HTTPS URL"));
        assert!(cfg.validate().is_ok(), "valid regex must pass validate()");
    }

    #[test]
    fn hyperlink_rules_parses_from_toml() {
        let toml_src = r#"
[[terminal.hyperlink_rules]]
regex = "https?://\\S+"
label = "HTTP URL"

[[terminal.hyperlink_rules]]
regex = "\\b[0-9a-f]{7,40}\\b"
label = "Git SHA"
"#;
        let cfg: crate::Config = toml::from_str(toml_src).expect("must parse");
        cfg.validate().expect("must validate");
        assert_eq!(cfg.terminal.hyperlink_rules.len(), 2);
        assert_eq!(cfg.terminal.hyperlink_rules[0].label, "HTTP URL");
        assert_eq!(cfg.terminal.hyperlink_rules[1].label, "Git SHA");
    }

    #[test]
    fn hyperlink_rule_new_sets_fields() {
        let r = HyperlinkRule::new(r"\d+", "digits");
        assert_eq!(r.regex, r"\d+");
        assert_eq!(r.label, "digits");
    }

    // ── ImageProtocolsConfig tests ────────────────────────────────────────────

    #[test]
    fn image_protocols_all_default_true() {
        let cfg = ImageProtocolsConfig::default();
        assert!(cfg.osc1337, "osc1337 must default to true");
        assert!(cfg.sixel, "sixel must default to true");
        assert!(cfg.apc, "apc must default to true");
    }

    #[test]
    fn image_protocols_roundtrip_toml() {
        let toml_src = r#"
[terminal.image_protocols]
osc1337 = false
sixel = true
apc = false
"#;
        let cfg: crate::Config = toml::from_str(toml_src).expect("must parse");
        cfg.validate().expect("must validate");
        assert!(!cfg.terminal.image_protocols.osc1337);
        assert!(cfg.terminal.image_protocols.sixel);
        assert!(!cfg.terminal.image_protocols.apc);

        let s = toml::to_string(&cfg).expect("must serialize");
        let back: crate::Config = toml::from_str(&s).expect("must roundtrip");
        assert!(!back.terminal.image_protocols.osc1337);
        assert!(back.terminal.image_protocols.sixel);
        assert!(!back.terminal.image_protocols.apc);
    }

    #[test]
    fn image_protocols_default_in_config_default() {
        let cfg = crate::Config::default();
        assert!(cfg.terminal.image_protocols.osc1337);
        assert!(cfg.terminal.image_protocols.sixel);
        assert!(cfg.terminal.image_protocols.apc);
        cfg.validate().expect("default config must validate");
    }

    // ── KeyboardEncoding tests ────────────────────────────────────────────────

    #[test]
    fn keyboard_encoding_default_is_auto() {
        assert_eq!(KeyboardEncoding::default(), KeyboardEncoding::Auto);
        assert_eq!(
            TerminalConfig::default().keyboard_encoding,
            KeyboardEncoding::Auto
        );
    }

    #[test]
    fn keyboard_encoding_roundtrip_toml() {
        for variant in KeyboardEncoding::all() {
            let mut cfg = crate::Config::default();
            cfg.terminal.keyboard_encoding = variant;
            let raw = toml::to_string(&cfg).expect("serialise");
            let back: crate::Config = toml::from_str(&raw).expect("deserialise");
            assert_eq!(
                back.terminal.keyboard_encoding, variant,
                "KeyboardEncoding::{variant:?} must round-trip through Config"
            );
        }
    }

    #[test]
    fn keyboard_encoding_parses_auto() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nkeyboard_encoding = \"auto\"\n").expect("must parse auto");
        assert_eq!(cfg.terminal.keyboard_encoding, KeyboardEncoding::Auto);
    }

    #[test]
    fn keyboard_encoding_parses_always_csi() {
        let cfg: crate::Config = toml::from_str("[terminal]\nkeyboard_encoding = \"always_csi\"\n")
            .expect("must parse always_csi");
        assert_eq!(cfg.terminal.keyboard_encoding, KeyboardEncoding::AlwaysCsi);
    }

    #[test]
    fn keyboard_encoding_absent_defaults_to_auto() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\n").expect("must parse empty terminal section");
        assert_eq!(cfg.terminal.keyboard_encoding, KeyboardEncoding::Auto);
    }

    // ── BroadcastScope tests ──────────────────────────────────────────────────

    #[test]
    fn broadcast_scope_default_is_all_panes_in_tab() {
        assert_eq!(BroadcastScope::default(), BroadcastScope::AllPanesInTab);
        assert_eq!(
            TerminalConfig::default().broadcast_scope,
            BroadcastScope::AllPanesInTab
        );
    }

    #[test]
    fn broadcast_scope_roundtrip_toml() {
        for variant in BroadcastScope::all() {
            let mut cfg = crate::Config::default();
            cfg.terminal.broadcast_scope = variant;
            let raw = toml::to_string(&cfg).expect("serialise");
            let back: crate::Config = toml::from_str(&raw).expect("deserialise");
            assert_eq!(
                back.terminal.broadcast_scope, variant,
                "BroadcastScope::{variant:?} must round-trip through Config"
            );
        }
    }

    #[test]
    fn broadcast_scope_parses_all_panes_in_tab() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nbroadcast_scope = \"all_panes_in_tab\"\n")
                .expect("must parse all_panes_in_tab");
        assert_eq!(cfg.terminal.broadcast_scope, BroadcastScope::AllPanesInTab);
    }

    #[test]
    fn broadcast_scope_parses_all_panes_in_window() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nbroadcast_scope = \"all_panes_in_window\"\n")
                .expect("must parse all_panes_in_window");
        assert_eq!(
            cfg.terminal.broadcast_scope,
            BroadcastScope::AllPanesInWindow
        );
    }

    #[test]
    fn broadcast_scope_absent_defaults_to_all_panes_in_tab() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\n").expect("must parse empty terminal section");
        assert_eq!(cfg.terminal.broadcast_scope, BroadcastScope::AllPanesInTab);
    }

    #[test]
    fn broadcast_scope_labels_are_non_empty() {
        for variant in BroadcastScope::all() {
            assert!(
                !variant.label().is_empty(),
                "BroadcastScope::{variant:?} must have a non-empty label"
            );
        }
    }

    // ── link_hover_tooltip / link_hover_delay_ms tests ────────────────────────

    #[test]
    fn link_hover_tooltip_default_is_true() {
        let cfg = TerminalConfig::default();
        assert!(
            cfg.link_hover_tooltip,
            "link_hover_tooltip must default to true"
        );
    }

    #[test]
    fn link_hover_delay_ms_default_is_zero() {
        let cfg = TerminalConfig::default();
        assert_eq!(
            cfg.link_hover_delay_ms, 0,
            "link_hover_delay_ms must default to 0"
        );
    }

    #[test]
    fn link_hover_tooltip_roundtrip_toml() {
        let mut cfg = crate::Config::default();
        cfg.terminal.link_hover_tooltip = false;
        cfg.terminal.link_hover_delay_ms = 500;
        let raw = toml::to_string(&cfg).expect("serialise");
        let back: crate::Config = toml::from_str(&raw).expect("deserialise");
        assert!(
            !back.terminal.link_hover_tooltip,
            "link_hover_tooltip=false must roundtrip"
        );
        assert_eq!(
            back.terminal.link_hover_delay_ms, 500,
            "link_hover_delay_ms=500 must roundtrip"
        );
    }

    #[test]
    fn link_hover_delay_ms_validation() {
        // 2001 is out of range.
        let bad = TerminalConfig {
            link_hover_delay_ms: 2001,
            ..Default::default()
        };
        assert!(bad.validate().is_err(), "2001 ms must be rejected");
        // 2000 is the max and must pass.
        let at_max = TerminalConfig {
            link_hover_delay_ms: 2000,
            ..Default::default()
        };
        assert!(at_max.validate().is_ok(), "2000 ms must be accepted");
        // 0 must pass.
        let at_zero = TerminalConfig {
            link_hover_delay_ms: 0,
            ..Default::default()
        };
        assert!(at_zero.validate().is_ok(), "0 ms must be accepted");
    }

    // ── ClipboardReadPolicy tests ─────────────────────────────────────────────

    #[test]
    fn clipboard_read_policy_default_is_deny() {
        assert_eq!(
            ClipboardReadPolicy::default(),
            ClipboardReadPolicy::Deny,
            "ClipboardReadPolicy must default to Deny"
        );
        assert_eq!(
            TerminalConfig::default().clipboard_read,
            ClipboardReadPolicy::Deny,
            "TerminalConfig.clipboard_read must default to Deny"
        );
    }

    #[test]
    fn clipboard_read_policy_roundtrip_toml() {
        for policy in ClipboardReadPolicy::all() {
            let mut cfg = crate::Config::default();
            cfg.terminal.clipboard_read = policy;
            let raw = toml::to_string(&cfg).expect("serialise");
            let back: crate::Config = toml::from_str(&raw).expect("deserialise");
            assert_eq!(
                back.terminal.clipboard_read, policy,
                "ClipboardReadPolicy::{policy:?} must round-trip through Config"
            );
        }
    }

    #[test]
    fn clipboard_read_policy_parses_deny() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nclipboard_read = \"deny\"\n").expect("must parse deny");
        assert_eq!(cfg.terminal.clipboard_read, ClipboardReadPolicy::Deny);
    }

    #[test]
    fn clipboard_read_policy_parses_allow() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\nclipboard_read = \"allow\"\n").expect("must parse allow");
        assert_eq!(cfg.terminal.clipboard_read, ClipboardReadPolicy::Allow);
    }

    #[test]
    fn clipboard_read_policy_absent_defaults_to_deny() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\n").expect("must parse empty terminal section");
        assert_eq!(
            cfg.terminal.clipboard_read,
            ClipboardReadPolicy::Deny,
            "absent clipboard_read must default to Deny"
        );
    }

    #[test]
    fn clipboard_read_policy_labels_non_empty() {
        for policy in ClipboardReadPolicy::all() {
            assert!(
                !policy.label().is_empty(),
                "ClipboardReadPolicy::{policy:?} label must not be empty"
            );
        }
    }

    // ── CommandHistoryScope tests ─────────────────────────────────────────────

    #[test]
    fn command_history_scope_default_is_current_tab() {
        assert_eq!(
            CommandHistoryScope::default(),
            CommandHistoryScope::CurrentTab,
            "CommandHistoryScope must default to CurrentTab"
        );
        assert_eq!(
            TerminalConfig::default().command_history_scope,
            CommandHistoryScope::CurrentTab,
            "TerminalConfig.command_history_scope must default to CurrentTab"
        );
    }

    #[test]
    fn command_history_scope_roundtrip_toml() {
        for scope in CommandHistoryScope::all() {
            let mut cfg = crate::Config::default();
            cfg.terminal.command_history_scope = scope;
            let raw = toml::to_string(&cfg).expect("serialise");
            let back: crate::Config = toml::from_str(&raw).expect("deserialise");
            assert_eq!(
                back.terminal.command_history_scope, scope,
                "CommandHistoryScope::{scope:?} must round-trip through Config"
            );
        }
    }

    #[test]
    fn command_history_scope_parses_current_pane() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\ncommand_history_scope = \"current_pane\"\n")
                .expect("must parse current_pane");
        assert_eq!(
            cfg.terminal.command_history_scope,
            CommandHistoryScope::CurrentPane
        );
    }

    #[test]
    fn command_history_scope_parses_window() {
        let cfg: crate::Config = toml::from_str("[terminal]\ncommand_history_scope = \"window\"\n")
            .expect("must parse window");
        assert_eq!(
            cfg.terminal.command_history_scope,
            CommandHistoryScope::Window
        );
    }

    #[test]
    fn command_history_scope_labels_non_empty() {
        for scope in CommandHistoryScope::all() {
            assert!(
                !scope.label().is_empty(),
                "CommandHistoryScope::{scope:?} label must not be empty"
            );
        }
    }

    #[test]
    fn command_history_max_entries_default_is_500() {
        assert_eq!(
            TerminalConfig::default().command_history_max_entries,
            500,
            "command_history_max_entries must default to 500"
        );
    }

    #[test]
    fn command_history_max_entries_validation() {
        let bad_zero = TerminalConfig {
            command_history_max_entries: 0,
            ..Default::default()
        };
        assert!(
            bad_zero.validate().is_err(),
            "max_entries=0 must fail validate()"
        );
        let bad_too_large = TerminalConfig {
            command_history_max_entries: 10_001,
            ..Default::default()
        };
        assert!(
            bad_too_large.validate().is_err(),
            "max_entries=10_001 must fail validate()"
        );
        let at_max = TerminalConfig {
            command_history_max_entries: 10_000,
            ..Default::default()
        };
        assert!(
            at_max.validate().is_ok(),
            "max_entries=10_000 must pass validate()"
        );
    }

    // ── paste safety config defaults ──────────────────────────────────────────

    #[test]
    fn paste_confirm_multiline_default_is_false() {
        let cfg = TerminalConfig::default();
        assert!(
            !cfg.paste_confirm_multiline,
            "paste_confirm_multiline must default to false"
        );
    }

    #[test]
    fn paste_confirm_when_unbracketed_default_is_true() {
        let cfg = TerminalConfig::default();
        assert!(
            cfg.paste_confirm_when_unbracketed,
            "paste_confirm_when_unbracketed must default to true (safety default)"
        );
    }

    #[test]
    fn paste_strip_control_chars_default_is_false() {
        let cfg = TerminalConfig::default();
        assert!(
            !cfg.paste_strip_control_chars,
            "paste_strip_control_chars must default to false"
        );
    }

    #[test]
    fn paste_safety_fields_roundtrip_toml() {
        let toml_src = r#"
[terminal]
paste_confirm_multiline = true
paste_confirm_when_unbracketed = false
paste_strip_control_chars = true
"#;
        let cfg: crate::Config = toml::from_str(toml_src).expect("paste safety fields must parse");
        cfg.validate().expect("must validate");
        assert!(cfg.terminal.paste_confirm_multiline);
        assert!(!cfg.terminal.paste_confirm_when_unbracketed);
        assert!(cfg.terminal.paste_strip_control_chars);

        let s = toml::to_string(&cfg).expect("must serialize");
        let back: crate::Config = toml::from_str(&s).expect("must roundtrip");
        assert!(back.terminal.paste_confirm_multiline);
        assert!(!back.terminal.paste_confirm_when_unbracketed);
        assert!(back.terminal.paste_strip_control_chars);
    }

    #[test]
    fn paste_safety_absent_keys_use_defaults() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\n").expect("must parse empty terminal section");
        assert!(
            !cfg.terminal.paste_confirm_multiline,
            "absent paste_confirm_multiline must default to false"
        );
        assert!(
            cfg.terminal.paste_confirm_when_unbracketed,
            "absent paste_confirm_when_unbracketed must default to true"
        );
        assert!(
            !cfg.terminal.paste_strip_control_chars,
            "absent paste_strip_control_chars must default to false"
        );
    }

    // ── highlight_on_jump tests ───────────────────────────────────────────────

    #[test]
    fn highlight_on_jump_defaults_true() {
        let cfg = TerminalConfig::default();
        assert!(
            cfg.highlight_on_jump,
            "highlight_on_jump must default to true"
        );
    }

    #[test]
    fn highlight_on_jump_roundtrip_toml() {
        let toml_src = "[terminal]\nhighlight_on_jump = false\n";
        let cfg: crate::Config = toml::from_str(toml_src).expect("must parse");
        assert!(
            !cfg.terminal.highlight_on_jump,
            "highlight_on_jump = false must round-trip correctly"
        );
    }

    #[test]
    fn highlight_on_jump_absent_defaults_to_true() {
        let cfg: crate::Config =
            toml::from_str("[terminal]\n").expect("must parse empty terminal section");
        assert!(
            cfg.terminal.highlight_on_jump,
            "absent highlight_on_jump must default to true"
        );
    }
}
