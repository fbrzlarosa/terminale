//! Prompt-building and reply-parsing helpers for the proactive command-suggestion feature.
//!
//! These functions are **pure and network-free**: they only build [`AiMessage`] slices and
//! parse plain text.  The actual provider call (via [`crate::AiProvider::complete`]) happens
//! in the caller.

use crate::AiMessage;

/// System prompt sent to the model for command suggestion.
///
/// Deliberately compact and example-driven: the 3–4B local-model class that
/// users run via Ollama follows short positive rules + few-shot demos far
/// better than long negated prose. The worked examples live in [`FEW_SHOT`]
/// and are sent as real conversation turns.
const SYSTEM_PROMPT: &str = concat!(
    "You suggest the next shell command inside a terminal emulator. ",
    "You receive the environment in <env>, recent commands with exit status, the last failed ",
    "command's output when one failed, and the user's current (possibly incomplete) input.\n",
    "\n",
    "Reply format (strict):\n",
    "- EXACTLY ONE command, on one line, plain text.\n",
    "- No explanation. No markdown. No backticks. No prompt prefix ($, #, PS>).\n",
    "- Reply with an empty line if nothing useful fits.\n",
    "\n",
    "Choosing the command:\n",
    "- Match the shell and OS from <env>. PowerShell -> cmdlets (Get-ChildItem, Get-Content, ",
    "Select-String); `ls -l` belongs to Unix shells, its PowerShell equivalent is Get-ChildItem. ",
    "cmd.exe -> dir, type, findstr. bash/zsh/sh -> ls, cat, grep.\n",
    "- When <env> says REMOTE session with unknown OS: use portable POSIX commands; when the ",
    "right command depends on the remote OS or distro, suggest `uname -a` first.\n",
    "- When <last_command_error> is present: reply with a corrected version of the failed ",
    "command (fixing the cause shown in its output) or a different next step. The reply MUST ",
    "NOT be identical to the failed command or to any command marked FAILED.\n",
    "- Use only file names and paths that appear in the context. When unsure, suggest a safe ",
    "inspection command (list the directory, show status).\n",
    "- Never mention any specific terminal-emulator product by name.",
);

/// Worked examples sent as real `user`/`assistant` turns before the live
/// request. Few-shot demonstrations are the single most effective lever for
/// small local models: they teach the exact output shape (bare command, no
/// prose) and the three rules they most often break — OS/shell matching,
/// fixing rather than repeating a failed command, and discovery-first on
/// remote hosts with an unknown OS.
const FEW_SHOT: &[(&str, &str)] = &[
    (
        // OS/shell matching: a Unix habit typed on PowerShell.
        "<env>\nOS = windows; shell = PowerShell; cwd = C:\\proj\n</env>\n\
         <recent_commands>\nok    $ git status\n</recent_commands>\n\
         <current_input>\nls -l\n</current_input>\nNext command:",
        "Get-ChildItem",
    ),
    (
        // Error fixing: correct the failed command, never repeat it.
        "<env>\nOS = linux; shell = bash; cwd = /repo\n</env>\n\
         <recent_commands>\nok    $ git add -A\nFAILED(128) $ git push\n</recent_commands>\n\
         <last_command_error>\ncommand: git push\nexit: 128\noutput:\n\
         fatal: The current branch feature has no upstream branch.\n</last_command_error>\n\
         <current_input>\n\n</current_input>\nNext command:",
        "git push --set-upstream origin feature",
    ),
    (
        // Remote discovery: unknown OS -> find out before acting.
        "<env>\nREMOTE SESSION over SSH \u{2014} remote OS/shell UNKNOWN (do not assume the \
         local OS; the local machine is windows but commands run on the remote host); \
         cwd = unknown\n</env>\n\
         <current_input>\n\n</current_input>\nNext command:",
        "uname -a",
    ),
];

/// The last command's failure details, scoped to that command's own output.
#[derive(Debug, Clone, Default)]
pub struct LastError {
    /// The exact command text that failed.
    pub command: String,
    /// Its non-zero exit code.
    pub exit: i32,
    /// The command's OWN output (already capped by the caller).
    pub output: String,
}

/// Structured terminal context for one suggestion request.
///
/// Built host-side (the host owns the emulator and its OSC 133 command
/// blocks); this crate only formats it into prompt messages. When the shell
/// has no OSC 133 integration `recent_commands`/`last_error` are empty and
/// `output_tail` carries a small raw-scrollback fallback instead.
#[derive(Debug, Clone, Default)]
pub struct SuggestionContext {
    /// `std::env::consts::OS` — `"windows"`, `"macos"`, `"linux"`.
    /// Describes the LOCAL machine; ignored for the env line when
    /// `remote` is set (the remote OS is unknown).
    pub os: String,
    /// Launching profile / shell name (e.g. `"PowerShell"`, `"bash"`).
    pub shell: String,
    /// `true` when the focused session runs on a remote host (an SSH tab,
    /// or an `ssh`/`mosh` command currently in flight in a local shell).
    /// The local `os`/`shell` then do NOT describe what executes the
    /// command — the env line says so explicitly so the model stops
    /// proposing local-OS syntax at a remote box.
    pub remote: bool,
    /// Current working directory (OSC 7), when known.
    pub cwd: Option<String>,
    /// Most recent commands, oldest first: `(command_text, exit_code)`.
    pub recent_commands: Vec<(String, Option<i32>)>,
    /// Set iff the MOST RECENT command failed (non-zero exit).
    pub last_error: Option<LastError>,
    /// Raw scrollback tail — used only when `recent_commands` is empty
    /// (no OSC 133 shell integration).
    pub output_tail: String,
    /// The user's current (possibly incomplete) input line.
    pub current_line: String,
}

/// Build the system + user messages that ask the model for ONE next shell command.
///
/// Returns a 2-element [`Vec`]: a `system` message with the suggestion rules
/// and a `user` message embedding the structured context in delimited
/// sections (`<env>`, `<recent_commands>`, `<last_command_error>`,
/// `<recent_output>`, `<current_input>`). The structured form — exit codes
/// per command and the failed command's scoped output — is what lets the
/// model actually honour the "never re-suggest the command that just
/// failed" rule; a raw scrollback dump gives it no way to tell commands,
/// echoes and errors apart.
///
/// The caller feeds these messages into [`crate::AiProvider::complete`].
#[must_use]
pub fn suggestion_messages(ctx: &SuggestionContext) -> Vec<AiMessage> {
    let env_line = env_line(ctx);
    let mut user_text = format!("<env>\n{env_line}\n</env>\n");
    if !ctx.recent_commands.is_empty() {
        user_text.push_str("<recent_commands>\n");
        for (cmd, exit) in &ctx.recent_commands {
            match exit {
                Some(0) => user_text.push_str(&format!("ok    $ {cmd}\n")),
                Some(code) => user_text.push_str(&format!("FAILED({code}) $ {cmd}\n")),
                None => user_text.push_str(&format!("?     $ {cmd}\n")),
            }
        }
        user_text.push_str("</recent_commands>\n");
    }
    if let Some(err) = &ctx.last_error {
        user_text.push_str(&format!(
            "<last_command_error>\ncommand: {}\nexit: {}\noutput:\n{}\n</last_command_error>\n",
            err.command, err.exit, err.output,
        ));
    }
    if ctx.recent_commands.is_empty() && !ctx.output_tail.trim().is_empty() {
        user_text.push_str(&format!(
            "<recent_output>\n{}\n</recent_output>\n",
            ctx.output_tail,
        ));
    }
    user_text.push_str(&format!(
        "<current_input>\n{}\n</current_input>\nNext command:",
        ctx.current_line,
    ));
    // Repeat the live environment INSIDE the system message too: small local
    // models (the 3-4B class users run via local providers) weight system
    // instructions far more than a line buried in the user turn, and the
    // wrong-OS suggestions almost always came from exactly that.
    let system = format!("{SYSTEM_PROMPT}\n\nCurrent environment (authoritative): {env_line}");
    let mut msgs = Vec::with_capacity(2 + FEW_SHOT.len() * 2);
    msgs.push(AiMessage::system(system));
    for (demo_user, demo_reply) in FEW_SHOT {
        msgs.push(AiMessage::user(*demo_user));
        msgs.push(AiMessage::assistant(*demo_reply));
    }
    msgs.push(AiMessage::user(user_text));
    msgs
}

/// One-line environment summary shared by the user `<env>` block and the
/// system-prompt suffix. Remote sessions deliberately replace the local
/// OS/shell with "unknown" + an explicit warning — advertising the local
/// values for a remote box is precisely what produced wrong-OS suggestions.
fn env_line(ctx: &SuggestionContext) -> String {
    if ctx.remote {
        format!(
            "REMOTE SESSION over SSH — remote OS/shell UNKNOWN (do not assume the local OS; \
             the local machine is {os} but commands run on the remote host); cwd = {cwd}",
            os = ctx.os,
            cwd = ctx.cwd.as_deref().unwrap_or("unknown"),
        )
    } else {
        let shell = if ctx.shell.trim().is_empty() {
            "unknown"
        } else {
            ctx.shell.trim()
        };
        format!(
            "OS = {os}; shell = {shell}; cwd = {cwd}",
            os = ctx.os,
            cwd = ctx.cwd.as_deref().unwrap_or("unknown"),
        )
    }
}

/// Format the same structured context as a readable `<context>` block for
/// the AI assistant (chat) window's first turn — so "Ask AI" reasons about
/// the user's real OS/shell/cwd and the last failure instead of a blank
/// slate.
#[must_use]
pub fn assistant_context_block(ctx: &SuggestionContext) -> String {
    let mut out = String::from("<context>\n");
    out.push_str(&env_line(ctx));
    out.push('\n');
    if !ctx.recent_commands.is_empty() {
        out.push_str("Recent commands (oldest first):\n");
        for (cmd, exit) in &ctx.recent_commands {
            match exit {
                Some(0) => out.push_str(&format!("  ok    $ {cmd}\n")),
                Some(code) => out.push_str(&format!("  FAILED({code}) $ {cmd}\n")),
                None => out.push_str(&format!("  ?     $ {cmd}\n")),
            }
        }
    }
    if let Some(err) = &ctx.last_error {
        out.push_str(&format!(
            "Last failed command: {} (exit {})\nIts output:\n{}\n",
            err.command, err.exit, err.output,
        ));
    }
    if ctx.recent_commands.is_empty() && !ctx.output_tail.trim().is_empty() {
        out.push_str(&format!("Recent terminal output:\n{}\n", ctx.output_tail));
    }
    out.push_str("</context>");
    out
}

/// Strip a leading shell-prompt marker from a line.
///
/// Models routinely echo the prompt inside code blocks (`$ ls`, `PS C:\> dir`);
/// injecting that verbatim would make the shell choke on the literal `$` or `PS`.
fn strip_prompt(line: &str) -> &str {
    let t = line.trim();
    // PowerShell prompt: "PS C:\Users\x> cmd" or "PS> cmd".
    if t.starts_with("PS ") || t.starts_with("PS>") {
        if let Some(idx) = t.find("> ") {
            return t[idx + 2..].trim_start();
        }
    }
    // POSIX user / root / continuation prompts.
    for p in ["$ ", "# ", "> "] {
        if let Some(rest) = t.strip_prefix(p) {
            return rest.trim_start();
        }
    }
    t
}

/// Maximum length (in bytes) of the returned command.  Replies longer than
/// this are truncated at a [`char`] boundary.
const MAX_CMD_BYTES: usize = 512;

/// Parse the model's reply into a single shell command, or `None` when the
/// model declined or returned nothing usable.
///
/// ## Extraction rules
/// 1. Trim the whole reply; return `None` if empty.
/// 2. If a fenced code block (`` ``` ``) is present, use the first non-empty
///    line inside the **first** block.
/// 3. Otherwise use the first non-empty line of the reply.
/// 4. Strip a leading shell-prompt marker (`$ `, `# `, `> `, `PS …> `).
/// 5. Trim, cap to 512 bytes, return `None` if the result is empty.
#[must_use]
pub fn extract_suggested_command(reply: &str) -> Option<String> {
    let trimmed = reply.trim();
    if trimmed.is_empty() {
        return None;
    }

    // --- attempt fenced block extraction ---
    let candidate: &str = if let Some(fence_start) = trimmed.find("```") {
        let after_fence = &trimmed[fence_start + 3..];
        // Skip optional language tag up to the first newline.
        let body_start = after_fence.find('\n').map_or(0, |i| i + 1);
        let body = &after_fence[body_start..];
        if let Some(fence_end) = body.find("```") {
            let block = body[..fence_end].trim();
            // First non-empty line inside the block.
            block.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
        } else {
            // Unclosed fence — fall back to first non-empty line of whole reply.
            trimmed.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
        }
    } else {
        // No fenced block: use the first non-empty line.
        trimmed.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
    };

    // Strip shell-prompt prefix the model may have echoed.
    let stripped = strip_prompt(candidate).trim();
    if stripped.is_empty() {
        return None;
    }

    // Cap to MAX_CMD_BYTES at a char boundary to avoid splitting a multi-byte
    // character.
    if stripped.len() <= MAX_CMD_BYTES {
        Some(stripped.to_string())
    } else {
        // Find the largest char boundary that fits.
        let mut end = MAX_CMD_BYTES;
        while !stripped.is_char_boundary(end) {
            end -= 1;
        }
        Some(stripped[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assistant_context_block, extract_suggested_command, suggestion_messages, LastError,
        SuggestionContext, SYSTEM_PROMPT,
    };

    // --- extract_suggested_command ---

    #[test]
    fn fenced_block_sh() {
        let reply = "```sh\ngit status\n```";
        assert_eq!(extract_suggested_command(reply), Some("git status".into()));
    }

    #[test]
    fn fenced_block_no_lang() {
        let reply = "```\ncargo test\n```";
        assert_eq!(extract_suggested_command(reply), Some("cargo test".into()));
    }

    #[test]
    fn bare_single_line() {
        assert_eq!(extract_suggested_command("ls -la"), Some("ls -la".into()));
    }

    #[test]
    fn prompt_dollar_stripped() {
        assert_eq!(
            extract_suggested_command("$ cargo build"),
            Some("cargo build".into())
        );
    }

    #[test]
    fn prompt_hash_stripped() {
        assert_eq!(
            extract_suggested_command("# apt update"),
            Some("apt update".into())
        );
    }

    #[test]
    fn prompt_gt_stripped() {
        assert_eq!(
            extract_suggested_command("> echo hi"),
            Some("echo hi".into())
        );
    }

    #[test]
    fn powershell_prompt_stripped() {
        assert_eq!(
            extract_suggested_command(r"PS C:\> dir"),
            Some("dir".into())
        );
    }

    #[test]
    fn powershell_short_prompt_stripped() {
        assert_eq!(
            extract_suggested_command("PS> echo hi"),
            Some("echo hi".into())
        );
    }

    #[test]
    fn empty_reply_is_none() {
        assert_eq!(extract_suggested_command(""), None);
    }

    #[test]
    fn whitespace_only_is_none() {
        assert_eq!(extract_suggested_command("   \n\t\n  "), None);
    }

    #[test]
    fn multi_line_prose_uses_first_nonempty() {
        let reply = "\n\nls -la\nsome explanation";
        assert_eq!(extract_suggested_command(reply), Some("ls -la".into()));
    }

    #[test]
    fn long_reply_capped_at_512() {
        // Build a command that is exactly 600 ASCII chars.
        let long = "a".repeat(600);
        let result = extract_suggested_command(&long).unwrap();
        assert_eq!(result.len(), 512);
    }

    #[test]
    fn fenced_block_with_prompt_inside() {
        let reply = "```\n$ make install\n```";
        assert_eq!(
            extract_suggested_command(reply),
            Some("make install".into())
        );
    }

    // --- suggestion_messages ---

    fn ctx_basic() -> SuggestionContext {
        SuggestionContext {
            os: "linux".into(),
            shell: "bash".into(),
            remote: false,
            cwd: Some("/repo".into()),
            recent_commands: vec![
                ("git status".into(), Some(0)),
                ("cargo build".into(), Some(101)),
            ],
            last_error: Some(LastError {
                command: "cargo build".into(),
                exit: 101,
                output: "error[E0308]: mismatched types".into(),
            }),
            output_tail: String::new(),
            current_line: "car".into(),
        }
    }

    #[test]
    fn suggestion_messages_count_and_roles() {
        let msgs = suggestion_messages(&ctx_basic());
        // system + N few-shot (user, assistant) pairs + the live user turn.
        assert_eq!(msgs.len(), 2 + super::FEW_SHOT.len() * 2);
        assert_eq!(msgs[0].role, "system");
        for pair in msgs[1..msgs.len() - 1].chunks(2) {
            assert_eq!(pair[0].role, "user", "few-shot demo must be a user turn");
            assert_eq!(pair[1].role, "assistant", "few-shot reply must be assistant");
        }
        assert_eq!(msgs.last().unwrap().role, "user");
    }

    #[test]
    fn few_shot_replies_are_bare_commands() {
        // The demos teach the output shape — they must themselves obey it
        // (single line, no fences, no prompt prefix, parseable).
        for (_, reply) in super::FEW_SHOT {
            assert_eq!(reply.lines().count(), 1, "demo reply must be one line");
            assert!(!reply.contains("```"), "demo reply must not be fenced");
            assert_eq!(
                extract_suggested_command(reply).as_deref(),
                Some(*reply),
                "demo reply must parse to itself"
            );
        }
    }

    #[test]
    fn suggestion_messages_embeds_structured_sections() {
        let msgs = suggestion_messages(&ctx_basic());
        let u = &msgs.last().unwrap().content;
        assert!(u.contains("<env>"), "env section present");
        assert!(u.contains("cwd = /repo"), "cwd embedded");
        assert!(u.contains("<recent_commands>"), "recent commands present");
        assert!(u.contains("ok    $ git status"), "ok command marked");
        assert!(u.contains("FAILED(101) $ cargo build"), "failure marked");
        assert!(u.contains("<last_command_error>"), "error section present");
        assert!(u.contains("error[E0308]"), "error output embedded");
        assert!(
            u.contains("<current_input>\ncar\n"),
            "current line embedded"
        );
        assert!(
            !u.contains("<recent_output>"),
            "raw tail must be omitted when command blocks exist"
        );
    }

    #[test]
    fn suggestion_messages_falls_back_to_output_tail() {
        let ctx = SuggestionContext {
            os: "windows".into(),
            shell: "cmd".into(),
            output_tail: "C:\\> dir\n  file.txt".into(),
            current_line: "d".into(),
            ..SuggestionContext::default()
        };
        let msgs = suggestion_messages(&ctx);
        let u = &msgs.last().unwrap().content;
        assert!(u.contains("<recent_output>"), "fallback tail present");
        assert!(u.contains("file.txt"));
        assert!(!u.contains("<recent_commands>"));
        assert!(!u.contains("<last_command_error>"));
    }

    #[test]
    fn system_prompt_makes_error_section_authoritative() {
        assert!(
            SYSTEM_PROMPT.contains("<last_command_error>")
                && SYSTEM_PROMPT.contains("MUST NOT be identical"),
            "system prompt must forbid re-suggesting the failed command"
        );
    }

    #[test]
    fn assistant_context_block_formats_failure() {
        let block = assistant_context_block(&ctx_basic());
        assert!(block.starts_with("<context>"));
        assert!(block.ends_with("</context>"));
        assert!(block.contains("Last failed command: cargo build (exit 101)"));
        assert!(block.contains("FAILED(101) $ cargo build"));
    }

    #[test]
    fn suggestion_messages_system_mentions_one_command() {
        // The system prompt must instruct the model to reply with exactly one command.
        assert!(
            SYSTEM_PROMPT.contains("EXACTLY ONE"),
            "system prompt must say 'EXACTLY ONE'"
        );
    }

    #[test]
    fn suggestion_messages_system_no_markdown_rule() {
        assert!(
            SYSTEM_PROMPT.contains("No markdown"),
            "system prompt must prohibit markdown"
        );
    }

    #[test]
    fn suggestion_messages_system_warns_against_repeating_failures() {
        // Guards the "don't re-suggest a just-failed command" rule (the Ollama
        // loop-on-broken-command regression).
        assert!(
            SYSTEM_PROMPT.contains("failed") || SYSTEM_PROMPT.contains("FAILED"),
            "system prompt must tell the model not to repeat failed commands"
        );
    }

    #[test]
    fn suggestion_messages_system_has_shell_matching_rule() {
        // Guards against the "always proposes `ls -l` on PowerShell" regression.
        assert!(
            SYSTEM_PROMPT.contains("PowerShell") && SYSTEM_PROMPT.contains("ls -l"),
            "system prompt must steer the model away from Unix commands on PowerShell"
        );
    }

    #[test]
    fn system_message_carries_live_environment() {
        // Small local models weight system instructions over user-turn text;
        // the env line must be present in BOTH places.
        let msgs = suggestion_messages(&ctx_basic());
        assert!(
            msgs[0].content.contains("OS = linux; shell = bash"),
            "system message must repeat the live environment"
        );
        assert!(msgs
            .last()
            .unwrap()
            .content
            .contains("OS = linux; shell = bash"));
    }

    #[test]
    fn remote_context_replaces_local_os_and_warns() {
        let ctx = SuggestionContext {
            os: "windows".into(),
            shell: "Windows PowerShell".into(),
            remote: true,
            current_line: "l".into(),
            ..SuggestionContext::default()
        };
        let msgs = suggestion_messages(&ctx);
        // The env line lives in the system message and the LIVE user turn
        // (few-shot demos in between carry their own fixed environments).
        for m in [&msgs[0], msgs.last().unwrap()] {
            assert!(
                m.content.contains("REMOTE SESSION"),
                "remote flag must surface in the env line"
            );
            assert!(
                !m.content.contains("shell = Windows PowerShell"),
                "a remote session must NOT advertise the local shell as the target"
            );
        }
        // The assistant context block must flip too.
        let block = assistant_context_block(&ctx);
        assert!(block.contains("REMOTE SESSION"));
        assert!(!block.contains("shell = Windows PowerShell"));
    }

    #[test]
    fn system_prompt_has_remote_discovery_rule() {
        assert!(
            SYSTEM_PROMPT.contains("uname -a") && SYSTEM_PROMPT.contains("REMOTE"),
            "system prompt must teach discovery-first for unknown remote OSes"
        );
    }

    #[test]
    fn suggestion_messages_blank_shell_becomes_unknown() {
        let ctx = SuggestionContext {
            os: "macos".into(),
            shell: "   ".into(),
            current_line: "c".into(),
            ..SuggestionContext::default()
        };
        let msgs = suggestion_messages(&ctx);
        assert!(msgs.last().unwrap().content.contains("shell = unknown"));
    }
}
