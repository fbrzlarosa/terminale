//! Shell profiles — named shell launch configurations (executable + args + env + cwd + icon).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// One launch profile.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Profile {
    /// Display name shown in pickers / tab titles (e.g. "PowerShell 7").
    pub name: String,
    /// Executable to launch (absolute path or PATH-resolvable name).
    pub command: String,
    /// CLI arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables set for the spawned shell.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory. `None` means inherit `terminale`'s cwd.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Optional one-glyph icon (Unicode) used in pickers and tabs.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Built-in shell profiles auto-detected at startup.
#[must_use]
pub fn auto_detect_profiles() -> Vec<Profile> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(path) = which("pwsh.exe") {
            out.push(Profile {
                name: "PowerShell 7".into(),
                command: path,
                args: vec!["-NoLogo".into()],
                env: HashMap::new(),
                cwd: None,
                icon: Some("⚡".into()),
            });
        }
        if let Some(path) = which("powershell.exe") {
            out.push(Profile {
                name: "Windows PowerShell".into(),
                command: path,
                args: vec!["-NoLogo".into()],
                env: HashMap::new(),
                cwd: None,
                icon: Some("\u{26A1}".into()), // ⚡ HIGH VOLTAGE SIGN (replaces U+F489 nf-md-terminal Nerd Font PUA, not bundled; ⚡ covered by NotoEmoji + emoji-icon-font, matches ICON_PRESETS 'PowerShell' in settings_window)
            });
        }
        if let Some(path) = which("cmd.exe") {
            out.push(Profile {
                name: "Command Prompt".into(),
                command: path,
                args: Vec::new(),
                env: HashMap::new(),
                cwd: None,
                icon: Some("📟".into()),
            });
        }
        for candidate in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ] {
            if std::path::Path::new(candidate).is_file() {
                out.push(Profile {
                    name: "Git Bash".into(),
                    command: candidate.into(),
                    args: vec!["--login".into(), "-i".into()],
                    env: HashMap::new(),
                    cwd: None,
                    icon: Some("🌿".into()),
                });
                break;
            }
        }
        if let Some(path) = which("wsl.exe") {
            out.push(Profile {
                name: "WSL".into(),
                command: path,
                args: Vec::new(),
                env: HashMap::new(),
                cwd: None,
                icon: Some("🐧".into()),
            });
        }
    } else {
        let candidates: &[(&str, &str)] = &[
            ("zsh", "Zsh"),
            ("bash", "Bash"),
            ("fish", "Fish"),
            ("nu", "Nushell"),
        ];
        // The user's `$SHELL` always wins as the first entry.
        if let Ok(shell) = std::env::var("SHELL") {
            if !shell.is_empty() {
                let pretty = shell.rsplit('/').next().unwrap_or(&shell).to_string();
                out.push(Profile {
                    name: pretty,
                    command: shell,
                    args: Vec::new(),
                    env: HashMap::new(),
                    cwd: None,
                    icon: Some("🐚".into()),
                });
            }
        }
        for (cmd, label) in candidates {
            if let Some(path) = which(cmd) {
                if out.iter().any(|p| p.command == path) {
                    continue;
                }
                out.push(Profile {
                    name: (*label).into(),
                    command: path,
                    args: Vec::new(),
                    env: HashMap::new(),
                    cwd: None,
                    icon: Some("🐚".into()),
                });
            }
        }
    }
    out
}

fn which(exe: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_at_least_one_profile() {
        let profiles = auto_detect_profiles();
        assert!(
            !profiles.is_empty(),
            "expected at least one shell profile on this host"
        );
    }
}
