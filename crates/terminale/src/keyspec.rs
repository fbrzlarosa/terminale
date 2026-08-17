//! Text key specs → PTY bytes, for the control API's `send-keys`.
//!
//! A caller writes what it means — `"ctrl+c"`, `"escape"`, `"shift+enter"`,
//! `"down down enter"` — and this module turns it into exactly the bytes the
//! focused program would have received had a human pressed those keys.
//!
//! "Exactly" is the whole point, so nothing here re-implements an encoding.
//! A spec is parsed into the same winit shapes a real key event carries
//! ([`ModifiersState`], [`PhysicalKey`], [`Key`]) and then handed to the
//! encoders the live keyboard path already uses:
//!
//! 1. [`crate::kitty_keyboard::encode_key`] when the program in the pane has
//!    engaged the kitty keyboard protocol — which is what makes an automated
//!    `shift+enter` land as `CSI 13;2u` in a TUI that asked for it, rather than
//!    as a bare `\r` a program cannot tell from `Enter`;
//! 2. [`crate::shortcuts::translate_key`] otherwise, the legacy xterm encoder
//!    (application cursor-key mode, `CSI 1;<mod><letter>`, C0 controls, the
//!    `ESC`-prefix meta form).
//!
//! Consequence worth stating: `send-keys` inherits every future fix to those
//! two paths for free, and can never drift from what typing does.
//!
//! # Grammar
//!
//! Whitespace separates key presses; `+` separates modifiers from the key.
//!
//! ```text
//! ctrl+c              one press
//! down down enter     three presses, in order
//! ctrl+shift+p        modifiers in any order
//! a                   a literal character
//! plus                the `+` character (which `+` itself cannot spell)
//! ```
//!
//! Modifier aliases: `ctrl`/`control`/`^`, `alt`/`opt`/`option`/`meta`,
//! `shift`, `super`/`cmd`/`command`/`win`. Key names are case-insensitive.

use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey, SmolStr};

/// One parsed key press.
struct KeyPress {
    /// Modifiers held during the press.
    mods: ModifiersState,
    /// Physical key, where the spec names one. Needed because the C0 control
    /// mapping (`ctrl+a` → `0x01`) is keyed on the physical code, exactly as it
    /// is for real events.
    physical: PhysicalKey,
    /// Logical key.
    logical: Key,
    /// Text the press would insert, for the paths that fall back to it.
    text: Option<SmolStr>,
}

/// Whether any press in `keys` would submit a line — i.e. carries `Enter` in
/// one of its spellings.
///
/// The control API's permission gate calls this *before* encoding, so a caller
/// without `allow_submit` cannot smuggle a command execution past it. It
/// deliberately errs toward "yes": `ctrl+m` and `ctrl+j` are `\r` and `\n` on
/// the wire, so they count.
pub(crate) fn any_submits(keys: &str) -> bool {
    keys.split_whitespace().any(|spec| {
        let (mods, key) = split_spec(spec);
        let ctrl = mods.iter().any(|m| {
            ["ctrl", "control", "^"]
                .iter()
                .any(|c| m.eq_ignore_ascii_case(c))
        });
        let key = key.to_ascii_lowercase();
        matches!(
            key.as_str(),
            "enter" | "return" | "cr" | "kpenter" | "kp_enter"
        ) || (ctrl && matches!(key.as_str(), "m" | "j"))
    })
}

/// Encode `keys` for a pane in the given modes.
///
/// `app_cursor` is DECCKM (application cursor keys) and `kitty` is the flag set
/// the focused program pushed via the kitty keyboard protocol — both read from
/// that pane's emulator, so the bytes match what it is currently expecting.
///
/// # Errors
///
/// Returns a message naming the offending token when a spec is empty, names an
/// unknown modifier or key, or encodes to nothing.
pub(crate) fn encode_keys(
    keys: &str,
    app_cursor: bool,
    kitty: terminale_term::KittyKeyboardFlags,
) -> Result<Vec<u8>, String> {
    let specs: Vec<&str> = keys.split_whitespace().collect();
    if specs.is_empty() {
        return Err("no keys given".into());
    }
    let mut out = Vec::new();
    for spec in specs {
        let press = parse_press(spec)?;
        out.extend_from_slice(&encode_press(&press, app_cursor, kitty, spec)?);
    }
    Ok(out)
}

/// Split `spec` into its modifier tokens and its final key token.
///
/// `+` is both the separator and a legitimate key, so a trailing `+` (as in
/// `ctrl++`) is read as the key rather than an empty token; the `plus` alias
/// exists for the unambiguous spelling.
fn split_spec(spec: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = spec.split('+').collect();
    // `ctrl++` splits to ["ctrl", "", ""] — the intended key is `+`.
    if parts.len() >= 2 && parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
        if parts.last().is_some_and(|p| p.is_empty()) {
            let n = parts.len();
            parts[n - 1] = "+";
        }
    }
    let key = parts.pop().unwrap_or("");
    (parts, key)
}

/// Parse one whitespace-delimited spec into a press.
fn parse_press(spec: &str) -> Result<KeyPress, String> {
    let (mod_tokens, key_token) = split_spec(spec);
    if key_token.is_empty() {
        return Err(format!("`{spec}`: no key named"));
    }

    let mut mods = ModifiersState::empty();
    for m in mod_tokens {
        mods |= match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "^" => ModifiersState::CONTROL,
            "alt" | "opt" | "option" | "meta" => ModifiersState::ALT,
            "shift" => ModifiersState::SHIFT,
            "super" | "cmd" | "command" | "win" => ModifiersState::SUPER,
            other => {
                return Err(format!(
                    "`{spec}`: unknown modifier `{other}` \
                     (use ctrl, alt, shift or super)"
                ))
            }
        };
    }

    let lower = key_token.to_ascii_lowercase();
    if let Some(named) = named_key(&lower) {
        return Ok(KeyPress {
            mods,
            physical: named_physical(named),
            logical: Key::Named(named),
            text: None,
        });
    }

    // A literal character. Shift uppercases it, matching what the keyboard
    // would have produced, so `shift+a` types `A` rather than a bare `a` the
    // program has to guess about.
    let c = if let Some(c) = char_alias(&lower) {
        c
    } else {
        let mut chars = key_token.chars();
        let c = chars
            .next()
            .ok_or_else(|| format!("`{spec}`: no key named"))?;
        if chars.next().is_some() {
            return Err(format!(
                "`{spec}`: unknown key `{key_token}` \
                 (a key name, or a single character)"
            ));
        }
        c
    };
    let c = if mods.shift_key() {
        c.to_ascii_uppercase()
    } else {
        c
    };
    let text = SmolStr::new(c.to_string());
    Ok(KeyPress {
        mods,
        physical: char_physical(c),
        logical: Key::Character(text.clone()),
        text: Some(text),
    })
}

/// Encode one press through the same two encoders the live key path uses.
fn encode_press(
    press: &KeyPress,
    app_cursor: bool,
    kitty: terminale_term::KittyKeyboardFlags,
    spec: &str,
) -> Result<Vec<u8>, String> {
    if kitty.any() {
        match crate::kitty_keyboard::encode_key(
            kitty,
            &press.mods,
            press.physical,
            &press.logical,
            press.text.as_deref(),
            crate::kitty_keyboard::KeyPhase::Press,
        ) {
            crate::kitty_keyboard::KittyOutcome::Bytes(b) => return Ok(b),
            // The protocol is engaged but does not encode this key (a plain
            // text key under disambiguate-only mode): fall through to legacy,
            // exactly as the live path does.
            crate::kitty_keyboard::KittyOutcome::Legacy => {}
            crate::kitty_keyboard::KittyOutcome::Ignore => return Ok(Vec::new()),
        }
    }
    crate::shortcuts::translate_key(
        &press.mods,
        press.physical,
        &press.logical,
        press.text.clone(),
        app_cursor,
    )
    .ok_or_else(|| format!("`{spec}`: this key combination sends nothing to the terminal"))
}

/// Map a lower-cased key name to its winit named key.
///
/// Aliases follow what other terminals' `send-keys` accept (tmux, kitty,
/// wezterm), so a script written against one of those mostly works here.
fn named_key(lower: &str) -> Option<NamedKey> {
    Some(match lower {
        "enter" | "return" | "cr" | "kpenter" | "kp_enter" => NamedKey::Enter,
        "tab" => NamedKey::Tab,
        "backspace" | "bs" => NamedKey::Backspace,
        "escape" | "esc" => NamedKey::Escape,
        "space" => NamedKey::Space,
        "up" | "arrowup" => NamedKey::ArrowUp,
        "down" | "arrowdown" => NamedKey::ArrowDown,
        "left" | "arrowleft" => NamedKey::ArrowLeft,
        "right" | "arrowright" => NamedKey::ArrowRight,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" | "pgup" | "prior" => NamedKey::PageUp,
        "pagedown" | "pgdn" | "next" => NamedKey::PageDown,
        "insert" | "ins" => NamedKey::Insert,
        "delete" | "del" => NamedKey::Delete,
        "f1" => NamedKey::F1,
        "f2" => NamedKey::F2,
        "f3" => NamedKey::F3,
        "f4" => NamedKey::F4,
        "f5" => NamedKey::F5,
        "f6" => NamedKey::F6,
        "f7" => NamedKey::F7,
        "f8" => NamedKey::F8,
        "f9" => NamedKey::F9,
        "f10" => NamedKey::F10,
        "f11" => NamedKey::F11,
        "f12" => NamedKey::F12,
        _ => return None,
    })
}

/// Spelled-out names for punctuation keys.
///
/// `+` cannot name itself (it is the modifier separator) and the rest are here
/// so a spec never has to embed a character that a shell would want to quote.
fn char_alias(lower: &str) -> Option<char> {
    Some(match lower {
        "plus" => '+',
        "minus" | "hyphen" => '-',
        "equal" | "equals" => '=',
        "comma" => ',',
        "period" | "dot" => '.',
        "slash" => '/',
        "backslash" => '\\',
        "semicolon" => ';',
        "colon" => ':',
        "quote" | "apostrophe" => '\'',
        "backquote" | "grave" | "tilde" => '`',
        "bracketleft" | "lbracket" => '[',
        "bracketright" | "rbracket" => ']',
        "underscore" => '_',
        "question" => '?',
        _ => return None,
    })
}

/// Physical code for a named key, where the encoders care about one.
///
/// Only the keys whose encoding consults the physical code need a real value;
/// the rest report [`PhysicalKey::Unidentified`], which the encoders treat as
/// "no C0 mapping" — correct, since none of them have one.
fn named_physical(named: NamedKey) -> PhysicalKey {
    let code = match named {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Tab => KeyCode::Tab,
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Escape => KeyCode::Escape,
        NamedKey::Space => KeyCode::Space,
        _ => {
            return PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        }
    };
    PhysicalKey::Code(code)
}

/// Physical code for a literal character on a US layout.
///
/// This is what makes `ctrl+a` reach [`crate::shortcuts::ctrl_code_for`] and
/// come out as `0x01`. Characters outside the mapped set report
/// `Unidentified`, which correctly yields no C0 byte.
fn char_physical(c: char) -> PhysicalKey {
    let code = match c.to_ascii_lowercase() {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        '[' => KeyCode::BracketLeft,
        ']' => KeyCode::BracketRight,
        '\\' => KeyCode::Backslash,
        '/' => KeyCode::Slash,
        '-' => KeyCode::Minus,
        '=' => KeyCode::Equal,
        ';' => KeyCode::Semicolon,
        '\'' => KeyCode::Quote,
        ',' => KeyCode::Comma,
        '.' => KeyCode::Period,
        '`' => KeyCode::Backquote,
        _ => {
            return PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        }
    };
    PhysicalKey::Code(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminale_term::KittyKeyboardFlags;

    /// Flags for "no program has engaged the protocol" — the common case.
    fn no_kitty() -> KittyKeyboardFlags {
        KittyKeyboardFlags::default()
    }

    fn enc(keys: &str) -> Vec<u8> {
        encode_keys(keys, false, no_kitty()).expect("encodes")
    }

    // ── the C0 controls an automation client most needs ──────────────────────

    #[test]
    fn ctrl_letters_are_c0_controls() {
        assert_eq!(enc("ctrl+c"), vec![0x03]);
        assert_eq!(enc("ctrl+d"), vec![0x04]);
        assert_eq!(enc("ctrl+z"), vec![0x1a]);
        assert_eq!(enc("ctrl+a"), vec![0x01]);
        // Case-insensitive, and the modifier order is free.
        assert_eq!(enc("CTRL+C"), vec![0x03]);
        assert_eq!(enc("control+c"), vec![0x03]);
        assert_eq!(enc("^+c"), vec![0x03]);
    }

    #[test]
    fn named_keys_get_their_legacy_bytes() {
        assert_eq!(enc("enter"), b"\r".to_vec());
        assert_eq!(enc("return"), b"\r".to_vec());
        assert_eq!(enc("tab"), b"\t".to_vec());
        assert_eq!(enc("escape"), vec![0x1b]);
        assert_eq!(enc("esc"), vec![0x1b]);
        assert_eq!(enc("backspace"), vec![0x7f]);
    }

    /// DECCKM is the difference between arrows a full-screen program
    /// understands and arrows it does not, so `send-keys` must honour it.
    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(enc("up"), b"\x1b[A".to_vec());
        assert_eq!(enc("down"), b"\x1b[B".to_vec());
        assert_eq!(enc("right"), b"\x1b[C".to_vec());
        assert_eq!(enc("left"), b"\x1b[D".to_vec());
        let app = |k: &str| encode_keys(k, true, no_kitty()).expect("encodes");
        assert_eq!(app("up"), b"\x1bOA".to_vec());
        assert_eq!(app("down"), b"\x1bOB".to_vec());
        assert_eq!(app("home"), b"\x1bOH".to_vec());
    }

    #[test]
    fn modified_specials_use_the_xterm_csi_form() {
        // 1 + shift(1) + alt(2) + ctrl(4) → ctrl+left = 1;5D
        assert_eq!(enc("ctrl+left"), b"\x1b[1;5D".to_vec());
        assert_eq!(enc("shift+up"), b"\x1b[1;2A".to_vec());
        assert_eq!(enc("alt+right"), b"\x1b[1;3C".to_vec());
        // Shift+Tab is back-tab, which readline and every TUI expects.
        assert_eq!(enc("shift+tab"), b"\x1b[Z".to_vec());
    }

    #[test]
    fn alt_prefixes_esc_for_characters() {
        assert_eq!(enc("alt+b"), vec![0x1b, b'b']);
        assert_eq!(enc("alt+f"), vec![0x1b, b'f']);
    }

    #[test]
    fn plain_and_shifted_characters() {
        assert_eq!(enc("a"), b"a".to_vec());
        assert_eq!(enc("shift+a"), b"A".to_vec());
        assert_eq!(enc("space"), b" ".to_vec());
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc("f1"), b"\x1bOP".to_vec());
        assert_eq!(enc("f5"), b"\x1b[15~".to_vec());
        assert_eq!(enc("f12"), b"\x1b[24~".to_vec());
    }

    #[test]
    fn several_presses_concatenate_in_order() {
        assert_eq!(enc("down down enter"), b"\x1b[B\x1b[B\r".to_vec());
        assert_eq!(enc("y enter"), b"y\r".to_vec());
    }

    /// `+` is the separator, so it needs an escape hatch as a key — and the two
    /// spellings must behave identically, whatever that behaviour is.
    #[test]
    fn the_plus_character_is_reachable() {
        assert_eq!(enc("plus"), b"+".to_vec());
        assert_eq!(enc("+"), b"+".to_vec());
        assert_eq!(enc("shift+plus"), b"+".to_vec());
        // Ctrl+Plus has no C0 mapping, so a real terminal sends nothing rather
        // than echoing a literal `+`. Both spellings must agree on that.
        let a = encode_keys("ctrl++", false, no_kitty());
        let b = encode_keys("ctrl+plus", false, no_kitty());
        assert_eq!(a.is_err(), b.is_err());
        assert!(a.is_err(), "ctrl+plus sends nothing: {a:?}");
    }

    // ── the kitty path ───────────────────────────────────────────────────────

    /// The reason this module exists at all: with the protocol engaged,
    /// `shift+enter` must be distinguishable from `enter` — that is what lets
    /// an automated newline reach a TUI like Claude Code as a newline instead
    /// of as a submit.
    #[test]
    fn shift_enter_is_disambiguated_under_kitty_flags() {
        let plain = enc("shift+enter");
        assert_eq!(
            plain,
            b"\r".to_vec(),
            "legacy encoding cannot distinguish it — that is the problem"
        );

        let flags = KittyKeyboardFlags {
            disambiguate: true,
            ..KittyKeyboardFlags::default()
        };
        let kitty = encode_keys("shift+enter", false, flags).expect("encodes");
        assert_eq!(kitty, b"\x1b[13;2u".to_vec());
        // A plain Enter keeps its byte even with the protocol on, since the
        // program only asked for disambiguation.
        assert_eq!(
            encode_keys("enter", false, flags).expect("encodes"),
            b"\r".to_vec()
        );
    }

    // ── any_submits (the permission gate) ────────────────────────────────────

    #[test]
    fn submitting_specs_are_recognised() {
        for k in [
            "enter",
            "return",
            "cr",
            "ENTER",
            "ctrl+m",
            "ctrl+j",
            "down enter",
            "ctrl+c enter",
            "shift+enter",
        ] {
            assert!(any_submits(k), "{k} must count as submitting");
        }
    }

    #[test]
    fn non_submitting_specs_are_not_flagged() {
        for k in ["ctrl+c", "escape", "down down", "a b c", "tab", "f5", ""] {
            assert!(!any_submits(k), "{k} must not count as submitting");
        }
    }

    // ── errors ───────────────────────────────────────────────────────────────

    #[test]
    fn unknown_tokens_are_reported_with_the_spec() {
        let e = encode_keys("hyper+x", false, no_kitty()).expect_err("unknown modifier");
        assert!(e.contains("hyper"), "{e}");
        let e = encode_keys("wat", false, no_kitty()).expect_err("unknown key");
        assert!(e.contains("wat"), "{e}");
        assert!(encode_keys("", false, no_kitty()).is_err());
        assert!(encode_keys("   ", false, no_kitty()).is_err());
    }
}
