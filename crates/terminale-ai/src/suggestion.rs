//! Prompt-building and reply-parsing helpers for the proactive command-suggestion feature.
//!
//! These functions are **pure and network-free**: they only build [`AiMessage`] slices and
//! parse plain text.  The actual provider call (via [`crate::AiProvider::complete`]) happens
//! in the caller.

use crate::AiMessage;

/// System prompt sent to the model for command suggestion.
///
/// Instructs the model to reply with exactly one shell command and nothing
/// else — no explanation, no markdown, no backticks.  If it cannot produce a
/// useful suggestion it must reply with an empty line.
const SYSTEM_PROMPT: &str = concat!(
    "You are a shell command suggester embedded in a terminal emulator. ",
    "You will be given structured terminal context (environment, recent commands with their ",
    "exit status, the last failed command's output when one failed) and the user's current ",
    "(possibly incomplete) input line. ",
    "Your job is to propose EXACTLY ONE next shell command the user most likely wants to run.\n",
    "\n",
    "Rules (follow strictly):\n",
    "- Reply with ONLY the command on a single line.\n",
    "- No explanation. No markdown. No backticks. No bullet points.\n",
    "- Do not repeat the prompt prefix (e.g. do not echo `$` or `PS >`).\n",
    "- If you cannot suggest anything useful, reply with an empty line and nothing else.\n",
    "- Treat <last_command_error> as authoritative: when it is present, your suggestion MUST ",
    "be either a CORRECTED version of that command (fixing the cause shown in its output) or a ",
    "clearly different next step. It MUST NOT be identical to the failed command, and MUST NOT ",
    "be identical to any command marked FAILED in <recent_commands>.\n",
    "- Do not invent file names, paths, or arguments that are not clearly present in the context. ",
    "When unsure, prefer a safe, broadly-useful command (list the current directory, check status) over guessing.\n",
    "- Use the syntax of the user's ACTUAL shell and OS (stated in <env>). ",
    "On Windows PowerShell use cmdlets such as Get-ChildItem / dir and NEVER `ls -l`; ",
    "on cmd.exe use dir; only use Unix commands (ls, grep, cat, ...) when the shell is clearly bash / zsh / sh.\n",
    "- Never mention any specific terminal-emulator product by name.",
);

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
    pub os: String,
    /// Launching profile / shell name (e.g. `"PowerShell"`, `"bash"`).
    pub shell: String,
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
    let shell = if ctx.shell.trim().is_empty() {
        "unknown"
    } else {
        ctx.shell.trim()
    };
    let mut user_text = format!(
        "<env>\nOS = {os}; shell = {shell}; cwd = {cwd}\n</env>\n",
        os = ctx.os,
        cwd = ctx.cwd.as_deref().unwrap_or("unknown"),
    );
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
    vec![AiMessage::system(SYSTEM_PROMPT), AiMessage::user(user_text)]
}

/// Format the same structured context as a readable `<context>` block for
/// the AI assistant (chat) window's first turn — so "Ask AI" reasons about
/// the user's real OS/shell/cwd and the last failure instead of a blank
/// slate.
#[must_use]
pub fn assistant_context_block(ctx: &SuggestionContext) -> String {
    let mut out = String::from("<context>\n");
    out.push_str(&format!(
        "OS: {} | shell: {} | cwd: {}\n",
        ctx.os,
        if ctx.shell.trim().is_empty() {
            "unknown"
        } else {
            ctx.shell.trim()
        },
        ctx.cwd.as_deref().unwrap_or("unknown"),
    ));
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
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
    }

    #[test]
    fn suggestion_messages_embeds_structured_sections() {
        let msgs = suggestion_messages(&ctx_basic());
        let u = &msgs[1].content;
        assert!(u.contains("<env>"), "env section present");
        assert!(u.contains("cwd = /repo"), "cwd embedded");
        assert!(u.contains("<recent_commands>"), "recent commands present");
        assert!(u.contains("ok    $ git status"), "ok command marked");
        assert!(u.contains("FAILED(101) $ cargo build"), "failure marked");
        assert!(u.contains("<last_command_error>"), "error section present");
        assert!(u.contains("error[E0308]"), "error output embedded");
        assert!(u.contains("<current_input>\ncar\n"), "current line embedded");
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
        let u = &msgs[1].content;
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
    fn suggestion_messages_blank_shell_becomes_unknown() {
        let ctx = SuggestionContext {
            os: "macos".into(),
            shell: "   ".into(),
            current_line: "c".into(),
            ..SuggestionContext::default()
        };
        let msgs = suggestion_messages(&ctx);
        assert!(msgs[1].content.contains("shell = unknown"));
    }
}
