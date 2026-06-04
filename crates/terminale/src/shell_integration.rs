//! Shell-integration injection: make recognised shells report their working
//! directory so it can be restored across sessions.
//!
//! # Why
//!
//! terminale learns a pane's working directory by sniffing `OSC 7` /
//! `OSC 9;9` escape sequences the shell prints (see `sniff_cwd` in
//! `terminale-term`). Most shells don't emit those out of the box. For shells
//! whose `cd` updates the OS process directory (cmd, bash, zsh) we can read
//! the cwd from the OS as a fallback — but **PowerShell** is special: its
//! `Set-Location` does *not* update the process directory, so the only way to
//! know where it is is to have it tell us.
//!
//! So when shell integration is enabled we inject a tiny startup hook that
//! wraps PowerShell's `prompt` function to emit `OSC 9;9;<path>` on every
//! prompt. terminale already understands that sequence, so the working
//! directory starts tracking with no further work — which makes
//! `window.restore_working_dirs` actually restore the folder for PowerShell
//! sessions.
//!
//! The hook is delivered via `-EncodedCommand` (base64 of UTF-16LE) so there
//! is zero command-line quoting to get wrong, plus `-NoExit` so the shell
//! stays interactive after running it.

/// If `command` is a shell we know how to instrument, return the argument
/// vector to launch it with cwd reporting injected; otherwise `None` (the
/// caller keeps the original args).
///
/// Injection is skipped when the profile already drives the shell with its
/// own command/script (`-Command`, `-File`, `-EncodedCommand`, …) so an
/// explicit launch is never hijacked.
#[must_use]
pub(crate) fn inject_cwd_reporting(command: &str, args: &[String]) -> Option<Vec<String>> {
    match shell_kind(command) {
        Some(ShellKind::PowerShell) => {
            if has_explicit_command(args) {
                return None;
            }
            let script = POWERSHELL_CWD_HOOK;
            let encoded = base64_utf16le(script);
            let mut out: Vec<String> = args.to_vec();
            out.push("-NoExit".to_string());
            out.push("-EncodedCommand".to_string());
            out.push(encoded);
            Some(out)
        }
        None => None,
    }
}

/// Shells terminale can instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// Windows PowerShell (`powershell.exe`) or PowerShell 7+ (`pwsh`).
    PowerShell,
}

/// Classify the executable by its file-stem, case-insensitively and ignoring
/// any directory and a trailing `.exe`.
fn shell_kind(command: &str) -> Option<ShellKind> {
    let stem = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .to_ascii_lowercase();
    match stem.as_str() {
        "powershell" | "pwsh" => Some(ShellKind::PowerShell),
        _ => None,
    }
}

/// True when the args already hand PowerShell something to run, in which case
/// we must not append our own `-EncodedCommand`. Matches the documented
/// switches and their unambiguous abbreviations.
fn has_explicit_command(args: &[String]) -> bool {
    args.iter().any(|a| {
        let t = a.trim_start_matches(['-', '/']).to_ascii_lowercase();
        matches!(
            t.as_str(),
            "command" | "c" | "file" | "f" | "encodedcommand" | "e" | "ec" | "enc"
        )
    })
}

/// PowerShell prompt hook: save the existing `prompt`, then redefine it to
/// emit `OSC 9;9;<filesystem path>` before delegating to the original (or the
/// default prompt string). One line so it survives as a single statement.
const POWERSHELL_CWD_HOOK: &str = "$global:__terminale_op=$function:prompt;function global:prompt{$p=$ExecutionContext.SessionState.Path.CurrentLocation.ProviderPath;$e=[char]27;[Console]::Write($e+']9;9;'+$p+$e+'\\');if($global:__terminale_op){& $global:__terminale_op}else{'PS '+$ExecutionContext.SessionState.Path.CurrentLocation.Path+'> '}}";

/// Encode `s` (ASCII) as base64 of its UTF-16LE bytes — the form PowerShell's
/// `-EncodedCommand` expects. Self-contained so we pull in no base64 crate for
/// this single use.
fn base64_utf16le(s: &str) -> String {
    // Widen ASCII → UTF-16LE (low byte then 0x00).
    let mut bytes = Vec::with_capacity(s.len() * 2);
    for &b in s.as_bytes() {
        bytes.push(b);
        bytes.push(0);
    }
    base64_standard(&bytes)
}

/// Standard base64 (RFC 4648, `+`/`/`, `=` padding).
fn base64_standard(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_powershell_variants() {
        assert_eq!(shell_kind("powershell.exe"), Some(ShellKind::PowerShell));
        assert_eq!(
            shell_kind(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(shell_kind("pwsh"), Some(ShellKind::PowerShell));
        assert_eq!(shell_kind("/usr/bin/pwsh"), Some(ShellKind::PowerShell));
        assert_eq!(shell_kind("cmd.exe"), None);
        assert_eq!(shell_kind("bash"), None);
    }

    #[test]
    fn injects_after_existing_args() {
        let args = vec!["-NoLogo".to_string()];
        let out = inject_cwd_reporting("powershell.exe", &args).unwrap();
        assert_eq!(out[0], "-NoLogo");
        assert_eq!(out[1], "-NoExit");
        assert_eq!(out[2], "-EncodedCommand");
        assert!(!out[3].is_empty());
        // The encoded payload is valid base64 (length multiple of 4).
        assert_eq!(out[3].len() % 4, 0);
    }

    #[test]
    fn skips_when_command_already_present() {
        for switch in ["-Command", "-c", "-File", "-EncodedCommand", "-e"] {
            let args = vec!["-NoLogo".to_string(), switch.to_string(), "x".to_string()];
            assert!(
                inject_cwd_reporting("powershell.exe", &args).is_none(),
                "should skip injection when {switch} is present"
            );
        }
    }

    #[test]
    fn non_powershell_is_untouched() {
        assert!(inject_cwd_reporting("cmd.exe", &[]).is_none());
        assert!(inject_cwd_reporting("/bin/bash", &["-i".to_string()]).is_none());
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors.
        assert_eq!(base64_standard(b""), "");
        assert_eq!(base64_standard(b"f"), "Zg==");
        assert_eq!(base64_standard(b"fo"), "Zm8=");
        assert_eq!(base64_standard(b"foo"), "Zm9v");
        assert_eq!(base64_standard(b"foob"), "Zm9vYg==");
        assert_eq!(base64_standard(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_standard(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn utf16le_encoding_widens_ascii() {
        // "Hi" → 48 00 69 00 → base64.
        assert_eq!(
            base64_utf16le("Hi"),
            base64_standard(&[0x48, 0x00, 0x69, 0x00])
        );
    }
}
