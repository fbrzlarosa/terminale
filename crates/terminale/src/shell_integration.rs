//! Shell-integration injection: get the shell to tell terminale what it is
//! doing — where it is, when a command starts, and how it ended.
//!
//! # Why
//!
//! A terminal emulator sees a byte stream. It cannot tell a prompt from output,
//! or know that a command failed, unless the shell says so. Two conventions
//! carry that information, and both need the shell's cooperation:
//!
//! * **`OSC 7` / `OSC 9;9`** — the current working directory (see `sniff_cwd` in
//!   `terminale-term`). This is what makes `window.restore_working_dirs` restore
//!   the folder, and what puts the directory in the tab title.
//! * **`OSC 133`** — semantic prompt marks: `A` before the prompt, `B` after it,
//!   `C` when a command starts, `D;<status>` when it ends. This is the
//!   foundation under command blocks, jump-to-next/previous-prompt,
//!   jump-to-failed-command, "fix this command", copy-last-command-output, and
//!   `terminale ctl last-command`.
//!
//! Almost no shell emits either out of the box. Historically this module only
//! instrumented PowerShell — which meant that on a default bash the whole
//! `OSC 133` half of the feature set was inert unless the user had already built
//! the marks into their own prompt. It now instruments bash too, which is what
//! makes those features work on a stock install.
//!
//! # How, per shell
//!
//! * **PowerShell** — a one-line startup hook delivered via `-EncodedCommand`
//!   (base64 of UTF-16LE, so there is no command-line quoting to get wrong) plus
//!   `-NoExit`. It wraps the `prompt` function to emit `OSC 9;9;<path>`.
//!   PowerShell needs this more than anyone: its `Set-Location` does not change
//!   the OS process directory, so there is no fallback way to learn its cwd.
//! * **bash** — `--rcfile <script>`, where the script is materialised on disk by
//!   [`ensure_script`]. Because `--rcfile` *replaces* `~/.bashrc`, the script
//!   sources the user's own rc first and installs its hooks last, leaving the
//!   shell exactly as configured plus the marks.
//!
//! zsh and fish are recognised but not yet instrumented — they need the
//! `ZDOTDIR` and `XDG_DATA_DIRS` dances respectively, and shipping either
//! untested risks breaking someone's shell startup, which is a far worse outcome
//! than a missing mark.
//!
//! # Safety rule for everything here
//!
//! Instrumentation is never load-bearing. Every injection is skipped when the
//! profile already drives the shell with its own command or rc file, every
//! failure to write a script degrades to launching the shell untouched, and the
//! scripts themselves are written so that an error costs a mark, not a session.

/// Instrument `args` for `command`, returning the argument vector to launch it
/// with; `None` leaves the caller's args alone.
///
/// Injection is skipped when the profile already drives the shell with its own
/// command or rc file (`-Command`, `-File`, `--rcfile`, `-c`, `--norc`, a login
/// shell, …), so an explicit launch is never hijacked.
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
        Some(ShellKind::Bash) => {
            if bash_drives_itself(args) {
                return None;
            }
            let script = ensure_script("terminale.bash", BASH_INTEGRATION)?;
            // Prepended, not appended: bash wants its options before any
            // operands, and a profile's args are arbitrary.
            let mut out = vec![
                "--rcfile".to_string(),
                script.to_string_lossy().into_owned(),
            ];
            out.extend_from_slice(args);
            Some(out)
        }
        None => None,
    }
}

/// True when bash's own arguments mean `--rcfile` would be wrong or ignored.
///
/// Four cases, all of which must be left alone:
/// * `-c` / `-s` — not an interactive prompt-driven shell at all;
/// * `--rcfile` / `--init-file` — the caller chose an rc file, and ours would
///   silently replace it;
/// * `--norc` / `--noprofile` — the caller asked for *no* rc file;
/// * `-l` / `--login` — a login shell reads the profile files and ignores
///   `--rcfile` entirely, so passing it would imply integration that never
///   loads.
fn bash_drives_itself(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "-c" | "-s" | "-l" | "--login" | "--rcfile" | "--init-file" | "--norc" | "--noprofile"
        ) ||
        // Bundled short flags, e.g. `-lc` or `-ic`.
        (a.starts_with('-')
            && !a.starts_with("--")
            && a.chars().skip(1).any(|c| matches!(c, 'c' | 's' | 'l')))
    })
}

/// Materialise `contents` at `<data dir>/shell-integration/<name>` and return
/// its path.
///
/// Written only when absent or different, so a shell launch normally costs one
/// `read` and no write. Returns `None` on any failure — the caller then launches
/// the shell uninstrumented, which is the whole point: a full disk must cost the
/// marks, not the session.
fn ensure_script(name: &str, contents: &str) -> Option<std::path::PathBuf> {
    let dir = terminale_config::paths::shell_integration_dir()?;
    let path = dir.join(name);
    // Compare first: rewriting on every launch would churn mtimes and, worse,
    // race two windows starting at once.
    if std::fs::read_to_string(&path).is_ok_and(|on_disk| on_disk == contents) {
        return Some(path);
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(?e, dir = %dir.display(), "could not create the shell-integration directory");
        return None;
    }
    // Write to a sibling and rename, so a shell starting concurrently either
    // sees the old complete file or the new one, never a half-written script.
    //
    // The scratch name has to be unique per *call*, not per process: keying it on
    // the pid alone made every thread share one temp file, so two concurrent
    // spawns could have one truncating it while the other renamed it into place —
    // leaving a reader with an empty script. (Found by the test suite doing
    // exactly that, in parallel, on one CI target.)
    static NEXT_SCRATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = NEXT_SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("{name}.{}.{scratch}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, contents) {
        tracing::warn!(?e, path = %tmp.display(), "could not write the shell-integration script");
        return None;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        tracing::warn!(?e, path = %path.display(), "could not install the shell-integration script");
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    tracing::info!(path = %path.display(), "installed shell integration");
    Some(path)
}

/// bash shell integration, loaded with `--rcfile`.
///
/// Kept as a literal rather than a separate asset file so the binary can always
/// re-materialise it: there is no install step to go missing, and a version
/// mismatch between binary and script is impossible.
///
/// The ordering rules that matter, since they are easy to get wrong:
/// * the user's `~/.bashrc` is sourced *first*, so our hooks end up last and are
///   not clobbered by a prompt framework;
/// * `PS1` is re-wrapped on every prompt, because tools like starship rewrite it
///   each time — and the wrap is skipped when the prompt already carries an `A`
///   mark, so a prompt tool that emits its own `OSC 133` is not double-marked;
/// * `C` is emitted from the `DEBUG` trap, but only when nothing else owns that
///   trap; where `bash-preexec` is installed we register with it instead.
const BASH_INTEGRATION: &str = r#"# terminale shell integration for bash — generated file, do not edit.
#
# Loaded with `bash --rcfile`, which REPLACES ~/.bashrc. So this file sources the
# user's own rc first and installs its hooks last, leaving the shell exactly as
# configured plus the marks terminale needs:
#
#   OSC 133 A/B/C/D  semantic prompt marks -> command blocks, exit codes,
#                    jump-to-failed-command, `terminale ctl last-command`
#   OSC 7            working directory     -> tab titles, restored cwd
#
# Everything below is guarded. A failure here must cost a mark, never a session.

# Nothing to mark in a non-interactive shell.
case $- in
  *i*) ;;
  *) return 0 ;;
esac

# Reproduce what bash would have read had --rcfile not been passed. (/etc/bash.bashrc
# is still read by bash itself, before this file.)
if [ -r "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc"
fi

# Install once per shell: a re-source would double every mark.
if [ -n "${__terminale_integration:-}" ]; then
  return 0
fi
__terminale_integration=1

# OSC 7. Only `%` and space need escaping for the URI to survive a percent-decode;
# doing it with parameter expansion keeps this off the per-prompt hot path.
__terminale_cwd() {
  local p=${PWD//\%/%25}
  p=${p// /%20}
  printf '\033]7;file://%s%s\a' "${HOSTNAME:-}" "$p"
}

# The command line about to run, read from history rather than $BASH_COMMAND:
# the DEBUG trap fires once per pipeline component, so $BASH_COMMAND would report
# `ls` for `ls | wc -l`. Sets a global instead of echoing, to avoid a subshell.
__terminale_read_command() {
  local h n rest
  __terminale_cmdline=
  h=$(HISTTIMEFORMAT='' builtin history 1 2>/dev/null)
  if [ -n "$h" ]; then
    # Format is "  <n>  <command>"; `read` drops the leading blanks for us.
    read -r n rest <<< "$h"
    __terminale_cmdline=$rest
  fi
  [ -z "$__terminale_cmdline" ] && __terminale_cmdline=${BASH_COMMAND:-}
}

# C — a command is about to run. The DEBUG trap fires before every simple
# command, including the ones bash runs inside PROMPT_COMMAND, so this guards
# against marking anything but the first command after a prompt.
#
# `OSC 633;E` reports the command line explicitly. Without it a terminal has to
# guess the command by reading the prompt line off the screen, which cannot tell
# the prompt apart from what was typed — so every captured command comes out as
# "[user@host ~]$ cargo test". `;` and `\` are escaped as the protocol requires.
__terminale_preexec() {
  [ -n "${COMP_LINE:-}" ] && return 0      # completion, not a command
  [ -n "${__terminale_in_precmd:-}" ] && return 0
  [ -n "${__terminale_ran:-}" ] && return 0
  __terminale_ran=1
  __terminale_read_command
  if [ -n "$__terminale_cmdline" ]; then
    local c=${__terminale_cmdline//\\/\\\\}
    c=${c//;/\\x3b}
    c=${c//$'\n'/\\x0a}
    printf '\033]633;E;%s\a' "$c"
  fi
  printf '\033]133;C\a'
}

# D — the previous command finished, with its status. Then A/B for the new prompt.
__terminale_precmd() {
  local status=$?
  __terminale_in_precmd=1
  # Nothing to close on the first prompt of the session, or after an empty line.
  if [ -n "${__terminale_ran:-}" ]; then
    printf '\033]133;D;%s\a' "$status"
  fi
  __terminale_ran=
  __terminale_cwd
  __terminale_wrap_ps1
  __terminale_in_precmd=
  return $status
}

# A before the prompt, B after it. Re-applied every prompt because prompt tools
# rewrite PS1; skipped when an A mark is already present, so a prompt that emits
# its own OSC 133 is left alone.
__terminale_wrap_ps1() {
  # Skip when the prompt already carries a prompt-start mark, in either the
  # OSC 133 spelling or VS Code's OSC 633 one — a user whose prompt tool already
  # emits marks must not get a second set.
  case "$PS1" in
    *'133;A'*|*'633;A'*) return 0 ;;
  esac
  PS1='\[\033]133;A\a\]'"$PS1"'\[\033]133;B\a\]'
}

__terminale_wrap_ps1

# PROMPT_COMMAND may be an array (bash 5.1+) or a string. Append, never replace.
case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in
  'declare -a'*)
    PROMPT_COMMAND+=(__terminale_precmd)
    ;;
  *)
    if [ -n "${PROMPT_COMMAND:-}" ]; then
      PROMPT_COMMAND="$PROMPT_COMMAND"$'\n'"__terminale_precmd"
    else
      PROMPT_COMMAND=__terminale_precmd
    fi
    ;;
esac

# Prefer bash-preexec if it is installed; otherwise take the DEBUG trap, but only
# when it is free. Stealing someone else's DEBUG trap would break their tooling,
# which is worse than losing the C mark.
if [ -n "${preexec_functions+x}" ]; then
  preexec_functions+=(__terminale_preexec)
elif [ -z "$(trap -p DEBUG)" ]; then
  trap '__terminale_preexec' DEBUG
fi

# Report the initial cwd so the tab title is right before the first command.
__terminale_cwd
"#;

/// Shells terminale can instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// Windows PowerShell (`powershell.exe`) or PowerShell 7+ (`pwsh`).
    PowerShell,
    /// GNU bash.
    Bash,
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
        "bash" => Some(ShellKind::Bash),
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
    }

    #[test]
    fn detects_bash() {
        assert_eq!(shell_kind("bash"), Some(ShellKind::Bash));
        assert_eq!(shell_kind("/bin/bash"), Some(ShellKind::Bash));
        assert_eq!(shell_kind("/usr/local/bin/bash"), Some(ShellKind::Bash));
        // `sh` may well *be* bash, but launched as `sh` it runs in POSIX mode
        // with different startup files — so it is deliberately not claimed.
        assert_eq!(shell_kind("/bin/sh"), None);
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
    fn uninstrumented_shells_are_untouched() {
        for shell in ["cmd.exe", "/bin/sh", "/usr/bin/zsh", "/usr/bin/fish"] {
            assert!(
                inject_cwd_reporting(shell, &[]).is_none(),
                "{shell} is not instrumented yet and must be left alone"
            );
        }
    }

    // ── bash ─────────────────────────────────────────────────────────────────

    /// A plain interactive bash gets `--rcfile <script>` prepended, and the
    /// script has to actually exist afterwards — the whole feature depends on
    /// bash finding it.
    #[test]
    fn bash_gets_an_rcfile_and_the_script_is_written() {
        let Some(out) = inject_cwd_reporting("/bin/bash", &[]) else {
            // No home directory (a bare CI container): nothing to assert.
            return;
        };
        assert_eq!(out[0], "--rcfile");
        let path = std::path::Path::new(&out[1]);
        assert!(
            path.is_file(),
            "script not materialised at {}",
            path.display()
        );
        let on_disk = std::fs::read_to_string(path).expect("read script");
        assert_eq!(on_disk, BASH_INTEGRATION);
    }

    /// Existing args must be preserved, after ours.
    #[test]
    fn bash_keeps_profile_args() {
        let Some(out) = inject_cwd_reporting("/bin/bash", &["-i".to_string()]) else {
            return;
        };
        assert_eq!(out[0], "--rcfile");
        assert_eq!(out[2], "-i");
    }

    /// Writing the script twice must be idempotent — the path is stable and no
    /// `.tmp` leftovers accumulate.
    #[test]
    fn ensure_script_is_idempotent() {
        let Some(first) = ensure_script("terminale-test.bash", "one\n") else {
            return;
        };
        let second = ensure_script("terminale-test.bash", "one\n").expect("second call");
        assert_eq!(first, second);
        // A changed body must be rewritten, not left stale.
        let third = ensure_script("terminale-test.bash", "two\n").expect("third call");
        assert_eq!(third, first);
        assert_eq!(std::fs::read_to_string(&third).expect("read"), "two\n");
        let _ = std::fs::remove_file(&third);
    }

    /// `ensure_script` is called once per pane spawn, so several calls can be in
    /// flight at once — and it must never hand back a path to a partially written
    /// file. This hammers it from eight threads and reads the result every time.
    ///
    /// Regression test: with the scratch file named after the pid alone, every
    /// thread shared one temp path, so one could truncate it while another renamed
    /// it into place and a reader would get an empty script. That surfaced as a
    /// single CI target failing while every other one passed.
    #[test]
    fn ensure_script_is_safe_under_concurrent_calls() {
        const BODY: &str = "concurrent body\n";
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..60 {
                        let Some(path) = ensure_script("terminale-race-test.bash", BODY) else {
                            return; // no home directory on this host
                        };
                        let got = std::fs::read_to_string(&path).unwrap_or_default();
                        assert_eq!(got, BODY, "a concurrent call exposed a partial script");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("worker thread panicked");
        }
        if let Some(dir) = terminale_config::paths::shell_integration_dir() {
            let _ = std::fs::remove_file(dir.join("terminale-race-test.bash"));
        }
    }

    /// The cases where `--rcfile` would be wrong: a shell already told what to
    /// run or which rc to read, and a login shell (which ignores `--rcfile`
    /// outright, so passing it would imply integration that never loads).
    #[test]
    fn bash_driving_itself_is_left_alone() {
        for args in [
            vec!["-c", "echo hi"],
            vec!["-lc", "echo hi"],
            vec!["-l"],
            vec!["--login"],
            vec!["--norc"],
            vec!["--noprofile"],
            vec!["--rcfile", "/tmp/other"],
            vec!["--init-file", "/tmp/other"],
            vec!["-s"],
        ] {
            let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            assert!(
                bash_drives_itself(&owned),
                "{args:?} must suppress injection"
            );
            assert!(inject_cwd_reporting("/bin/bash", &owned).is_none());
        }
        // …and the cases where it should still happen.
        for args in [vec![], vec!["-i"], vec!["--posix"]] {
            let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
            assert!(
                !bash_drives_itself(&owned),
                "{args:?} must not suppress injection"
            );
        }
    }

    /// The script's contract, asserted on the literal so a careless edit cannot
    /// quietly drop a mark. Each of these is load-bearing for a user-visible
    /// feature.
    #[test]
    fn bash_script_emits_every_mark() {
        for (needle, why) in [
            ("133;A", "prompt start — block boundaries"),
            ("133;B", "prompt end — where user input begins"),
            ("133;C", "command start — output boundary"),
            ("133;D;%s", "command end with exit status"),
            ("633;E;%s", "the command line, reported explicitly"),
            ("]7;file://", "cwd reporting"),
        ] {
            assert!(
                BASH_INTEGRATION.contains(needle),
                "the bash script must emit {needle} ({why})"
            );
        }
        // It must source the user's rc — `--rcfile` replaces it, so forgetting
        // this would silently wipe out the user's shell configuration.
        assert!(
            BASH_INTEGRATION.contains(". \"$HOME/.bashrc\""),
            "the script must source the user's own bashrc"
        );
        // And it must bail out of non-interactive shells.
        assert!(BASH_INTEGRATION.contains("case $- in"));
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
