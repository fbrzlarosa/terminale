//! Per-window runtime state for the proactive AI command-suggestion bar.
//!
//! This module owns the debounce logic, the provider-usability check, and the
//! pure decision function that `about_to_wait` calls each tick.  All state that
//! changes during a normal interaction (Loading spinner frame, idle timer, …)
//! lives on [`SuggestionRuntime`]; the request-spawn logic in `main.rs` just
//! reads/writes these fields and fires a Tokio task.

use std::time::{Duration, Instant};
use terminale_config::SuggestionTrigger;

// ── Public state types ────────────────────────────────────────────────────────

/// What the bar is currently showing for one window.
pub enum SuggestionState {
    /// Bar not visible.
    Hidden,
    /// Waiting for the provider to reply — shows a scanning animation.
    Loading,
    /// A command was returned; the bar is showing it.
    Ready(String),
    /// The provider errored; a short message is shown in place of the command.
    Error(String),
    /// Unobtrusive "Fix last command" offer after a non-zero exit
    /// (`ai.offer_fix_on_failure`). Shows a `[Fix]` button instead of
    /// `[Inject]`; shown independently of the auto-suggestion trigger.
    Hint(String),
}

/// Per-window runtime state for the proactive suggestion bar.
pub struct SuggestionRuntime {
    /// Current bar content / visibility.
    pub state: SuggestionState,
    /// Bumped on every new request; stale async results are dropped when
    /// the generation stored here no longer matches the delivered one.
    pub generation: u64,
    /// Wall-clock instant when the focused-pane last received PTY output.
    /// `None` until the first output arrives after window creation.
    pub last_output_at: Option<Instant>,
    /// Gate: at most one Auto suggestion per prompt.  Reset to `false`
    /// when new PTY output arrives (i.e. a new prompt appeared).
    pub fired_for_prompt: bool,
    /// Frame counter for the Loading spinner animation (0-255, wraps).
    pub loading_frame: u8,
    /// Set by the `SuggestCommand` shortcut or palette action to request a
    /// manual (on-demand) suggestion on the next `about_to_wait` tick.
    pub manual_requested: bool,
    /// Mirror of `config.ai.suggestions.enabled`.  Kept on `SuggestionRuntime`
    /// so `render_main` — which takes only `&mut RunningState` without access
    /// to the `App`-level `Config` — can gate the bar with a cheap field read.
    pub enabled: bool,
    /// Mirror of `config.ai.suggestions.trigger`.  `Off` fully hides the bar
    /// and blocks even manual requests.
    pub trigger: SuggestionTrigger,
}

impl Default for SuggestionRuntime {
    fn default() -> Self {
        Self {
            state: SuggestionState::Hidden,
            generation: 0,
            last_output_at: None,
            fired_for_prompt: false,
            loading_frame: 0,
            manual_requested: false,
            enabled: false,
            trigger: SuggestionTrigger::Off,
        }
    }
}

impl SuggestionRuntime {
    /// Call whenever new PTY output was processed for the focused pane.
    ///
    /// Resets the idle timer and clears the per-prompt gate so a new Auto
    /// suggestion can fire after the next idle window.
    pub fn note_output(&mut self) {
        self.last_output_at = Some(Instant::now());
        self.fired_for_prompt = false;
    }
}

// ── Decision logic ────────────────────────────────────────────────────────────

/// Whether an Auto suggestion should fire right now.
///
/// This is a pure function — `now` is injected so it can be unit-tested without
/// sleeping.  The call-site passes `Instant::now()`.
///
/// Returns `true` only when **all** of the following hold:
/// - `enabled` is `true`
/// - `trigger` is [`SuggestionTrigger::Auto`]
/// - the runtime is not already `Loading`
/// - the per-prompt gate (`fired_for_prompt`) is not set
/// - the terminal has been idle (no new PTY output) for at least `idle`
#[must_use]
pub fn should_auto_fire(
    rt: &SuggestionRuntime,
    trigger: SuggestionTrigger,
    enabled: bool,
    idle: Duration,
    now: Instant,
) -> bool {
    if !enabled || trigger != SuggestionTrigger::Auto || rt.fired_for_prompt {
        return false;
    }
    if matches!(rt.state, SuggestionState::Loading) {
        return false;
    }
    match rt.last_output_at {
        // `saturating_duration_since` so a `last_output_at` set slightly after
        // `now` (output drained later in the same event-loop tick) yields 0,
        // never a panic, and simply defers the fire to a later tick.
        Some(t) => now.saturating_duration_since(t) >= idle,
        None => false,
    }
}

/// Index into `Emulator::buffer_lines_text()` of the cursor's current screen
/// row.  `buffer_lines_text()` is laid out as `[scrollback(history_size) … ,
/// visible_screen(rows) …]`, so the visible viewport row `cursor_row` lives at
/// `history_size + cursor_row` — NOT at `cursor_row` (that would point into the
/// scrollback whenever history is non-empty).
#[must_use]
pub fn current_line_index(cursor_row: u16, history_size: usize) -> usize {
    history_size + cursor_row as usize
}

/// Heuristic: is the user currently INSIDE a remote shell started from this
/// local session (an `ssh`/`mosh` command still running)?
///
/// OSC 133 command blocks come from the LOCAL shell; while a remote login is
/// in flight, the most recent block is the `ssh …` invocation with no exit
/// code yet. In that state the local OS/shell stop describing what executes
/// typed commands, so the AI context must flip to "remote, OS unknown".
/// Native SSH tabs don't need this — their `Session::is_remote()` says so
/// directly.
#[must_use]
pub fn inflight_remote_shell(recent_commands: &[(String, Option<i32>)]) -> bool {
    let Some((cmd, exit)) = recent_commands.last() else {
        return false;
    };
    if exit.is_some() {
        return false; // finished — we're back at the local prompt
    }
    let first_word = cmd.trim().split_whitespace().next().unwrap_or("");
    // Strip a path prefix (`/usr/bin/ssh`) and a Windows extension.
    let bare = first_word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first_word)
        .trim_end_matches(".exe");
    matches!(bare, "ssh" | "mosh" | "et")
}

// ── Provider-usability check ──────────────────────────────────────────────────

/// Returns `true` when the configured default AI provider can actually be used.
///
/// The check mirrors the env-beats-config key resolution used by the AI
/// assistant window: a provider is "usable" when it either has an API key
/// stored in the config file **or** the corresponding environment variable is
/// set.  Ollama (local) is always usable because it needs no key.
#[must_use]
pub fn provider_usable(ai: &terminale_config::AiConfig) -> bool {
    match ai.default_provider.trim().to_ascii_lowercase().as_str() {
        "ollama" => true,
        "openai" => !ai.openai.api_key.is_empty() || std::env::var("OPENAI_API_KEY").is_ok(),
        _ => !ai.claude.api_key.is_empty() || std::env::var("ANTHROPIC_API_KEY").is_ok(),
    }
}

// ── Outcome type (mirrored in main.rs as UserEvent::Suggestion) ───────────────

/// Outcome of a one-shot AI suggestion request.  Delivered back to the UI
/// thread via `UserEvent::Suggestion`.
#[derive(Debug, Clone)]
pub enum SuggestionOutcome {
    /// The model returned a usable command string.
    Ready(String),
    /// The request failed or returned no extractable command.
    Error(String),
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_rt(secs_ago: u64) -> SuggestionRuntime {
        SuggestionRuntime {
            last_output_at: Instant::now().checked_sub(Duration::from_secs(secs_ago)),
            ..SuggestionRuntime::default()
        }
    }

    // ── inflight_remote_shell ─────────────────────────────────────────────────

    #[test]
    fn inflight_ssh_detected() {
        let recent = vec![
            ("git status".to_string(), Some(0)),
            ("ssh user@host".to_string(), None),
        ];
        assert!(inflight_remote_shell(&recent));
    }

    #[test]
    fn inflight_detects_path_and_exe_variants() {
        assert!(inflight_remote_shell(&[("/usr/bin/ssh box".into(), None)]));
        assert!(inflight_remote_shell(&[(
            r"C:\Windows\System32\OpenSSH\ssh.exe box".into(),
            None
        )]));
        assert!(inflight_remote_shell(&[("mosh devbox".into(), None)]));
    }

    #[test]
    fn finished_ssh_is_local_again() {
        // Exit code present → the remote session ended; back at local prompt.
        assert!(!inflight_remote_shell(&[("ssh user@host".into(), Some(0))]));
    }

    #[test]
    fn non_ssh_running_command_is_not_remote() {
        assert!(!inflight_remote_shell(&[("cargo build".into(), None)]));
        // Substring traps: `sshuttle` is not `ssh`.
        assert!(!inflight_remote_shell(&[(
            "sshuttle -r host 0/0".into(),
            None
        )]));
        assert!(!inflight_remote_shell(&[]));
    }

    #[test]
    fn fires_after_idle_elapsed() {
        let rt = idle_rt(10);
        assert!(should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            true,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_before_idle_elapsed() {
        let rt = idle_rt(1);
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            true,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_while_loading() {
        let mut rt = idle_rt(10);
        rt.state = SuggestionState::Loading;
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            true,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_twice_per_prompt() {
        let mut rt = idle_rt(10);
        rt.fired_for_prompt = true;
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            true,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_when_disabled() {
        let rt = idle_rt(10);
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            false,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_when_trigger_is_off() {
        let rt = idle_rt(10);
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Off,
            true,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_when_trigger_is_manual() {
        let rt = idle_rt(10);
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Manual,
            true,
            Duration::from_secs(5),
            Instant::now(),
        ));
    }

    #[test]
    fn does_not_fire_without_any_output() {
        let rt = SuggestionRuntime::default(); // last_output_at = None
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            true,
            Duration::from_secs(1),
            Instant::now(),
        ));
    }

    #[test]
    fn note_output_resets_fired_for_prompt() {
        let mut rt = idle_rt(10);
        rt.fired_for_prompt = true;
        rt.note_output();
        assert!(!rt.fired_for_prompt);
    }

    #[test]
    fn current_line_index_accounts_for_scrollback() {
        // No scrollback: index == viewport row.
        assert_eq!(current_line_index(5, 0), 5);
        // With scrollback the visible row is offset past the history prefix.
        assert_eq!(current_line_index(5, 100), 105);
        assert_eq!(current_line_index(0, 0), 0);
    }

    #[test]
    fn note_output_resets_idle_timer() {
        let mut rt = SuggestionRuntime::default();
        rt.note_output();
        // Immediately after note_output the idle has not elapsed yet.
        assert!(!should_auto_fire(
            &rt,
            SuggestionTrigger::Auto,
            true,
            Duration::from_secs(30),
            Instant::now(),
        ));
    }
}
