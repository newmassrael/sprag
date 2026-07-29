//! The local terminal's keystrokes -> the wire's key vocabulary.
//!
//! [`sprag_input::encode`] is the FORWARD direction — a W3C `KeyboardEvent.key` name plus
//! modifiers becomes the bytes a child process reads — and it runs on the HOST, at the PTY
//! boundary. This module is its inverse, and it runs in the client: bytes this terminal produced,
//! back into the name-and-modifiers pair [`HostClient::send_key`](sprag_host::HostClient::send_key)
//! carries.
//!
//! # Why a decode-and-re-encode rather than a byte passthrough
//!
//! It looks like wasted work — the client has bytes, the child wants bytes — and it is not. **The
//! two ends are in different modes, and only the host knows the child's.**
//!
//! A pane's live `InputModes` belong to the program running in it: DECCKM
//! decides whether an arrow key is `ESC [ A` or `ESC O A`, the Kitty keyboard protocol's
//! disambiguate flag rewrites every `Ctrl` combination as a `CSI u` code, LNM decides whether
//! Return is `CR` or `CR LF`. The TUI's own terminal is in whatever modes the TUI put it in, which
//! are nobody's authority over the child. Forwarding raw bytes would deliver the wrong encoding
//! whenever the two differ — and would do it silently, because wrong bytes are still bytes.
//!
//! So the client reports the semantic edge and the host encodes it against the mode it owns. That
//! is the same discipline `sprag_host::wire`'s `MOUSE_ACTION` and `PASTE_ACTION` already state for
//! the GUI, and it is why an arrow key typed into `sprag-tui` reaches `vim` correctly even though
//! this client has never heard of DECCKM.
//!
//! # What the wire can carry, and what it cannot
//!
//! Not every [`KeyCode`] has a wire spelling, and a key with no spelling is [`None`] here rather
//! than a guess. Two groups are deliberately dropped:
//!
//! * **Modifier keys themselves** ([`KeyCode::is_modifier`]). A bare `Shift` press is not an input
//!   event a PTY has any encoding for; a terminal never reports one, and Windows' console decoder
//!   is the only thing in termwiz that can produce one at all.
//! * **Keys outside the terminal vocabulary** — media keys, browser keys, `Print`, `Sleep`. They
//!   have W3C names, but no terminal sends them and no child reads them.
//!
//! Keys this table DOES spell but [`sprag_input::encode`] cannot encode (F13 upward) are passed
//! through anyway. The client's job is to name the key; deciding what bytes a name becomes — or
//! that it becomes none — is the host's, and duplicating that policy here would mean a host that
//! learned F13 could not be reached without also shipping a new client.

use sprag_input::Modifiers;
use termwiz::input::{KeyCode, KeyEvent, Modifiers as LocalModifiers};

/// A keystroke in the vocabulary the wire carries: a W3C `KeyboardEvent.key` name and the four
/// modifiers `sprag-input` encodes against.
///
/// Produced only by [`wire_key`], so a value of this type is a key the wire can address — the
/// "unspellable key" case is [`None`] there and does not survive into a value here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WireKey {
    name: KeyName,
    mods: Modifiers,
}

impl WireKey {
    /// This key's W3C `KeyboardEvent.key` string.
    ///
    /// `scratch` is where a CHARACTER key's UTF-8 goes, and it is the reason this is not a plain
    /// getter: a character key's W3C name IS the character, so spelling it needs four bytes that
    /// outlive the call, and taking them from the caller keeps a keystroke off the heap. A named
    /// key ignores the buffer entirely.
    #[must_use]
    pub fn name<'a>(&'a self, scratch: &'a mut [u8; 4]) -> &'a str {
        match self.name {
            KeyName::Named(name) => name,
            KeyName::Char(c) => c.encode_utf8(scratch),
        }
    }

    /// The modifiers held with this key.
    #[must_use]
    pub fn mods(&self) -> Modifiers {
        self.mods
    }
}

/// A W3C key name, in the two shapes it comes in.
///
/// Private: it is an implementation detail of how [`WireKey`] avoids allocating, not a
/// distinction any caller has to make — [`WireKey::name`] erases it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeyName {
    /// One of the named keys, from the static table below.
    Named(&'static str),
    /// A character key, whose W3C name is the character itself.
    Char(char),
}

/// Decode one local key event into the wire's vocabulary, or [`None`] for a key the wire has no
/// spelling for (see the module docs for which, and why that is a drop rather than a guess).
#[must_use]
pub fn wire_key(event: &KeyEvent) -> Option<WireKey> {
    Some(WireKey {
        name: key_name(event.key)?,
        mods: modifiers(event.modifiers),
    })
}

/// The W3C `key` name for a local key code.
///
/// The aliasing pairs are not redundancy: termwiz carries `ApplicationUpArrow` / `KeyPadHome` /
/// `Numpad4` as SEPARATE codes because it also ENCODES keys (`KeyCode::encode`), where the
/// distinction picks a different escape sequence. W3C's `key` does not make that distinction — the
/// keypad's identity lives in `code`, not `key` — so both collapse onto one name here. The unix
/// [`InputParser`](termwiz::input::InputParser) does not currently produce the keypad codes at all;
/// they are mapped because they are SYNONYMS of codes it does produce, so a version of termwiz that
/// started emitting them would work rather than silently swallow keys.
fn key_name(code: KeyCode) -> Option<KeyName> {
    // A modifier key on its own is checked FIRST and by termwiz's own predicate rather than by
    // listing eight variants here — a list would go stale the moment termwiz named a ninth.
    if code.is_modifier() {
        return None;
    }
    Some(match code {
        KeyCode::Char(c) => KeyName::Char(c),

        KeyCode::Enter => KeyName::Named("Enter"),
        KeyCode::Tab => KeyName::Named("Tab"),
        KeyCode::Backspace => KeyName::Named("Backspace"),
        KeyCode::Escape => KeyName::Named("Escape"),

        KeyCode::UpArrow | KeyCode::ApplicationUpArrow => KeyName::Named("ArrowUp"),
        KeyCode::DownArrow | KeyCode::ApplicationDownArrow => KeyName::Named("ArrowDown"),
        KeyCode::RightArrow | KeyCode::ApplicationRightArrow => KeyName::Named("ArrowRight"),
        KeyCode::LeftArrow | KeyCode::ApplicationLeftArrow => KeyName::Named("ArrowLeft"),

        KeyCode::Home | KeyCode::KeyPadHome => KeyName::Named("Home"),
        KeyCode::End | KeyCode::KeyPadEnd => KeyName::Named("End"),
        KeyCode::PageUp | KeyCode::KeyPadPageUp => KeyName::Named("PageUp"),
        KeyCode::PageDown | KeyCode::KeyPadPageDown => KeyName::Named("PageDown"),
        KeyCode::Insert => KeyName::Named("Insert"),
        KeyCode::Delete => KeyName::Named("Delete"),

        KeyCode::Function(n) => KeyName::Named(FUNCTION_KEYS.get(usize::from(n).checked_sub(1)?)?),

        // The numeric keypad in its NUMERIC state: W3C names each of these by the character it
        // produces, so they are character keys that happened to arrive under a keypad code.
        KeyCode::Numpad0 => KeyName::Char('0'),
        KeyCode::Numpad1 => KeyName::Char('1'),
        KeyCode::Numpad2 => KeyName::Char('2'),
        KeyCode::Numpad3 => KeyName::Char('3'),
        KeyCode::Numpad4 => KeyName::Char('4'),
        KeyCode::Numpad5 => KeyName::Char('5'),
        KeyCode::Numpad6 => KeyName::Char('6'),
        KeyCode::Numpad7 => KeyName::Char('7'),
        KeyCode::Numpad8 => KeyName::Char('8'),
        KeyCode::Numpad9 => KeyName::Char('9'),
        KeyCode::Multiply => KeyName::Char('*'),
        KeyCode::Add => KeyName::Char('+'),
        KeyCode::Subtract => KeyName::Char('-'),
        KeyCode::Decimal => KeyName::Char('.'),
        KeyCode::Divide => KeyName::Char('/'),

        // Everything else has no terminal encoding to reach. `Separator` and `KeyPadBegin` are the
        // two near-misses and are dropped ON PURPOSE: the keypad separator's character is
        // locale-dependent (`,` or `.`, and this client cannot know which), and `KeyPadBegin`'s
        // W3C name is `Clear`, which no child reads and `sprag-input` does not encode.
        _ => return None,
    })
}

/// `F1` … `F24`, indexed by `n - 1`.
///
/// Spelled out rather than formatted so a function key costs no allocation, and carried past F12
/// — where [`sprag_input::encode`] currently stops — for the reason the module docs give: naming
/// the key is this crate's job, encoding it is the host's.
const FUNCTION_KEYS: [&str; 24] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15",
    "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

/// The local terminal's modifier bits as the wire's four.
///
/// Only the four BASE flags are read. termwiz also carries positional variants (`LEFT_CTRL`,
/// `RIGHT_SHIFT`, …) and virtual ones (`LEADER`, `ENHANCED_KEY`), but the positional bits are
/// supplemental — `Modifiers::remove_positional_mods` exists precisely because they accompany the
/// base flag rather than replace it — so `contains` sees a side-specific `Ctrl` as `Ctrl`. The
/// virtual bits are wezterm's own key-table machinery and mean nothing to a PTY.
fn modifiers(mods: LocalModifiers) -> Modifiers {
    Modifiers {
        ctrl: mods.contains(LocalModifiers::CTRL),
        alt: mods.contains(LocalModifiers::ALT),
        shift: mods.contains(LocalModifiers::SHIFT),
        sup: mods.contains(LocalModifiers::SUPER),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a hand-built event — for the cases a real terminal cannot produce (the keypad codes,
    /// the modifier keys). Everything a terminal CAN produce is tested in `tests/key_round_trip.rs`
    /// against the real parser instead, because a hand-built event is one this author invented.
    fn decoded(key: KeyCode, mods: LocalModifiers) -> Option<(String, Modifiers)> {
        let key = wire_key(&KeyEvent {
            key,
            modifiers: mods,
        })?;
        let mut scratch = [0u8; 4];
        Some((key.name(&mut scratch).to_owned(), key.mods()))
    }

    #[test]
    fn a_modifier_key_alone_has_no_wire_spelling() {
        for code in [
            KeyCode::Shift,
            KeyCode::LeftShift,
            KeyCode::Control,
            KeyCode::RightControl,
            KeyCode::Alt,
            KeyCode::Super,
            KeyCode::Hyper,
            KeyCode::Meta,
        ] {
            assert_eq!(decoded(code, LocalModifiers::NONE), None, "{code:?}");
        }
    }

    #[test]
    fn keys_outside_the_terminal_vocabulary_are_dropped() {
        for code in [
            KeyCode::VolumeUp,
            KeyCode::BrowserHome,
            KeyCode::Print,
            KeyCode::Sleep,
            KeyCode::Separator,
            KeyCode::KeyPadBegin,
        ] {
            assert_eq!(decoded(code, LocalModifiers::NONE), None, "{code:?}");
        }
    }

    /// The keypad's codes collapse onto the same W3C names their main-keyboard peers use — the
    /// distinction W3C keeps in `code`, not `key`.
    #[test]
    fn the_keypad_collapses_onto_its_main_keyboard_names() {
        let pairs = [
            (KeyCode::ApplicationUpArrow, "ArrowUp"),
            (KeyCode::KeyPadHome, "Home"),
            (KeyCode::KeyPadEnd, "End"),
            (KeyCode::KeyPadPageUp, "PageUp"),
            (KeyCode::KeyPadPageDown, "PageDown"),
            (KeyCode::Numpad7, "7"),
            (KeyCode::Add, "+"),
            (KeyCode::Divide, "/"),
        ];
        for (code, name) in pairs {
            assert_eq!(
                decoded(code, LocalModifiers::NONE).map(|(name, _)| name),
                Some(name.to_owned()),
                "{code:?}",
            );
        }
    }

    /// Function keys are named past the point `sprag-input` can encode them, and stop at the point
    /// termwiz can report them.
    #[test]
    fn function_keys_are_named_to_f24_and_no_further() {
        assert_eq!(
            decoded(KeyCode::Function(1), LocalModifiers::NONE).map(|(name, _)| name),
            Some("F1".to_owned()),
        );
        // Past `sprag-input`'s table, deliberately: the host decides what a name encodes to.
        assert_eq!(
            decoded(KeyCode::Function(13), LocalModifiers::NONE).map(|(name, _)| name),
            Some("F13".to_owned()),
        );
        assert_eq!(
            decoded(KeyCode::Function(24), LocalModifiers::NONE).map(|(name, _)| name),
            Some("F24".to_owned()),
        );
        // `Function(0)` is not a key; `checked_sub` is what keeps it from indexing F24.
        assert_eq!(decoded(KeyCode::Function(0), LocalModifiers::NONE), None);
        assert_eq!(decoded(KeyCode::Function(25), LocalModifiers::NONE), None);
    }

    /// A positional modifier bit reads as the modifier it is a side of.
    ///
    /// The revert-proof: replace `contains` with an equality test against the base flag and this
    /// fails, because the positional bit is still set alongside it.
    #[test]
    fn a_side_specific_modifier_still_reads_as_that_modifier() {
        let (name, mods) = decoded(
            KeyCode::Char('a'),
            LocalModifiers::CTRL | LocalModifiers::LEFT_CTRL,
        )
        .expect("a character key decodes");
        assert_eq!(name, "a");
        assert!(mods.ctrl, "a left Ctrl is a Ctrl");
        assert!(!mods.alt && !mods.shift && !mods.sup);
    }

    /// The virtual bits wezterm uses for its own key tables are not modifiers a PTY has heard of.
    #[test]
    fn the_virtual_modifier_bits_do_not_reach_the_wire() {
        let (_, mods) = decoded(
            KeyCode::Char('a'),
            LocalModifiers::LEADER | LocalModifiers::ENHANCED_KEY,
        )
        .expect("a character key decodes");
        assert_eq!(mods, Modifiers::default());
    }
}
