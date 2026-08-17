//! OS-standard paths for `terminale`'s configuration, cache, and data
//! directories.

use directories::ProjectDirs;
use std::path::PathBuf;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "terminale";
const APPLICATION: &str = "terminale";

/// Path to the user config file (`config.toml`) under the OS-standard config
/// directory.
///
/// Returns `None` if the platform has no notion of a home directory (rare,
/// but possible on minimal CI environments).
#[must_use]
pub fn config_file() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|d| d.config_dir().join("config.toml"))
}

/// Directory where transient cache files (font atlases, etc.) live.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).map(|d| d.cache_dir().to_path_buf())
}

/// Directory where persistent app data (history, profiles, etc.) lives.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).map(|d| d.data_dir().to_path_buf())
}

/// Directory holding the shell-integration scripts terminale materialises on
/// disk and then asks a shell to load.
///
/// These are generated files, not user configuration — they live under the data
/// directory (rather than the config directory) precisely so that a user who
/// edits one gets it overwritten on the next launch instead of quietly diverging
/// from what the running binary expects.
#[must_use]
pub fn shell_integration_dir() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|d| d.data_dir().join("shell-integration"))
}

/// Default directory for user-installed Lua plugins.
#[must_use]
pub fn plugin_dir() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).map(|d| d.config_dir().join("plugins"))
}

/// Directory where named workspace files are stored (`workspaces/<name>.toml`).
#[must_use]
pub fn workspaces_dir() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|d| d.config_dir().join("workspaces"))
}

/// Default directory for drop-in theme files (`themes/<name>.toml`).
///
/// Each `.toml` file in this directory is loaded at startup as an additional
/// `Theme` and appended to the available-theme list after built-ins and any
/// inline `[[appearance.themes]]` entries. Files that fail to parse are
/// silently skipped (a warning is logged).
#[must_use]
pub fn themes_dir() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).map(|d| d.config_dir().join("themes"))
}

/// Path for the auto-saved last-session file.
#[must_use]
pub fn last_session_path() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|d| d.data_dir().join("last_session.toml"))
}

/// Path for the directory-jump frecency history file.
///
/// The file is written by `dir_jump::DirJumpStore` whenever a visit is
/// recorded and `persist` is enabled. Stored under the data directory so it
/// survives config resets without wiping the session history.
#[must_use]
pub fn dir_history_path() -> Option<PathBuf> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .map(|d| d.data_dir().join("dir_history.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_is_some_on_typical_host() {
        // Most CI runners and dev machines have a HOME; this can fail in
        // exotic sandboxes, which is fine.
        if let Some(p) = config_file() {
            assert!(p.ends_with("config.toml"));
        }
    }
}
