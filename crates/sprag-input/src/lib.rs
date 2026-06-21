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

/// Encode a W3C named key (`"Enter"`, `"ArrowUp"`, `"F5"`, …).
fn encode_named(key: &str, mods: Modifiers, modes: InputModes) -> Option<Vec<u8>> {
    // Simple keys that map to a single C0/DEL byte. `Alt` prefixes `ESC`;
    // `Shift`+`Tab` is the back-tab (CBT) exception.
    match key {
        "Enter" => return Some(simple(0x0d, mods)),
        "Tab" if mods.shift => return Some(csi(b"", b'Z')),
        "Tab" => return Some(simple(0x09, mods)),
        "Backspace" => return Some(simple(0x7f, mods)),
        "Escape" => return Some(simple(ESC, mods)),
        _ => {}
    }

    // Cursor / positional keys with a single letter final: `CSI`/`SS3` per
    // DECCKM when unmodified, always `CSI 1;mod final` when modified.
    if let Some(final_byte) = letter_final(key) {
        return Some(if mods.any() {
            csi(format!("1;{}", mods.xterm_param()).as_bytes(), final_byte)
        } else if modes.application_cursor_keys {
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
        }
    }

    fn enc(key: &str, mods: Modifiers) -> Vec<u8> {
        encode(key, mods, modes(false)).expect("encodable")
    }

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        alt: false,
        shift: false,
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
}
