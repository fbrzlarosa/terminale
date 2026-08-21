//! Binding the Quake toggle to a key that the *desktop* owns.
//!
//! Under Wayland no application may grab a key globally, so terminale's own
//! hotkey never fires there. Two escape hatches exist, and this module holds
//! the shared plumbing for both:
//!
//! * the XDG global-shortcuts portal ([`crate::portal_shortcuts`]), which is
//!   automatic but needs the desktop to know terminale's *application id* —
//!   something a process only has when it was launched through its `.desktop`
//!   entry, not from a shell;
//! * a plain desktop keybinding that runs `terminale --toggle-quake`, which
//!   always works. [`register_gnome`] sets one up on GNOME with a single call
//!   so the user doesn't have to hand-build the command in system settings.
//!
//! Both need the user's `Ctrl+\`-style binding re-spelled in the syntax their
//! target expects, which is what [`xdg_trigger`] and [`gtk_accelerator`] do.

use std::process::Command;

/// dconf path of the custom keybinding terminale manages. Fixed (rather than
/// the `custom0`, `custom1`, … GNOME's own UI generates) so registering twice
/// updates one entry instead of piling up duplicates.
const GNOME_KEYBINDING_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/terminale-quake/";
/// Schema holding the list of custom keybinding paths.
const GNOME_MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
/// Per-keybinding schema, addressed with `schema:path`.
const GNOME_KEYBINDING_SCHEMA: &str =
    "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
/// Name shown in GNOME's keyboard settings.
const GNOME_KEYBINDING_NAME: &str = "terminale: toggle Quake drop-down";

// ── Key-syntax conversion ────────────────────────────────────────────────────

/// Split a terminale binding (`Ctrl+Shift+T`) into its modifiers and key.
/// Returns `None` when there is no non-modifier key, or the key has no keysym
/// name we can spell.
fn split_binding(binding: &str) -> Option<(Vec<Modifier>, String)> {
    let binding = binding.trim();
    if binding.is_empty() {
        return None;
    }
    let mut mods = Vec::new();
    let mut key = None;
    for raw in binding.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.push(Modifier::Ctrl),
            "shift" => mods.push(Modifier::Shift),
            "alt" | "option" => mods.push(Modifier::Alt),
            "cmd" | "super" | "meta" | "win" => mods.push(Modifier::Super),
            other => key = Some(xkb_keysym(other)?),
        }
    }
    Some((mods, key?))
}

/// The four modifiers both target syntaxes understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

/// Translate a terminale hotkey into the XDG "shortcuts" trigger syntax used by
/// `org.freedesktop.portal.GlobalShortcuts`: modifiers named
/// `CTRL`/`SHIFT`/`ALT`/`LOGO` followed by an XKB keysym, joined with `+`
/// (``Ctrl+\`` → `CTRL+backslash`).
pub(crate) fn xdg_trigger(binding: &str) -> Option<String> {
    let (mods, key) = split_binding(binding)?;
    let mut parts: Vec<&str> = mods
        .iter()
        .map(|m| match m {
            Modifier::Ctrl => "CTRL",
            Modifier::Shift => "SHIFT",
            Modifier::Alt => "ALT",
            Modifier::Super => "LOGO",
        })
        .collect();
    parts.push(&key);
    Some(parts.join("+"))
}

/// Translate a terminale hotkey into a GTK accelerator string, which is what
/// GNOME stores in a custom keybinding (``Ctrl+\`` → `<Control>backslash`).
pub(crate) fn gtk_accelerator(binding: &str) -> Option<String> {
    let (mods, key) = split_binding(binding)?;
    let mut out = String::new();
    for m in &mods {
        out.push_str(match m {
            Modifier::Ctrl => "<Control>",
            Modifier::Shift => "<Shift>",
            Modifier::Alt => "<Alt>",
            Modifier::Super => "<Super>",
        });
    }
    out.push_str(&key);
    Some(out)
}

/// Lower-case terminale key name → XKB keysym name. Both the XDG shortcut
/// syntax and GTK accelerators name the non-modifier key this way.
fn xkb_keysym(name: &str) -> Option<String> {
    // Single character: letters and digits are their own keysym names,
    // punctuation has a spelled-out one.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_lowercase().to_string());
        }
        return match c {
            '`' => Some("grave"),
            '-' => Some("minus"),
            '=' => Some("equal"),
            '[' => Some("bracketleft"),
            ']' => Some("bracketright"),
            '\\' => Some("backslash"),
            ';' => Some("semicolon"),
            '\'' => Some("apostrophe"),
            ',' => Some("comma"),
            '.' => Some("period"),
            '/' => Some("slash"),
            ' ' => Some("space"),
            _ => None,
        }
        .map(ToString::to_string);
    }
    // Function keys keep their number: F1 … F24.
    if let Some(digits) = name.strip_prefix(['f', 'F']) {
        if !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit())
            && digits.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))
        {
            return Some(format!("F{digits}"));
        }
    }
    let named = match name {
        "space" => "space",
        "enter" | "return" => "Return",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "Page_Up",
        "pagedown" | "pgdn" => "Page_Down",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        _ => return None,
    };
    Some(named.to_string())
}

// ── GNOME custom keybinding ──────────────────────────────────────────────────

/// The command a desktop keybinding should run to toggle the drop-down —
/// this executable's absolute path plus `--toggle-quake`.
pub(crate) fn toggle_command() -> String {
    let exe = std::env::current_exe()
        .map_or_else(|_| "terminale".to_string(), |p| p.display().to_string());
    format!("{exe} --toggle-quake")
}

/// Whether a GNOME-style custom-keybinding registration is possible here:
/// `gsettings` is on PATH and the media-keys schema is installed.
pub(crate) fn gnome_available() -> bool {
    gsettings(&["get", GNOME_MEDIA_KEYS_SCHEMA, "custom-keybindings"]).is_ok()
}

/// The accelerator currently bound to terminale's custom keybinding, if the
/// entry exists and is non-empty. `None` means "not registered".
pub(crate) fn gnome_current_binding() -> Option<String> {
    let schema = format!("{GNOME_KEYBINDING_SCHEMA}:{GNOME_KEYBINDING_PATH}");
    let value = gsettings(&["get", &schema, "binding"]).ok()?;
    let unquoted = unquote_gvariant_string(value.trim())?;
    if unquoted.is_empty() {
        return None;
    }
    // The entry must also still be listed, or GNOME ignores it entirely.
    let list = gsettings(&["get", GNOME_MEDIA_KEYS_SCHEMA, "custom-keybindings"]).ok()?;
    parse_gvariant_string_list(&list)
        .iter()
        .any(|p| p == GNOME_KEYBINDING_PATH)
        .then_some(unquoted)
}

/// Register (or update) a GNOME custom keybinding that runs
/// `terminale --toggle-quake` on `binding`.
///
/// This is the reliable Wayland path: unlike the global-shortcuts portal it
/// needs no application id, so it works however terminale was started. It only
/// ever touches terminale's own fixed dconf path, so re-registering updates the
/// single entry rather than accumulating duplicates, and [`unregister_gnome`]
/// removes exactly what was added.
///
/// # Errors
///
/// Returns a human-readable message when `gsettings` is missing, the GNOME
/// schemas aren't installed, or the binding can't be expressed as an
/// accelerator.
pub(crate) fn register_gnome(binding: &str) -> Result<String, String> {
    let accel = gtk_accelerator(binding)
        .ok_or_else(|| format!("`{binding}` can't be expressed as a desktop shortcut"))?;
    let command = toggle_command();

    // Add our path to the list of custom keybindings if it isn't there yet.
    let list_raw = gsettings(&["get", GNOME_MEDIA_KEYS_SCHEMA, "custom-keybindings"])
        .map_err(|e| format!("GNOME keyboard settings are not available: {e}"))?;
    let mut paths = parse_gvariant_string_list(&list_raw);
    if !paths.iter().any(|p| p == GNOME_KEYBINDING_PATH) {
        paths.push(GNOME_KEYBINDING_PATH.to_string());
        let encoded = format_gvariant_string_list(&paths);
        gsettings(&[
            "set",
            GNOME_MEDIA_KEYS_SCHEMA,
            "custom-keybindings",
            &encoded,
        ])
        .map_err(|e| format!("could not update the custom-keybindings list: {e}"))?;
    }

    let schema = format!("{GNOME_KEYBINDING_SCHEMA}:{GNOME_KEYBINDING_PATH}");
    for (key, value) in [
        ("name", GNOME_KEYBINDING_NAME),
        ("command", command.as_str()),
        ("binding", accel.as_str()),
    ] {
        gsettings(&["set", &schema, key, value])
            .map_err(|e| format!("could not set the shortcut's {key}: {e}"))?;
    }
    tracing::info!(binding = %accel, command = %command, "registered a GNOME custom keybinding");
    Ok(accel)
}

/// Remove the custom keybinding [`register_gnome`] created, leaving any other
/// custom keybindings the user has untouched.
///
/// # Errors
///
/// Returns a human-readable message when `gsettings` is unavailable or the
/// list could not be rewritten.
pub(crate) fn unregister_gnome() -> Result<(), String> {
    let list_raw = gsettings(&["get", GNOME_MEDIA_KEYS_SCHEMA, "custom-keybindings"])
        .map_err(|e| format!("GNOME keyboard settings are not available: {e}"))?;
    let paths: Vec<String> = parse_gvariant_string_list(&list_raw)
        .into_iter()
        .filter(|p| p != GNOME_KEYBINDING_PATH)
        .collect();
    let encoded = format_gvariant_string_list(&paths);
    gsettings(&[
        "set",
        GNOME_MEDIA_KEYS_SCHEMA,
        "custom-keybindings",
        &encoded,
    ])
    .map_err(|e| format!("could not update the custom-keybindings list: {e}"))?;
    // Clear the entry's own keys so a stale binding can't come back if the
    // path is ever re-listed by hand.
    let schema = format!("{GNOME_KEYBINDING_SCHEMA}:{GNOME_KEYBINDING_PATH}");
    for key in ["name", "command", "binding"] {
        let _ = gsettings(&["set", &schema, key, ""]);
    }
    tracing::info!("removed the GNOME custom keybinding");
    Ok(())
}

// ── Drop-down shell extensions ───────────────────────────────────────────────

/// UUID of the GNOME Shell extension terminale knows how to hand its drop-down
/// over to. It owns the hotkey, the geometry and the animation itself — the
/// three things a Wayland client is not allowed to do for itself.
const QUAKE_EXTENSION_UUID: &str = "quake-terminal@diegodario88.github.io";
/// Its settings schema, and the key naming the app it drives.
const QUAKE_EXTENSION_SCHEMA: &str = "org.gnome.shell.extensions.quake-terminal";
const QUAKE_EXTENSION_APP_KEY: &str = "terminal-id";
/// The key holding the extension's own hotkey, read only to show the user which
/// key they will be pressing.
const QUAKE_EXTENSION_SHORTCUT_KEY: &str = "terminal-shortcut";

/// Where the extension's compiled schema lives.
///
/// An extension's schema is installed inside the extension, not in the system
/// schema directory, so plain `gsettings` cannot see it — every call needs an
/// explicit `--schemadir`. Both install locations are probed: per-user
/// (`$XDG_DATA_HOME`, how the Extensions app installs) and system-wide.
fn quake_extension_schemadir() -> Option<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(std::path::PathBuf::from(data));
    }
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(std::path::PathBuf::from(home).join(".local/share"));
    }
    roots.push(std::path::PathBuf::from("/usr/share"));
    roots.push(std::path::PathBuf::from("/usr/local/share"));
    roots
        .into_iter()
        .map(|r| {
            r.join("gnome-shell/extensions")
                .join(QUAKE_EXTENSION_UUID)
                .join("schemas")
        })
        .find(|d| d.join("gschemas.compiled").is_file())
}

/// Whether the drop-down extension is installed and its settings reachable.
pub(crate) fn quake_extension_available() -> bool {
    quake_extension_schemadir().is_some()
}

/// The desktop-entry id the extension is currently set to drive, if readable.
pub(crate) fn quake_extension_app() -> Option<String> {
    let dir = quake_extension_schemadir()?;
    let raw = gsettings_in(
        &dir,
        &["get", QUAKE_EXTENSION_SCHEMA, QUAKE_EXTENSION_APP_KEY],
    )
    .ok()?;
    unquote_gvariant_string(raw.trim())
}

/// The key the extension will toggle the drop-down with, in GTK accelerator
/// spelling (`<Control>backslash`), if readable.
pub(crate) fn quake_extension_shortcut() -> Option<String> {
    let dir = quake_extension_schemadir()?;
    let raw = gsettings_in(
        &dir,
        &["get", QUAKE_EXTENSION_SCHEMA, QUAKE_EXTENSION_SHORTCUT_KEY],
    )
    .ok()?;
    parse_gvariant_string_list(&raw).into_iter().next()
}

/// Point the drop-down extension at `entry_id` (a `.desktop` file name).
///
/// This is the one call that takes the user's existing drop-down key — whatever
/// they already press, bound to whatever terminal they had — and makes it open
/// terminale instead. Only the one key is written; the extension's geometry,
/// animation and hotkey settings are left exactly as the user tuned them.
///
/// # Errors
///
/// Returns a message to show the user when the extension is not installed or
/// `gsettings` refuses the write.
pub(crate) fn point_quake_extension_at(entry_id: &str) -> Result<String, String> {
    let dir = quake_extension_schemadir().ok_or_else(|| {
        format!("the {QUAKE_EXTENSION_UUID} GNOME extension does not seem to be installed")
    })?;
    gsettings_in(
        &dir,
        &[
            "set",
            QUAKE_EXTENSION_SCHEMA,
            QUAKE_EXTENSION_APP_KEY,
            entry_id,
        ],
    )?;
    tracing::info!(entry_id, "pointed the drop-down extension at terminale");
    Ok(quake_extension_shortcut().unwrap_or_else(|| "its configured key".to_string()))
}

/// Run `gsettings` against a schema installed outside the system schema path.
fn gsettings_in(schemadir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("gsettings")
        .arg("--schemadir")
        .arg(schemadir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run `gsettings` with `args`, returning its stdout on success.
fn gsettings(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse the GVariant array-of-strings `gsettings get` prints, e.g.
/// `['/a/', '/b/']` or the empty-array form `@as []`.
fn parse_gvariant_string_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('\'') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('\'') else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

/// Encode a list of paths as a GVariant array-of-strings literal.
fn format_gvariant_string_list(paths: &[String]) -> String {
    if paths.is_empty() {
        return "@as []".to_string();
    }
    let inner: Vec<String> = paths.iter().map(|p| format!("'{p}'")).collect();
    format!("[{}]", inner.join(", "))
}

/// Strip the surrounding quotes from a GVariant string literal.
fn unquote_gvariant_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default Quake binding must survive translation into both syntaxes —
    /// this is the exact string most users will have in their config.
    #[test]
    fn translates_the_default_binding() {
        assert_eq!(xdg_trigger("Ctrl+\\").as_deref(), Some("CTRL+backslash"));
        assert_eq!(
            gtk_accelerator("Ctrl+\\").as_deref(),
            Some("<Control>backslash")
        );
    }

    #[test]
    fn translates_modifiers_and_letters() {
        assert_eq!(xdg_trigger("Ctrl+Shift+T").as_deref(), Some("CTRL+SHIFT+t"));
        assert_eq!(
            gtk_accelerator("Ctrl+Shift+T").as_deref(),
            Some("<Control><Shift>t")
        );
        assert_eq!(xdg_trigger("Alt+Space").as_deref(), Some("ALT+space"));
        assert_eq!(gtk_accelerator("Super+`").as_deref(), Some("<Super>grave"));
    }

    #[test]
    fn translates_named_and_function_keys() {
        assert_eq!(xdg_trigger("Ctrl+F12").as_deref(), Some("CTRL+F12"));
        assert_eq!(
            gtk_accelerator("Ctrl+PageUp").as_deref(),
            Some("<Control>Page_Up")
        );
        assert_eq!(xdg_trigger("Alt+Escape").as_deref(), Some("ALT+Escape"));
    }

    /// An unbindable / unknown key must yield `None` so callers fall back
    /// instead of writing a string the desktop would reject.
    #[test]
    fn unknown_keys_and_empty_bindings_yield_none() {
        assert_eq!(xdg_trigger(""), None);
        assert_eq!(gtk_accelerator("   "), None);
        assert_eq!(xdg_trigger("Ctrl+NotAKey"), None);
        assert_eq!(gtk_accelerator("Ctrl+F99"), None);
        // Modifiers alone are not a trigger.
        assert_eq!(xdg_trigger("Ctrl+Shift"), None);
    }

    // ── GVariant helpers ──────────────────────────────────────────────────────

    #[test]
    fn parses_the_empty_array_form() {
        assert!(parse_gvariant_string_list("@as []\n").is_empty());
        assert!(parse_gvariant_string_list("[]").is_empty());
    }

    #[test]
    fn parses_and_reformats_a_populated_list() {
        let raw = "['/org/gnome/a/', '/org/gnome/b/']\n";
        let parsed = parse_gvariant_string_list(raw);
        assert_eq!(parsed, vec!["/org/gnome/a/", "/org/gnome/b/"]);
        assert_eq!(
            format_gvariant_string_list(&parsed),
            "['/org/gnome/a/', '/org/gnome/b/']"
        );
    }

    /// Round-tripping an empty list must produce the typed empty-array literal:
    /// a bare `[]` has no type and `gsettings set` rejects it.
    #[test]
    fn empty_list_round_trips_as_typed_literal() {
        assert_eq!(format_gvariant_string_list(&[]), "@as []");
        assert!(parse_gvariant_string_list(&format_gvariant_string_list(&[])).is_empty());
    }

    /// Removing terminale's entry must preserve every other custom keybinding.
    #[test]
    fn removal_preserves_other_keybindings() {
        let raw =
            format!("['/org/gnome/custom0/', '{GNOME_KEYBINDING_PATH}', '/org/gnome/custom1/']");
        let remaining: Vec<String> = parse_gvariant_string_list(&raw)
            .into_iter()
            .filter(|p| p != GNOME_KEYBINDING_PATH)
            .collect();
        assert_eq!(
            remaining,
            vec!["/org/gnome/custom0/", "/org/gnome/custom1/"]
        );
    }

    #[test]
    fn unquotes_gvariant_strings() {
        assert_eq!(
            unquote_gvariant_string("'<Control>backslash'\n").as_deref(),
            Some("<Control>backslash")
        );
        assert_eq!(unquote_gvariant_string("''").as_deref(), Some(""));
        assert_eq!(unquote_gvariant_string("nonsense"), None);
    }

    /// The bound command must carry `--toggle-quake`, since that is the whole
    /// point of the registration.
    #[test]
    fn toggle_command_targets_this_binary() {
        assert!(toggle_command().ends_with(" --toggle-quake"));
    }
}
