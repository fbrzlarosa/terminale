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
    "You will be given recent terminal output and the user's current (possibly incomplete) input line. ",
    "Your job is to propose EXACTLY ONE next shell command the user most likely wants to run.\n",
    "\n",
    "Rules (follow strictly):\n",
    "- Reply with ONLY the command on a single line.\n",
    "- No explanation. No markdown. No backticks. No bullet points.\n",
    "- Do not repeat the prompt prefix (e.g. do not echo `$` or `PS >`).\n",
    "- If you cannot suggest anything useful, reply with an empty line and nothing else.\n",
    "- The output may include commands that FAILED (error messages). NEVER propose a command that just failed; ",
    "if the last command errored, propose a corrected version or a different, clearly useful command instead.\n",
    "- Do not invent file names, paths, or arguments that are not clearly present in the output. ",
    "When unsure, prefer a safe, broadly-useful command (list the current directory, check status) over guessing.\n",
    "- Use the syntax of the user's ACTUAL shell and OS (stated in the user message and visible in the output). ",
    "On Windows PowerShell use cmdlets such as Get-ChildItem / dir and NEVER `ls -l`; ",
    "on cmd.exe use dir; only use Unix commands (ls, grep, cat, ...) when the shell is clearly bash / zsh / sh.\n",
    "- Never mention any specific terminal-emulator product by name.",
);

/// Build the system + user messages that ask the model for ONE next shell command.
///
/// Returns a 2-element [`Vec`]:
/// 1. A `system` message containing the suggestion rules.
/// 2. A `user` message embedding the environment (`os`, `shell`),
///    `terminal_context`, and `current_line`.
///
/// `os` is typically [`std::env::consts::OS`] (`"windows"`, `"macos"`, `"linux"`)
/// and `shell` is the launching profile / shell name (e.g. `"PowerShell"`,
/// `"bash"`); both steer the model toward the correct command syntax so it does
/// not, say, propose `ls -l` at a PowerShell prompt.
///
/// The caller feeds these messages into [`crate::AiProvider::complete`].
#[must_use]
pub fn suggestion_messages(
    terminal_context: &str,
    current_line: &str,
    shell: &str,
    os: &str,
) -> Vec<AiMessage> {
    let shell = if shell.trim().is_empty() {
        "unknown"
    } else {
        shell.trim()
    };
    let user_text = format!(
        "Environment: OS = {os}; shell = {shell}.\n\
         Infer the shell from the prompt and any errors in the output below and match its exact \
         syntax (a `PS ...>` prompt or `Get-ChildItem` errors mean PowerShell — use \
         `Get-ChildItem` / `dir`, never `ls -l`).\n\n\
         Recent terminal output:\n{terminal_context}\n\nCurrent input line:\n{current_line}\n\nNext command:"
    );
    vec![AiMessage::system(SYSTEM_PROMPT), AiMessage::user(user_text)]
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
    use super::{extract_suggested_command, suggestion_messages, SYSTEM_PROMPT};

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

    #[test]
    fn suggestion_messages_count_and_roles() {
        let msgs = suggestion_messages("output here", "cur", "bash", "linux");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
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
    fn suggestion_messages_user_contains_context_line_and_env() {
        let ctx = "some terminal output";
        let line = "git s";
        let msgs = suggestion_messages(ctx, line, "PowerShell", "windows");
        let user_content = &msgs[1].content;
        assert!(
            user_content.contains(ctx),
            "user message must embed terminal_context"
        );
        assert!(
            user_content.contains(line),
            "user message must embed current_line"
        );
        assert!(
            user_content.contains("windows"),
            "user message must state the OS"
        );
        assert!(
            user_content.contains("PowerShell"),
            "user message must state the shell"
        );
    }

    #[test]
    fn suggestion_messages_blank_shell_becomes_unknown() {
        let msgs = suggestion_messages("o", "c", "   ", "macos");
        assert!(msgs[1].content.contains("shell = unknown"));
    }
}
