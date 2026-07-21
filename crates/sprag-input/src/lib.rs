//! sprag-input — encode keys into PTY input bytes (PINION-REQUIREMENTS R2.6).
//!
//! R2.6 places key→PTY-byte encoding on sprag: pinion provides the stable
//! W3C `KeyboardEvent.key` representation, and sprag owns the policy that
//! turns a key string + [`Modifiers`] (plus the terminal's [`InputModes`])
//! into the bytes the focused child process reads. The scheme is the
//! de-facto xterm one: C0 control codes for `Ctrl`, an `ESC` prefix for
//! `Alt` (metaSendsEscape), `CSI`/`SS3` introducers for the named keys, the
//! PC-style modifier parameter (`1 + Shift + Alt*2 + Ctrl*4 + Meta*8`), and
//! DECCKM-dependent cursor keys.
//!
//! This crate is pinion-free by design — it lives in the producer layer
//! beside sprag-vt and depends on it only for [`InputModes`].

use sprag_vt::InputModes;

/// The escape byte (`0x1b`) that introduces every `CSI`/`SS3` sequence and
/// prefixes `Alt`-modified keys.
const ESC: u8 = 0x1b;

/// Keyboard modifier state accompanying a key (PINION-REQUIREMENTS R2.5).
///
/// Mirrors pinion's four-bool `Modifiers`. `shift` is only consulted for
/// keys whose W3C string does not already fold it in (`Tab`→back-tab, the
/// cursor/function keys); for a printable character key the case or symbol
/// is already reflected in the key string, so `shift` is not re-applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The "super"/logo/meta key. Contributes the meta bit (8) to the
    /// modifier parameter; terminals otherwise have no encoding for it.
    pub sup: bool,
}

impl Modifiers {
    /// The xterm/PC modifier parameter: `1 + Shift + Alt*2 + Ctrl*4 +
    /// Meta*8`. A value of `1` means "no modifiers" and selects the base
    /// (unparameterized) sequence.
    fn xterm_param(self) -> u8 {
        1 + u8::from(self.shift)
            + (u8::from(self.alt) << 1)
            + (u8::from(self.ctrl) << 2)
            + (u8::from(self.sup) << 3)
    }

    /// Whether any modifier is held (equivalently `xterm_param() != 1`).
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.sup
    }
}

/// Encode a W3C `KeyboardEvent.key` string plus modifiers into the PTY
/// input bytes the focused child should receive, given the terminal's
/// current input [`InputModes`].
///
/// Returns `None` for an empty key, a multi-codepoint non-named string
/// (e.g. IME composition output), or an unrecognized named key — the
/// caller rejects loudly rather than injecting nothing silently. A
/// single-codepoint key always encodes (its bytes are well defined).
#[must_use]
pub fn encode(key: &str, mods: Modifiers, modes: InputModes) -> Option<Vec<u8>> {
    // Under the Kitty keyboard protocol's DISAMBIGUATE flag the whole encoding changes (Esc and
    // Ctrl/Alt/Super combos become unambiguous `CSI u` codes), so it is a distinct path, not a
    // tweak of the legacy one.
    if modes.kitty_keyboard.disambiguate() {
        return encode_kitty_disambiguate(key, mods);
    }
    let mut chars = key.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        // Single Unicode scalar → a character key.
        Some(encode_char(first, mods))
    } else {
        // Multi-char → a W3C named key.
        encode_named(key, mods, modes)
    }
}

/// Encode a single character key: its UTF-8 bytes, transformed to a C0
/// control code under `Ctrl` and prefixed with `ESC` under `Alt`.
fn encode_char(c: char, mods: Modifiers) -> Vec<u8> {
    let mut bytes = match (mods.ctrl, control_byte(c)) {
        (true, Some(ctl)) => vec![ctl],
        // Ctrl with no C0 equivalent (digits, most symbols): default xterm
        // emits the unmodified character (modifyOtherKeys is not modeled).
        _ => char_utf8(c),
    };
    if mods.alt {
        bytes.insert(0, ESC); // metaSendsEscape
    }
    bytes
}

/// The C0 control byte `Ctrl`+`c` produces, or `None` when the character
/// has no control equivalent.
fn control_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1), // ^A..^Z → 0x01..0x1A
        // ^@ ^A..^Z ^[ ^\ ^] ^^ ^_ → 0x00..0x1F (uppercase + the symbols).
        '@'..='_' => Some(c as u8 & 0x1f),
        ' ' => Some(0x00), // Ctrl+Space → NUL
        '?' => Some(0x7f), // Ctrl+? → DEL
        _ => None,
    }
}

/// The UTF-8 encoding of a character as owned bytes.
fn char_utf8(c: char) -> Vec<u8> {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// Encode a W3C named key (`"Enter"`, `"ArrowUp"`, `"F5"`, `"Space"`, …).
fn encode_named(key: &str, mods: Modifiers, modes: InputModes) -> Option<Vec<u8>> {
    // Simple keys that map to a single C0/DEL byte. `Alt` prefixes `ESC`;
    // `Shift`+`Tab` is the back-tab (CBT) exception.
    match key {
        "Enter" => return Some(simple(0x0d, mods)),
        "Tab" if mods.shift => return Some(csi(b"", b'Z')),
        "Tab" => return Some(simple(0x09, mods)),
        "Backspace" => return Some(simple(0x7f, mods)),
        "Escape" => return Some(simple(ESC, mods)),
        // The spacebar's W3C `key` is the single space " " (handled by the
        // single-codepoint path), but a platform / IME layer may deliver the
        // `code`-style name "Space" instead — notably the space that COMMITS a
        // Hangul/CJK composition arrives named, not as " ". Encode it identically to
        // the space character (Ctrl+Space → NUL, Alt+Space → ESC-prefixed) so a space
        // typed mid-composition is not silently dropped.
        "Space" => return Some(encode_char(' ', mods)),
        _ => {}
    }
    encode_functional(key, mods, modes.application_cursor_keys)
}

/// Encode the cursor / navigation / function keys — the part of a named key shared by the legacy
/// path ([`encode_named`]) and the Kitty disambiguate path ([`encode_kitty_disambiguate`], which
/// passes `application_cursor_keys = false` because the enhanced protocol reports these with `CSI`
/// finals, not the DECCKM `SS3` form). `None` for a name that is not one of these keys.
fn encode_functional(key: &str, mods: Modifiers, application_cursor_keys: bool) -> Option<Vec<u8>> {
    // Cursor / positional keys with a single letter final: `CSI`/`SS3` per
    // DECCKM when unmodified, always `CSI 1;mod final` when modified.
    if let Some(final_byte) = letter_final(key) {
        return Some(if mods.any() {
            csi(format!("1;{}", mods.xterm_param()).as_bytes(), final_byte)
        } else if application_cursor_keys {
            vec![ESC, b'O', final_byte]
        } else {
            vec![ESC, b'[', final_byte]
        });
    }

    // Keypad / extended function keys terminated by `~`.
    if let Some(num) = tilde_number(key) {
        return Some(if mods.any() {
            csi(format!("{num};{}", mods.xterm_param()).as_bytes(), b'~')
        } else {
            csi(num.to_string().as_bytes(), b'~')
        });
    }

    // F1–F4: `SS3` base, switching to `CSI 1;mod final` when modified.
    if let Some(final_byte) = ss3_function(key) {
        return Some(if mods.any() {
            csi(format!("1;{}", mods.xterm_param()).as_bytes(), final_byte)
        } else {
            vec![ESC, b'O', final_byte]
        });
    }

    None
}

/// Encode a key under the Kitty keyboard protocol's DISAMBIGUATE flag. Relative to the legacy
/// encoding, the ONLY keys whose bytes change are the genuinely-ambiguous ones: `Esc` (always a
/// `CSI 27 u` now, so a lone Escape is distinct from an escape-sequence prefix), any printable key
/// held with `Ctrl`/`Alt`/`Super` (a `CSI code ; mods u` code, so e.g. `Ctrl+i` is distinct from
/// `Tab`), and a MODIFIED `Enter`/`Tab`/`Backspace`. Everything else — unmodified text, shifted
/// text, unmodified `Enter`/`Tab`/`Backspace`, and the cursor / navigation / function keys — keeps
/// its legacy bytes (the flag only disambiguates what was ambiguous). `None` for an unrecognized
/// named key, matching [`encode_named`].
fn encode_kitty_disambiguate(key: &str, mods: Modifiers) -> Option<Vec<u8>> {
    let mut chars = key.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        // A single character key: plain text unless Ctrl/Alt/Super is held.
        return Some(encode_kitty_char(first, mods));
    }
    match key {
        // Esc is ALWAYS reported as a CSI u code under disambiguate — that is the flag's namesake.
        "Escape" => Some(csi_u(27, mods)),
        // These keep their legacy control byte UNMODIFIED (kitty preserves basic compatibility),
        // but disambiguate to a CSI u code when modified (so Ctrl+Enter ≠ Enter, etc.).
        "Enter" => Some(kitty_legacy_or_u(0x0d, 13, mods)),
        "Tab" => Some(kitty_legacy_or_u(0x09, 9, mods)),
        "Backspace" => Some(kitty_legacy_or_u(0x7f, 127, mods)),
        // The named spacebar encodes exactly like the space character.
        "Space" => Some(encode_kitty_char(' ', mods)),
        // Cursor / navigation / function keys: the legacy CSI encoding, but the CSI (not SS3) form
        // — the enhanced protocol ignores DECCKM for these.
        _ => encode_functional(key, mods, false),
    }
}

/// A single-character key under disambiguate: its plain (already-shifted) text when no
/// `Ctrl`/`Alt`/`Super` is held, else the unambiguous `CSI code ; mods u` form.
fn encode_kitty_char(c: char, mods: Modifiers) -> Vec<u8> {
    if mods.ctrl || mods.alt || mods.sup {
        csi_u(base_key_code(c), mods)
    } else {
        // No modifiers, or Shift only — the character already carries the shift, so it is text.
        char_utf8(c)
    }
}

/// The Kitty key code for a printable character: the UNSHIFTED base. ASCII uppercase folds to
/// lowercase (so `Ctrl+A` and `Ctrl+Shift+A` share code 97, with Shift carried in the modifier
/// field). A shifted SYMBOL keeps its shifted codepoint — reverse-mapping it to its base key needs
/// the keyboard layout, which the display client does not supply; a documented bound that touches
/// only `Ctrl`/`Alt` + shifted-symbol combos (rare), never letters or unmodified keys.
fn base_key_code(c: char) -> u32 {
    if c.is_ascii_uppercase() {
        c as u32 + 32
    } else {
        c as u32
    }
}

/// `Enter`/`Tab`/`Backspace` under disambiguate: the legacy control byte when unmodified, the
/// `CSI code ; mods u` code when any modifier is held.
fn kitty_legacy_or_u(legacy: u8, code: u32, mods: Modifiers) -> Vec<u8> {
    if mods.any() {
        csi_u(code, mods)
    } else {
        vec![legacy]
    }
}

/// The Kitty functional-key code `CSI code u` (no modifiers) or `CSI code ; modifiers u`, where the
/// modifier value is the same `1 + Shift + Alt*2 + Ctrl*4 + Super*8` the legacy CSI keys use.
fn csi_u(code: u32, mods: Modifiers) -> Vec<u8> {
    if mods.any() {
        csi(format!("{code};{}", mods.xterm_param()).as_bytes(), b'u')
    } else {
        csi(code.to_string().as_bytes(), b'u')
    }
}

/// A single base byte, prefixed with `ESC` when `Alt` is held.
fn simple(byte: u8, mods: Modifiers) -> Vec<u8> {
    if mods.alt {
        vec![ESC, byte]
    } else {
        vec![byte]
    }
}

/// Build `ESC [ <params> <final>`.
fn csi(params: &[u8], final_byte: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity(params.len() + 3);
    v.push(ESC);
    v.push(b'[');
    v.extend_from_slice(params);
    v.push(final_byte);
    v
}

/// The CSI/SS3 final byte for the letter-final cursor/positional keys.
fn letter_final(key: &str) -> Option<u8> {
    Some(match key {
        "ArrowUp" => b'A',
        "ArrowDown" => b'B',
        "ArrowRight" => b'C',
        "ArrowLeft" => b'D',
        "Home" => b'H',
        "End" => b'F',
        _ => return None,
    })
}

/// The numeric parameter for the `~`-terminated keypad/function keys.
fn tilde_number(key: &str) -> Option<u8> {
    Some(match key {
        "Insert" => 2,
        "Delete" => 3,
        "PageUp" => 5,
        "PageDown" => 6,
        "F5" => 15,
        "F6" => 17,
        "F7" => 18,
        "F8" => 19,
        "F9" => 20,
        "F10" => 21,
        "F11" => 23,
        "F12" => 24,
        _ => return None,
    })
}

/// The SS3 final byte for F1–F4.
fn ss3_function(key: &str) -> Option<u8> {
    Some(match key {
        "F1" => b'P',
        "F2" => b'Q',
        "F3" => b'R',
        "F4" => b'S',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(app_cursor: bool) -> InputModes {
        InputModes {
            application_cursor_keys: app_cursor,
            kitty_keyboard: sprag_vt::KittyKeyboardFlags::default(),
        }
    }

    /// Input modes with the Kitty keyboard DISAMBIGUATE flag active.
    fn kitty_modes() -> InputModes {
        InputModes {
            application_cursor_keys: false,
            kitty_keyboard: sprag_vt::KittyKeyboardFlags::from_bits(
                sprag_vt::KittyKeyboardFlags::DISAMBIGUATE,
            ),
        }
    }

    fn enc(key: &str, mods: Modifiers) -> Vec<u8> {
        encode(key, mods, modes(false)).expect("encodable")
    }

    /// Encode a key under the Kitty DISAMBIGUATE flag.
    fn kenc(key: &str, mods: Modifiers) -> Vec<u8> {
        encode(key, mods, kitty_modes()).expect("encodable")
    }

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        alt: false,
        shift: false,
        sup: false,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        alt: false,
        shift: true,
        sup: false,
    };
    const ALT: Modifiers = Modifiers {
        ctrl: false,
        alt: true,
        shift: false,
        sup: false,
    };
    const SHIFT: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: true,
        sup: false,
    };

    #[test]
    fn plain_character_is_its_utf8() {
        assert_eq!(enc("a", Modifiers::default()), b"a");
        assert_eq!(enc("A", Modifiers::default()), b"A"); // shift already folded in
        assert_eq!(enc("$", Modifiers::default()), b"$");
        assert_eq!(enc("\u{4e16}", Modifiers::default()), "\u{4e16}".as_bytes());
    }

    #[test]
    fn ctrl_letter_maps_to_c0_control() {
        assert_eq!(enc("a", CTRL), vec![0x01]);
        assert_eq!(enc("c", CTRL), vec![0x03]);
        assert_eq!(enc("z", CTRL), vec![0x1a]);
        // Ctrl with the symbol set and the NUL/DEL specials.
        assert_eq!(enc("[", CTRL), vec![0x1b]);
        assert_eq!(enc(" ", CTRL), vec![0x00]);
        assert_eq!(enc("?", CTRL), vec![0x7f]);
        // Ctrl with no C0 equivalent falls back to the plain character.
        assert_eq!(enc("1", CTRL), b"1");
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc("a", ALT), vec![ESC, b'a']);
        // Alt+Ctrl composes: ESC then the control byte.
        let alt_ctrl = Modifiers {
            ctrl: true,
            alt: true,
            shift: false,
            sup: false,
        };
        assert_eq!(enc("a", alt_ctrl), vec![ESC, 0x01]);
    }

    #[test]
    fn simple_control_keys() {
        assert_eq!(enc("Enter", Modifiers::default()), vec![0x0d]);
        assert_eq!(enc("Tab", Modifiers::default()), vec![0x09]);
        assert_eq!(enc("Backspace", Modifiers::default()), vec![0x7f]);
        assert_eq!(enc("Escape", Modifiers::default()), vec![ESC]);
        // Alt prefixes ESC on these too.
        assert_eq!(enc("Enter", ALT), vec![ESC, 0x0d]);
    }

    /// The named `"Space"` key (the form the IME-commit path delivers) encodes
    /// identically to the space character — the fix for a space dropped while typing
    /// Hangul. The single-space " " form still works via the character path.
    #[test]
    fn named_space_encodes_like_the_space_char() {
        assert_eq!(enc("Space", Modifiers::default()), vec![0x20]);
        assert_eq!(enc(" ", Modifiers::default()), vec![0x20]);
        // Same modifier behaviour as the space character.
        assert_eq!(enc("Space", CTRL), vec![0x00]); // Ctrl+Space → NUL
        assert_eq!(enc("Space", ALT), vec![ESC, 0x20]); // Alt prefixes ESC
    }

    #[test]
    fn shift_tab_is_back_tab() {
        assert_eq!(enc("Tab", SHIFT), vec![ESC, b'[', b'Z']);
    }

    #[test]
    fn ctrl_i_and_tab_are_distinct() {
        // R2.3 ambiguity: Ctrl+I and Tab both yield 0x09 at the byte level,
        // but arrive as distinct keys, so an app using modifyOtherKeys could
        // tell them apart upstream. Here they coincide by design (legacy).
        assert_eq!(enc("i", CTRL), vec![0x09]);
        assert_eq!(enc("Tab", Modifiers::default()), vec![0x09]);
        // Escape vs Ctrl+[ likewise coincide at 0x1b.
        assert_eq!(enc("[", CTRL), vec![ESC]);
        assert_eq!(enc("Escape", Modifiers::default()), vec![ESC]);
    }

    #[test]
    fn arrows_follow_decckm() {
        // Normal (DECCKM off): CSI.
        assert_eq!(
            encode("ArrowUp", Modifiers::default(), modes(false)).unwrap(),
            vec![ESC, b'[', b'A']
        );
        assert_eq!(
            encode("ArrowLeft", Modifiers::default(), modes(false)).unwrap(),
            vec![ESC, b'[', b'D']
        );
        // Application cursor keys (DECCKM on): SS3.
        assert_eq!(
            encode("ArrowUp", Modifiers::default(), modes(true)).unwrap(),
            vec![ESC, b'O', b'A']
        );
        assert_eq!(
            encode("ArrowRight", Modifiers::default(), modes(true)).unwrap(),
            vec![ESC, b'O', b'C']
        );
    }

    #[test]
    fn modified_cursor_keys_use_csi_param() {
        // Ctrl+Right → CSI 1 ; 5 C (modifier param 5 = 1 + ctrl*4).
        assert_eq!(
            enc("ArrowRight", CTRL),
            vec![ESC, b'[', b'1', b';', b'5', b'C']
        );
        // A modifier forces CSI even under DECCKM.
        assert_eq!(
            encode("ArrowUp", CTRL, modes(true)).unwrap(),
            vec![ESC, b'[', b'1', b';', b'5', b'A']
        );
        // Home / End are in the same group.
        assert_eq!(enc("End", CTRL), vec![ESC, b'[', b'1', b';', b'5', b'F']);
        assert_eq!(enc("Home", Modifiers::default()), vec![ESC, b'[', b'H']);
    }

    #[test]
    fn tilde_keys() {
        assert_eq!(
            enc("Insert", Modifiers::default()),
            vec![ESC, b'[', b'2', b'~']
        );
        assert_eq!(
            enc("Delete", Modifiers::default()),
            vec![ESC, b'[', b'3', b'~']
        );
        assert_eq!(
            enc("PageUp", Modifiers::default()),
            vec![ESC, b'[', b'5', b'~']
        );
        assert_eq!(
            enc("PageDown", Modifiers::default()),
            vec![ESC, b'[', b'6', b'~']
        );
        // Shift+Delete → CSI 3 ; 2 ~.
        assert_eq!(
            enc("Delete", SHIFT),
            vec![ESC, b'[', b'3', b';', b'2', b'~']
        );
    }

    #[test]
    fn function_keys() {
        // F1–F4 are SS3.
        assert_eq!(enc("F1", Modifiers::default()), vec![ESC, b'O', b'P']);
        assert_eq!(enc("F4", Modifiers::default()), vec![ESC, b'O', b'S']);
        // Modified F1 switches to CSI form.
        assert_eq!(enc("F1", SHIFT), vec![ESC, b'[', b'1', b';', b'2', b'P']);
        // F5–F12 are CSI ~ with the historical numbering gaps.
        assert_eq!(
            enc("F5", Modifiers::default()),
            vec![ESC, b'[', b'1', b'5', b'~']
        );
        assert_eq!(
            enc("F12", Modifiers::default()),
            vec![ESC, b'[', b'2', b'4', b'~']
        );
    }

    #[test]
    fn unencodable_keys_return_none() {
        assert_eq!(encode("", Modifiers::default(), modes(false)), None);
        assert_eq!(encode("Nonsense", Modifiers::default(), modes(false)), None);
        // Multi-codepoint (IME composition) is not a single key.
        assert_eq!(
            encode("\u{c548}\u{b155}", Modifiers::default(), modes(false)),
            None
        );
    }

    // ----- Kitty keyboard protocol: DISAMBIGUATE encoding -----

    /// Under the disambiguate flag, only the genuinely-ambiguous keys change bytes. Text keys with
    /// no modifiers or Shift-only stay plain; Esc and Ctrl/Alt/Super combos become `CSI u` codes.
    /// The modifier value is `1 + Shift + Alt*2 + Ctrl*4 + Super*8`, and the key code is the
    /// UNSHIFTED base (so Ctrl+A and Ctrl+Shift+A share code 97).
    #[test]
    fn kitty_disambiguate_text_and_modified_keys() {
        // No / Shift-only modifiers → plain (already-shifted) UTF-8 text.
        assert_eq!(kenc("a", Modifiers::default()), b"a");
        assert_eq!(kenc("A", SHIFT), b"A");
        // Ctrl / Alt printable → CSI code;mods u (code 97 = unshifted 'a').
        assert_eq!(kenc("a", CTRL), b"\x1b[97;5u"); // ctrl → 1+4 = 5 (NOT signal 0x01)
        assert_eq!(kenc("a", ALT), b"\x1b[97;3u"); // alt  → 1+2 = 3
        assert_eq!(kenc("A", CTRL_SHIFT), b"\x1b[97;6u"); // ctrl+shift → 1+4+1 = 6, base 'a'
        // Esc is ALWAYS a CSI u code — the namesake disambiguation.
        assert_eq!(kenc("Escape", Modifiers::default()), b"\x1b[27u");
        assert_eq!(kenc("Escape", CTRL), b"\x1b[27;5u");
        // Enter/Tab/Backspace: legacy byte unmodified, CSI u when modified.
        assert_eq!(kenc("Enter", Modifiers::default()), vec![0x0d]);
        assert_eq!(kenc("Enter", CTRL), b"\x1b[13;5u");
        assert_eq!(kenc("Tab", Modifiers::default()), vec![0x09]);
        assert_eq!(kenc("Tab", SHIFT), b"\x1b[9;2u"); // shift → 1+1 = 2
        assert_eq!(kenc("Backspace", Modifiers::default()), vec![0x7f]);
        // Space: plain 0x20 unmodified; Ctrl+Space → CSI 32;5 u (32 = the space codepoint).
        assert_eq!(kenc("Space", Modifiers::default()), b" ");
        assert_eq!(kenc(" ", CTRL), b"\x1b[32;5u");
    }

    /// The cursor / navigation / function keys under disambiguate use the legacy CSI encoding, but
    /// the `CSI` (not `SS3`) form — the enhanced protocol ignores DECCKM for these.
    #[test]
    fn kitty_disambiguate_functional_keys_use_csi() {
        assert_eq!(kenc("ArrowUp", Modifiers::default()), b"\x1b[A");
        assert_eq!(kenc("ArrowUp", CTRL), b"\x1b[1;5A");
        assert_eq!(kenc("Home", Modifiers::default()), b"\x1b[H");
        assert_eq!(kenc("PageUp", Modifiers::default()), b"\x1b[5~");
        assert_eq!(kenc("Delete", CTRL), b"\x1b[3;5~");
    }

    /// The disambiguate encoder RE-USES the legacy functional path with DECCKM forced off, so the
    /// refactor that extracted it did not change the legacy output: an arrow in application-cursor
    /// mode still encodes as SS3 when the flag is off. (Guards the refactor.)
    #[test]
    fn legacy_functional_encoding_is_unchanged_by_the_refactor() {
        assert_eq!(
            encode("ArrowUp", Modifiers::default(), modes(true)),
            Some(vec![ESC, b'O', b'A']),
            "DECCKM arrow stays SS3 in the legacy path",
        );
        assert_eq!(
            encode("ArrowUp", CTRL, modes(true)),
            Some(b"\x1b[1;5A".to_vec()),
            "a modified arrow is CSI 1;mods regardless of DECCKM",
        );
    }
}
