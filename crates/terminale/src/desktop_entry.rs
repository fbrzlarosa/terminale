//! Linux desktop-entry self-registration.
//!
//! On Windows the MSI registers Start-Menu / Desktop shortcuts and on macOS the
//! `.app` bundle lives in `/Applications`, so both are discoverable after
//! install. Linux ships as a plain tarball (or Homebrew), with no install-time
//! hook — so on launch we drop a `freedesktop` `.desktop` entry and the brand
//! icon under `$XDG_DATA_HOME` ourselves. That makes `terminale` show up in the
//! GNOME/KDE application menu and launcher search.
//!
//! Everything here is idempotent: files are only rewritten when their contents
//! change (e.g. the executable moved), so calling [`ensure_installed`] on every
//! launch is cheap.

use std::io;
use std::path::{Path, PathBuf};

/// The bundled brand SVG — the same source the runtime window icon uses.
const ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/icon.svg");
/// Icon "theme" name; referenced by `Icon=` in the desktop entry.
const ICON_NAME: &str = "terminale";
const DESKTOP_FILE: &str = "terminale.desktop";

/// `$XDG_DATA_HOME`, falling back to `$HOME/.local/share` per the XDG spec.
/// A relative `$XDG_DATA_HOME` is ignored (the spec requires an absolute path).
fn data_home() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p);
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
}

/// Render the `.desktop` file body for the given executable path.
fn desktop_contents(exec: &str) -> String {
    // Quote the Exec path so a binary installed under a path with spaces still
    // launches correctly (the desktop-entry spec honours double quotes).
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=terminale\n\
         GenericName=Terminal\n\
         Comment=A native, cross-platform, GPU-accelerated terminal\n\
         Exec=\"{exec}\"\n\
         Icon={ICON_NAME}\n\
         Terminal=false\n\
         Categories=System;TerminalEmulator;Utility;\n\
         Keywords=terminal;shell;console;command;\n\
         StartupNotify=true\n\
         StartupWMClass=terminale\n"
    )
}

/// Write `bytes` to `path` only if the file is missing or differs. Returns
/// whether anything was written.
fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == bytes {
            return Ok(false);
        }
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

/// Install (or refresh) the desktop entry and icon for the current executable.
///
/// Returns `Ok(true)` when something was written, `Ok(false)` when everything
/// was already up to date.
///
/// # Errors
///
/// Propagates filesystem errors. Returns `Ok(false)` if neither
/// `$XDG_DATA_HOME` nor `$HOME` is set (nowhere to install).
pub fn ensure_installed() -> io::Result<bool> {
    let Some(data) = data_home() else {
        return Ok(false);
    };
    let exec = std::env::current_exe()?.to_string_lossy().into_owned();

    let icon_dir = data.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&icon_dir)?;
    let mut changed = write_if_changed(&icon_dir.join(format!("{ICON_NAME}.svg")), ICON_SVG)?;

    let apps_dir = data.join("applications");
    std::fs::create_dir_all(&apps_dir)?;
    changed |= write_if_changed(
        &apps_dir.join(DESKTOP_FILE),
        desktop_contents(&exec).as_bytes(),
    )?;

    Ok(changed)
}

/// Remove the desktop entry and icon previously installed by
/// [`ensure_installed`]. Best-effort: missing files and IO errors are ignored.
pub fn remove() {
    if let Some(data) = data_home() {
        let _ = std::fs::remove_file(data.join("applications").join(DESKTOP_FILE));
        let _ = std::fs::remove_file(
            data.join("icons/hicolor/scalable/apps")
                .join(format!("{ICON_NAME}.svg")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_contents_has_required_keys() {
        let body = desktop_contents("/usr/bin/terminale");
        assert!(body.starts_with("[Desktop Entry]"));
        assert!(body.contains("Type=Application"));
        assert!(body.contains("Exec=\"/usr/bin/terminale\""));
        assert!(body.contains("Icon=terminale"));
        assert!(body.contains("TerminalEmulator"));
    }

    #[test]
    fn write_if_changed_is_idempotent() {
        let dir = std::env::temp_dir().join("terminale-desktop-test");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("probe.txt");
        let _ = std::fs::remove_file(&f);
        assert!(write_if_changed(&f, b"hello").expect("first write"));
        assert!(!write_if_changed(&f, b"hello").expect("second write")); // unchanged
        assert!(write_if_changed(&f, b"world").expect("third write")); // changed
        let _ = std::fs::remove_file(&f);
    }
}
