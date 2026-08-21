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

/// Application id worn by an instance launched from the drop-down entry.
///
/// Deliberately *not* [`crate::app_icon::DEFAULT_APP_ID`]: a shell extension
/// asked to toggle the drop-down finds the window by app id, and if that id
/// were shared with ordinary windows the extension would just as happily grab
/// the terminale you were working in. A dot rather than a dash keeps it out of
/// the way of `linux_window`'s `app-<id>-<pid>.scope` parsing, which splits on
/// dashes.
pub const QUAKE_APP_ID: &str = "terminale.Quake";
/// A desktop entry's *id* is its basename, and that is what a shell extension
/// stores as the app to launch — so the file name is part of the contract.
const QUAKE_DESKTOP_FILE: &str = "terminale.Quake.desktop";
/// Autostart entries live under their own directory, one file per application,
/// and are keyed by file name — so this one shares the application's name.
const AUTOSTART_FILE: &str = "terminale.desktop";

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

/// Render the `.desktop` body for the drop-down launcher.
///
/// This entry exists so a GNOME/KDE shell extension that implements a drop-down
/// terminal itself — Quake Terminal being the common one on GNOME — can *start*
/// terminale on the first keypress and toggle it from then on. Two things differ
/// from the application-menu entry, both load-bearing:
///
/// * `--class` and `StartupWMClass` are set to [`QUAKE_APP_ID`], which is how
///   the extension recognises the window it launched. GNOME resolves a window
///   to a `.desktop` entry through this id; get it wrong and the extension
///   reports "launched but no windows" and gives up.
/// * No `--quake`. When an extension owns the drop-down it owns the geometry,
///   the always-on-top and the show/hide animation, so terminale must come up
///   as an ordinary window and stay out of the way. Docking and animating it
///   ourselves as well is what makes the drop-down look like it is fighting the
///   desktop, because it is.
///
/// The entry stays visible in the application list on purpose (no `NoDisplay`):
/// the extension's app picker filters on `should_show()` and on a `Categories`
/// value containing "terminal", so hiding it would take it out of the very
/// picker the user has to choose it from.
fn quake_desktop_contents(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=terminale (drop-down)\n\
         GenericName=Terminal\n\
         Comment=Drop-down terminale window driven by a Quake-mode shell extension\n\
         Exec=\"{exec}\" --class={QUAKE_APP_ID}\n\
         Icon={ICON_NAME}\n\
         Terminal=false\n\
         Categories=System;TerminalEmulator;\n\
         Keywords=terminal;quake;dropdown;\n\
         StartupNotify=true\n\
         StartupWMClass={QUAKE_APP_ID}\n"
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
    // Not `current_exe()`: on Linux that carries a `(deleted)` marker once the
    // binary has been replaced under this process, and writing it into `Exec=`
    // produces a launcher that cannot launch anything. See `update::running_exe`.
    let exec = crate::update::running_exe()?.to_string_lossy().into_owned();

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

/// `$XDG_CONFIG_HOME`, falling back to `$HOME/.config` per the XDG spec.
/// A relative `$XDG_CONFIG_HOME` is ignored (the spec requires an absolute path).
fn config_home() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p);
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

/// Render the autostart entry's body.
///
/// It launches terminale `--start-hidden`, which is the whole point: a
/// drop-down that has to start an application on the first keypress cannot feel
/// instant, however fast the application is. With the process, the GPU surface
/// and the shell already up and only the window unmapped, the first press is a
/// reveal like every press after it.
///
/// `X-GNOME-Autostart-enabled=true` is what GNOME's own tooling writes and
/// what it flips when a user disables an entry from the Tweaks UI, so honouring
/// the key keeps that switch meaningful. `NoDisplay=true` keeps this entry out
/// of the application menu — the menu already has one, and this file exists to
/// be read by the session, not clicked.
fn autostart_contents(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=terminale\n\
         Comment=Start terminale hidden, ready for the drop-down hotkey\n\
         Exec=\"{exec}\" --start-hidden\n\
         Icon={ICON_NAME}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// Path the autostart entry is installed at, if there is anywhere to put it.
fn autostart_path() -> Option<PathBuf> {
    config_home().map(|c| c.join("autostart").join(AUTOSTART_FILE))
}

/// Whether terminale is set to start hidden at login.
#[must_use]
pub fn autostart_installed() -> bool {
    autostart_path().is_some_and(|p| p.is_file())
}

/// Install (or refresh) the autostart entry for the current executable.
///
/// Returns `Ok(true)` when the file was written, `Ok(false)` when it was
/// already up to date.
///
/// # Errors
///
/// Propagates filesystem errors. Returns `Ok(false)` if neither
/// `$XDG_CONFIG_HOME` nor `$HOME` is set (nowhere to install).
pub fn ensure_autostart() -> io::Result<bool> {
    let Some(path) = autostart_path() else {
        return Ok(false);
    };
    // Not `current_exe()`: on Linux that carries a `(deleted)` marker once the
    // binary has been replaced under this process, and writing it into `Exec=`
    // produces a launcher that cannot launch anything. See `update::running_exe`.
    let exec = crate::update::running_exe()?.to_string_lossy().into_owned();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_if_changed(&path, autostart_contents(&exec).as_bytes())
}

/// Remove the autostart entry. Best-effort.
pub fn remove_autostart() {
    if let Some(p) = autostart_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// Path the drop-down launcher is installed at, if there is anywhere to put it.
fn quake_launcher_path() -> Option<PathBuf> {
    data_home().map(|d| d.join("applications").join(QUAKE_DESKTOP_FILE))
}

/// Whether the drop-down launcher entry is currently installed.
#[must_use]
pub fn quake_launcher_installed() -> bool {
    quake_launcher_path().is_some_and(|p| p.is_file())
}

/// The desktop-entry id a shell extension should be pointed at to drive
/// terminale's drop-down — i.e. what goes in Quake Terminal's `terminal-id`.
#[must_use]
pub fn quake_launcher_id() -> &'static str {
    QUAKE_DESKTOP_FILE
}

/// Install (or refresh) the drop-down launcher entry for the current
/// executable. See [`quake_desktop_contents`] for what it is and why.
///
/// Returns `Ok(true)` when the file was written, `Ok(false)` when it was
/// already up to date.
///
/// # Errors
///
/// Propagates filesystem errors. Returns `Ok(false)` if neither
/// `$XDG_DATA_HOME` nor `$HOME` is set (nowhere to install).
pub fn ensure_quake_launcher() -> io::Result<bool> {
    let Some(data) = data_home() else {
        return Ok(false);
    };
    // Not `current_exe()`: on Linux that carries a `(deleted)` marker once the
    // binary has been replaced under this process, and writing it into `Exec=`
    // produces a launcher that cannot launch anything. See `update::running_exe`.
    let exec = crate::update::running_exe()?.to_string_lossy().into_owned();
    let apps_dir = data.join("applications");
    std::fs::create_dir_all(&apps_dir)?;
    write_if_changed(
        &apps_dir.join(QUAKE_DESKTOP_FILE),
        quake_desktop_contents(&exec).as_bytes(),
    )
}

/// Remove the drop-down launcher entry. Best-effort.
pub fn remove_quake_launcher() {
    if let Some(p) = quake_launcher_path() {
        let _ = std::fs::remove_file(p);
    }
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
    fn quake_contents_carry_what_the_extension_matches_on() {
        let body = quake_desktop_contents("/usr/bin/terminale");
        assert!(body.starts_with("[Desktop Entry]"));
        // The extension launches the entry, then looks for a window whose app
        // id is this one. Both halves have to agree, or it waits forever.
        assert!(body.contains(&format!("--class={QUAKE_APP_ID}")));
        assert!(body.contains(&format!("StartupWMClass={QUAKE_APP_ID}")));
        // The extension owns the geometry and the animation for this window,
        // so terminale must not also dock and animate it.
        assert!(!body.contains("--quake"));
        // Its app picker filters on `should_show()` and a "terminal" category.
        assert!(body.contains("TerminalEmulator"));
        assert!(!body.contains("NoDisplay"));
        assert!(!body.contains("Hidden=true"));
    }

    #[test]
    fn quake_launcher_wears_a_different_identity_than_ordinary_windows() {
        // The whole point of the separate entry: an extension told to toggle
        // the drop-down must not be able to match a window you were working in.
        assert_ne!(QUAKE_APP_ID, crate::app_icon::DEFAULT_APP_ID);
        // `linux_window` parses `app-<id>-<pid>.scope` by splitting on dashes,
        // so the id must not contain one.
        assert!(!QUAKE_APP_ID.contains('-'));
        // A desktop entry's id is its basename; the extension stores it verbatim.
        assert_eq!(quake_launcher_id(), QUAKE_DESKTOP_FILE);
        assert!(QUAKE_DESKTOP_FILE.ends_with(".desktop"));
    }

    #[test]
    fn autostart_entry_starts_hidden_and_stays_out_of_the_menu() {
        let body = autostart_contents("/usr/bin/terminale");
        assert!(body.starts_with("[Desktop Entry]"));
        // The whole reason the entry exists: warm, not visible.
        assert!(body.contains("--start-hidden"));
        // The application menu already has an entry; this one is for the
        // session to read, not for anyone to click.
        assert!(body.contains("NoDisplay=true"));
        // Honour the switch GNOME's own tooling flips.
        assert!(body.contains("X-GNOME-Autostart-enabled=true"));
        // An autostart entry that opened a drop-down would defeat itself.
        assert!(!body.contains("--quake"));
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
