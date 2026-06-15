//! Kitty keyboard protocol — the *send* side (keystroke → PTY bytes).
//!
//! The receive side (parsing `CSI > … u` / `CSI < … u` / `CSI = … u` /
//! `CSI ? u`, maintaining the per-screen progressive-enhancement flag stack,
//! and answering the query) is handled inside `alacritty_terminal`, enabled
//! via `Config::kitty_keyboard` in `terminale-term`. This module is the
//! complement: given the flags the focused app has pushed (read from the
//! emulator via [`terminale_term::KittyKeyboardFlags`]) and a winit key event,
//! it produces the `CSI … u` encoding the app expects — most importantly the
//! `CSI 13 ; 2 u` that lets Claude Code (and other modern TUIs) tell
//! `Shift+Enter` apart from a plain `Enter`.
//!
//! Spec: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
//!
//! Design notes / deliberate limitations:
//! - Key codes are resolved against a US-QWERTY base layout for the recovery
//!   of the un-shifted code (so `Shift+1` reports key `49` not `33`). Exotic
//!   layouts fall back to the lower-cased logical character, which is correct
//!   for letters everywhere and close enough elsewhere.
//! - Base-layout alternate key (the third sub-field of the key code) is not
//!   reported — winit does not expose it portably. Shifted alternate keys
//!   *are* reported when `report_alternate_keys` is active.
//! - Lone modifier presses/releases (Shift/Ctrl/Alt/Super by themselves) are
//!   not reported; they fall through to the legacy path (which emits nothing).

use terminale_term::KittyKeyboardFlags;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// Phase of a key event, mapped to the protocol's event-type codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyPhase {
    /// Initial key-down (event type 1 — omitted from the wire form).
    Press,
    /// Auto-repeat key-down (event type 2).
    Repeat,
    /// Key-up (event type 3).
    Release,
}

impl KeyPhase {
    /// Protocol event-type code.
    fn code(self) -> u8 {
        match self {
            KeyPhase::Press => 1,
            KeyPhase::Repeat => 2,
            KeyPhase::Release => 3,
        }
    }
}

/// What the caller should do with a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KittyOutcome {
    /// Send these bytes to the PTY (a `CSI … u`/`~`/letter sequence).
    Bytes(Vec<u8>),
    /// Not a kitty-encoded key — fall back to the legacy xterm encoder.
    Legacy,
    /// Emit nothing (e.g. a key release we are not asked to report).
    Ignore,
}

/// A functional (non-text) key: the protocol number and the CSI terminator it
/// uses. `is_escape` marks the one key that disambiguate-mode encodes even
/// without modifiers.
struct FuncKey {
    number: u32,
    terminator: u8,
    is_escape: bool,
}

/// Map a named key to its protocol number + terminator, or `None` when the key
/// is not a recognised functional key (text keys, lone modifiers, …).
fn functional_key(named: NamedKey) -> Option<FuncKey> {
    let f = |number, terminator| FuncKey {
        number,
        terminator,
        is_escape: false,
    };
    Some(match named {
        NamedKey::Enter => f(13, b'u'),
        NamedKey::Tab => f(9, b'u'),
        NamedKey::Backspace => f(127, b'u'),
        NamedKey::Escape => FuncKey {
            number: 27,
            terminator: b'u',
            is_escape: true,
        },
        NamedKey::Insert => f(2, b'~'),
        NamedKey::Delete => f(3, b'~'),
        NamedKey::PageUp => f(5, b'~'),
        NamedKey::PageDown => f(6, b'~'),
        NamedKey::ArrowUp => f(1, b'A'),
        NamedKey::ArrowDown => f(1, b'B'),
        NamedKey::ArrowRight => f(1, b'C'),
        NamedKey::ArrowLeft => f(1, b'D'),
        NamedKey::Home => f(1, b'H'),
        NamedKey::End => f(1, b'F'),
        NamedKey::F1 => f(1, b'P'),
        NamedKey::F2 => f(1, b'Q'),
        NamedKey::F3 => f(1, b'R'),
        NamedKey::F4 => f(1, b'S'),
        NamedKey::F5 => f(15, b'~'),
        NamedKey::F6 => f(17, b'~'),
        NamedKey::F7 => f(18, b'~'),
        NamedKey::F8 => f(19, b'~'),
        NamedKey::F9 => f(20, b'~'),
        NamedKey::F10 => f(21, b'~'),
        NamedKey::F11 => f(23, b'~'),
        NamedKey::F12 => f(24, b'~'),
        _ => return None,
    })
}

/// US-QWERTY un-shifted character for a physical key, used to recover the
/// protocol key code (which is layout-independent of Shift). Letters map to
/// their lower-case form. Returns `None` for keys with no single-char base.
fn us_layout_unshifted(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Backquote => '`',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        KeyCode::Space => ' ',
        _ => return None,
    })
}

/// Resolve the protocol key code for a text key: the layout-independent
/// un-shifted Unicode codepoint. Prefers the physical-key base (so `Shift+1`
/// → `1`), falling back to the lower-cased logical character.
fn text_key_code(physical: PhysicalKey, logical: &Key) -> Option<u32> {
    if let PhysicalKey::Code(code) = physical {
        if let Some(c) = us_layout_unshifted(code) {
            return Some(c as u32);
        }
    }
    if let Key::Character(s) = logical {
        let c = s.chars().next()?;
        // Lower-case so the key code is Shift-independent (kitty reports the
        // base key in the first field and the shifted form as an alternate).
        return Some(c.to_lowercase().next().unwrap_or(c) as u32);
    }
    None
}

/// Modifier bitmask in the protocol's order: shift=1, alt=2, ctrl=4, super=8.
/// (caps-/num-lock bits are not surfaced by winit's `ModifiersState`.)
fn mod_mask(mods: &ModifiersState) -> u32 {
    let mut m = 0;
    if mods.shift_key() {
        m |= 1;
    }
    if mods.alt_key() {
        m |= 2;
    }
    if mods.control_key() {
        m |= 4;
    }
    if mods.super_key() {
        m |= 8;
    }
    m
}

/// Printable codepoints a key produced, for the optional associated-text
/// field. Control characters (CR, TAB, DEL, …) are excluded — the protocol
/// only reports text that is actually inserted.
fn associated_text_codepoints(text: Option<&str>) -> Vec<u32> {
    let Some(t) = text else {
        return Vec::new();
    };
    let cps: Vec<u32> = t
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| c as u32)
        .collect();
    cps
}

/// Assemble the final escape sequence.
///
/// `number` is the key code (always present — letter-terminated functional
/// keys pass `1`). `shifted` is the optional alternate (shifted) key code.
/// `text_cps` is the associated-text field (empty = omitted). The modifier
/// field is emitted whenever it carries information (non-default modifiers, a
/// non-press event type, or a trailing text field that needs the separator).
fn build_sequence(
    number: u32,
    shifted: Option<u32>,
    modmask: u32,
    phase: KeyPhase,
    text_cps: &[u32],
    terminator: u8,
) -> Vec<u8> {
    let mut s = String::with_capacity(16);
    s.push_str("\x1b[");
    s.push_str(&number.to_string());
    if let Some(alt) = shifted {
        s.push(':');
        s.push_str(&alt.to_string());
    }

    let event_code = phase.code();
    // The modifier field must appear if modifiers are held, the event type is
    // not a plain press, or a text field follows (it occupies the 3rd slot, so
    // the 2nd must be present as a placeholder).
    let need_mods = modmask != 0 || event_code != 1 || !text_cps.is_empty();
    if need_mods {
        s.push(';');
        s.push_str(&(modmask + 1).to_string());
        if event_code != 1 {
            s.push(':');
            s.push_str(&event_code.to_string());
        }
    }

    if !text_cps.is_empty() {
        s.push(';');
        for (i, cp) in text_cps.iter().enumerate() {
            if i > 0 {
                s.push(':');
            }
            s.push_str(&cp.to_string());
        }
    }

    s.push(terminator as char);
    s.into_bytes()
}

/// Encode a key event under the active kitty keyboard flags.
///
/// Returns [`KittyOutcome::Legacy`] when the key should use the legacy xterm
/// encoder instead (e.g. a plain text key under disambiguate-only mode), and
/// [`KittyOutcome::Ignore`] when nothing should be sent (e.g. a key release
/// while event reporting is off).
pub(crate) fn encode_key(
    flags: KittyKeyboardFlags,
    mods: &ModifiersState,
    physical_key: PhysicalKey,
    logical_key: &Key,
    text: Option<&str>,
    phase: KeyPhase,
) -> KittyOutcome {
    // Releases only matter when the app asked for event reporting; otherwise
    // terminals never transmit key-up.
    if phase == KeyPhase::Release && !flags.report_event_types {
        return KittyOutcome::Ignore;
    }
    if !flags.any() {
        // Protocol not engaged: presses use legacy encoding, releases nothing.
        return if phase == KeyPhase::Release {
            KittyOutcome::Ignore
        } else {
            KittyOutcome::Legacy
        };
    }

    let modmask = mod_mask(mods);
    let report_all = flags.report_all_keys_as_esc;

    // ── Functional keys (Enter, Tab, arrows, F-keys, …) ──────────────────────
    if let Key::Named(named) = logical_key {
        if let Some(fk) = functional_key(*named) {
            // Encode when: modified, the app wants every key as an escape code,
            // event types are being reported (so press/release stay paired), or
            // this is the Escape key under disambiguate mode. Otherwise the
            // unmodified key keeps its legacy byte(s) (\r, \t, 0x7f, CSI A …).
            let encode = report_all
                || modmask != 0
                || flags.report_event_types
                || (fk.is_escape && flags.disambiguate);
            if !encode {
                return if phase == KeyPhase::Release {
                    KittyOutcome::Ignore
                } else {
                    KittyOutcome::Legacy
                };
            }
            // Associated text is reported only for keys that insert printable
            // text; functional keys generally don't, so this is usually empty.
            let text_cps = if flags.report_associated_text && phase != KeyPhase::Release {
                associated_text_codepoints(text)
            } else {
                Vec::new()
            };
            return KittyOutcome::Bytes(build_sequence(
                fk.number,
                None,
                modmask,
                phase,
                &text_cps,
                fk.terminator,
            ));
        }
        // A named key we don't model (lone modifiers, media keys, …): leave it
        // to the legacy path.
        return if phase == KeyPhase::Release {
            KittyOutcome::Ignore
        } else {
            KittyOutcome::Legacy
        };
    }

    // ── Text keys ────────────────────────────────────────────────────────────
    // Encoded only when a non-shift modifier is held (so Ctrl+I is told apart
    // from Tab) or the app wants every key as an escape code. Shift-only and
    // unmodified text keys stay plain so ordinary typing is untouched.
    let has_non_shift_mod = (modmask & !1) != 0;
    if !report_all && !has_non_shift_mod {
        return if phase == KeyPhase::Release {
            KittyOutcome::Ignore
        } else {
            KittyOutcome::Legacy
        };
    }
    let Some(number) = text_key_code(physical_key, logical_key) else {
        return if phase == KeyPhase::Release {
            KittyOutcome::Ignore
        } else {
            KittyOutcome::Legacy
        };
    };

    // Shifted alternate key code (e.g. base `97` 'a' with shifted `65` 'A'),
    // reported only when requested and when it actually differs from the base.
    let shifted = if flags.report_alternate_keys && mods.shift_key() {
        if let Key::Character(s) = logical_key {
            s.chars().next().map(|c| c as u32).filter(|&cp| cp != number)
        } else {
            None
        }
    } else {
        None
    };

    let text_cps = if flags.report_associated_text && phase != KeyPhase::Release {
        associated_text_codepoints(text)
    } else {
        Vec::new()
    };

    KittyOutcome::Bytes(build_sequence(
        number, shifted, modmask, phase, &text_cps, b'u',
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn flags(
        disambiguate: bool,
        report_event_types: bool,
        report_alternate_keys: bool,
        report_all_keys_as_esc: bool,
        report_associated_text: bool,
    ) -> KittyKeyboardFlags {
        KittyKeyboardFlags {
            disambiguate,
            report_event_types,
            report_alternate_keys,
            report_all_keys_as_esc,
            report_associated_text,
        }
    }

    fn disambiguate_only() -> KittyKeyboardFlags {
        flags(true, false, false, false, false)
    }

    fn mods(shift: bool, ctrl: bool, alt: bool, sup: bool) -> ModifiersState {
        let mut m = ModifiersState::empty();
        if shift {
            m |= ModifiersState::SHIFT;
        }
        if ctrl {
            m |= ModifiersState::CONTROL;
        }
        if alt {
            m |= ModifiersState::ALT;
        }
        if sup {
            m |= ModifiersState::SUPER;
        }
        m
    }

    fn named(n: NamedKey) -> Key {
        Key::Named(n)
    }

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }

    fn bytes(o: KittyOutcome) -> Vec<u8> {
        match o {
            KittyOutcome::Bytes(b) => b,
            other => panic!("expected Bytes, got {other:?}"),
        }
    }

    #[test]
    fn shift_enter_is_csi_13_2_u() {
        // The headline case: Claude Code reads CSI 13;2u as Shift+Enter.
        let out = encode_key(
            disambiguate_only(),
            &mods(true, false, false, false),
            PhysicalKey::Code(KeyCode::Enter),
            &named(NamedKey::Enter),
            None,
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[13;2u");
    }

    #[test]
    fn plain_enter_stays_legacy() {
        // Plain Enter must keep \r so it still submits in shells and Claude.
        let out = encode_key(
            disambiguate_only(),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::Enter),
            &named(NamedKey::Enter),
            None,
            KeyPhase::Press,
        );
        assert_eq!(out, KittyOutcome::Legacy);
    }

    #[test]
    fn plain_letter_stays_legacy() {
        let out = encode_key(
            disambiguate_only(),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("a"),
            Some("a"),
            KeyPhase::Press,
        );
        assert_eq!(out, KittyOutcome::Legacy);
    }

    #[test]
    fn shift_letter_stays_legacy_text() {
        // Shift alone on a text key keeps plain text ("A"), not CSI u.
        let out = encode_key(
            disambiguate_only(),
            &mods(true, false, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("A"),
            Some("A"),
            KeyPhase::Press,
        );
        assert_eq!(out, KittyOutcome::Legacy);
    }

    #[test]
    fn ctrl_letter_is_disambiguated() {
        // Ctrl+A → CSI 97;5u (modmask ctrl=4, +1 = 5).
        let out = encode_key(
            disambiguate_only(),
            &mods(false, true, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("a"),
            None,
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[97;5u");
    }

    #[test]
    fn ctrl_i_is_distinct_from_tab() {
        // Ctrl+I encodes as CSI 105;5u — the whole point of disambiguation.
        let out = encode_key(
            disambiguate_only(),
            &mods(false, true, false, false),
            PhysicalKey::Code(KeyCode::KeyI),
            &ch("i"),
            None,
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[105;5u");
    }

    #[test]
    fn escape_is_disambiguated_unmodified() {
        let out = encode_key(
            disambiguate_only(),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::Escape),
            &named(NamedKey::Escape),
            None,
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[27u");
    }

    #[test]
    fn shift_arrow_uses_letter_terminator() {
        // Shift+Up → CSI 1;2A.
        let out = encode_key(
            disambiguate_only(),
            &mods(true, false, false, false),
            PhysicalKey::Code(KeyCode::ArrowUp),
            &named(NamedKey::ArrowUp),
            None,
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[1;2A");
    }

    #[test]
    fn unmodified_arrow_stays_legacy() {
        let out = encode_key(
            disambiguate_only(),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::ArrowUp),
            &named(NamedKey::ArrowUp),
            None,
            KeyPhase::Press,
        );
        assert_eq!(out, KittyOutcome::Legacy);
    }

    #[test]
    fn report_all_keys_encodes_plain_letter() {
        let out = encode_key(
            flags(true, false, false, true, false),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("a"),
            Some("a"),
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[97u");
    }

    #[test]
    fn associated_text_appended() {
        // report_all + report_associated_text: 'a' → CSI 97;;97u
        // (mods field present as empty-default placeholder so text is the 3rd).
        let out = encode_key(
            flags(true, false, false, true, true),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("a"),
            Some("a"),
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[97;1;97u");
    }

    #[test]
    fn release_ignored_without_event_reporting() {
        let out = encode_key(
            disambiguate_only(),
            &mods(false, false, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("a"),
            None,
            KeyPhase::Release,
        );
        assert_eq!(out, KittyOutcome::Ignore);
    }

    #[test]
    fn release_reported_with_event_types() {
        // Ctrl+A release with event reporting → CSI 97;5:3u.
        let out = encode_key(
            flags(true, true, false, false, false),
            &mods(false, true, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("a"),
            None,
            KeyPhase::Release,
        );
        assert_eq!(bytes(out), b"\x1b[97;5:3u");
    }

    #[test]
    fn repeat_event_type() {
        // Shift+Enter repeat → CSI 13;2:2u.
        let out = encode_key(
            flags(true, true, false, false, false),
            &mods(true, false, false, false),
            PhysicalKey::Code(KeyCode::Enter),
            &named(NamedKey::Enter),
            None,
            KeyPhase::Repeat,
        );
        assert_eq!(bytes(out), b"\x1b[13;2:2u");
    }

    #[test]
    fn alternate_key_reported() {
        // Ctrl+Shift+a with alternate keys → base 97, shifted 65, mods 6.
        let out = encode_key(
            flags(true, false, true, false, false),
            &mods(true, true, false, false),
            PhysicalKey::Code(KeyCode::KeyA),
            &ch("A"),
            None,
            KeyPhase::Press,
        );
        assert_eq!(bytes(out), b"\x1b[97:65;6u");
    }

    #[test]
    fn shift_digit_recovers_base_key() {
        // Ctrl+Shift+1 ('!' on US): base key code is '1' (49), not '!'.
        let out = encode_key(
            disambiguate_only(),
            &mods(true, true, false, false),
            PhysicalKey::Code(KeyCode::Digit1),
            &ch("!"),
            None,
            KeyPhase::Press,
        );
        // modmask shift|ctrl = 1|4 = 5, +1 = 6.
        assert_eq!(bytes(out), b"\x1b[49;6u");
    }

    #[test]
    fn inactive_flags_fall_back_to_legacy() {
        let out = encode_key(
            KittyKeyboardFlags::default(),
            &mods(true, false, false, false),
            PhysicalKey::Code(KeyCode::Enter),
            &named(NamedKey::Enter),
            None,
            KeyPhase::Press,
        );
        assert_eq!(out, KittyOutcome::Legacy);
    }
}
