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

use sprag_vt::{InputModes, MouseEncoding};

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

    /// The modifier bits a mouse report packs directly into the button code: Shift = 4, Alt/Meta =
    /// 8, Ctrl = 16 (a different layout from the key [`xterm_param`](Self::xterm_param) parameter).
    /// Mouse reports have no encoding for the super/logo key, so it does not contribute.
    fn mouse_bits(self) -> u8 {
        (u8::from(self.shift) << 2) | (u8::from(self.alt) << 3) | (u8::from(self.ctrl) << 4)
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
    // LNM (ANSI mode 20): under new-line mode an UNMODIFIED Return transmits CR+LF instead of a bare
    // CR. Checked before the protocol branch so it applies whichever key encoding is active; only the
    // plain Return is affected — modified Enter combos are a post-LNM extension owned by the legacy /
    // Kitty paths below.
    if modes.newline_mode && key == "Enter" && !mods.any() {
        return Some(vec![0x0d, 0x0a]);
    }
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

/// The W3C `KeyboardEvent.key` names this crate's vocabulary spells — the multi-character half of
/// it, since a character key's name is the character itself.
///
/// # Why the list is here and not at either end that uses it
///
/// Two places need to know whether a string names a key, and neither may own the answer. A CLIENT
/// decodes its own keyboard into these names (`sprag-tui`'s `wire_key`, pinion's `KeyboardEvent`),
/// and [`encode`] turns them back into bytes — but a THIRD reader arrived with the keymap: a user's
/// `config.toml` binds a key by NAME, and a name nothing can ever produce has to be refused when the
/// file is read rather than left as a binding that silently never fires. A list in the keymap would
/// be a second vocabulary that drifts from this one; a list in a client would be that client's.
///
/// # It is deliberately WIDER than what [`encode`] accepts
///
/// `F13` upward are named here and encode to nothing. That is the same asymmetry `sprag-tui`'s key
/// decoder already documents from the other side: naming a key is the vocabulary's job, deciding
/// what bytes a name becomes — or that it becomes none — is the encoder's. A binding to `F13` is
/// therefore legitimate (it addresses a CLIENT command, which never reaches a PTY) even though
/// sending `F13` to a pane is not.
pub const NAMED_KEYS: &[&str] = &[
    "Enter",
    "Tab",
    "Backspace",
    "Escape",
    // The spacebar's W3C `key` is the single space, so `" "` is already a character key here.
    // `Space` is its `code`-style spelling, accepted by `encode_named` because an IME delivers it
    // that way — and the ONLY spelling available to a config file, where a lone space is invisible.
    "Space",
    "ArrowUp",
    "ArrowDown",
    "ArrowRight",
    "ArrowLeft",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Insert",
    "Delete",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "F13",
    "F14",
    "F15",
    "F16",
    "F17",
    "F18",
    "F19",
    "F20",
    "F21",
    "F22",
    "F23",
    "F24",
];

/// Whether `key` names a key in the wire's vocabulary — one of [`NAMED_KEYS`], or any single
/// Unicode scalar (whose W3C name IS the character).
///
/// The two-branch shape is [`encode`]'s own: it splits on exactly this test before choosing between
/// the character path and the named path, so a string this accepts is one `encode` will route
/// somewhere rather than reject for being unspellable.
#[must_use]
pub fn is_key_name(key: &str) -> bool {
    let mut chars = key.chars();
    if chars.next().is_some() && chars.next().is_none() {
        return true;
    }
    NAMED_KEYS.contains(&key)
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

// ----- Mouse reporting (the DECSET 1000/1002/1003 tracking modes, X10 + SGR 1006 encodings) -----
//
// A mouse report flows FROM the terminal TO the child: when the child has enabled a tracking mode,
// a pointer event over the pane is serialized here and written to the PTY. Encoding is sprag's
// responsibility (the same R2.6 boundary as keys), so the report bytes are built here rather than in
// any display client — the client supplies only a semantic [`MouseInput`] (a cell + a button edge).

sprag_vt::closed_set! {
    /// A pointer button in a mouse report. Wheel steps are reported as pseudo-buttons (xterm's
    /// model); [`MouseButton::None`] is the "no button" used for a bare motion event under
    /// any-event tracking.
    ///
    /// A [`closed_set!`](sprag_vt::closed_set!) because its WIRE VOCABULARY is published
    /// ([`WIRE_WORDS`](Self::WIRE_WORDS)) — see [`wire_str`](Self::wire_str) for the two readers
    /// that used to spell it separately.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum MouseButton {
        /// The primary (left) button — report button 0.
        Left,
        /// The middle button — report button 1.
        Middle,
        /// The secondary (right) button — report button 2.
        Right,
        /// Wheel scrolled up / away — xterm pseudo-button 64.
        WheelUp,
        /// Wheel scrolled down / toward — xterm pseudo-button 65.
        WheelDown,
        /// Horizontal wheel, xterm pseudo-button 66 (its button 6).
        ///
        /// # The NAME is a reading; the report is not
        ///
        /// xterm's ctlseqs define buttons 6 and 7 as pseudo-buttons and leave which way each points
        /// to convention, and these names follow the common one. **Nothing in sprag depends on the
        /// reading being right**: a display client observes its own terminal's direction flag and
        /// this encoder reproduces the same button number, so a swapped name would be a wrong LABEL
        /// in this file and never a wrong report at a child. The place it would matter is a client
        /// that SYNTHESISED a horizontal scroll from something other than a horizontal wheel, and
        /// none does.
        WheelLeft,
        /// Horizontal wheel the other way — xterm pseudo-button 67 (its button 7). See
        /// [`MouseButton::WheelLeft`] for why the name is a reading and the report is not.
        WheelRight,
        /// No button held — the "button" of a bare motion event (any-event tracking).
        None,
    }
}

impl MouseButton {
    /// This button's word on the pane-input `mouse` action.
    ///
    /// # ⚠⚠ This vocabulary was spelled TWICE, in two crates
    ///
    /// The display client encoded it (`sprag_client`'s `mouse_button_wire`) and the host decoded it
    /// (`parse_mouse_args`), as two independent hand-written matches over one set of eight words —
    /// each documented as the other's "twin", with nothing holding them together but a unit test
    /// that named four of the eight. A word renamed on either side would have made that button
    /// unsendable, and both crates' suites would have stayed green.
    ///
    /// So the spelling lives HERE, on the type both sides already share, and both read through it.
    /// The publication does too ([`WIRE_WORDS`](Self::WIRE_WORDS)), which is what lets the pane
    /// surface tell a client what a `button` may be — a client that had to know these words out of
    /// band before.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
            Self::WheelUp => "wheelup",
            Self::WheelDown => "wheeldown",
            Self::WheelLeft => "wheelleft",
            Self::WheelRight => "wheelright",
            Self::None => "none",
        }
    }

    /// The button a `mouse` action's `button` word names, or [`None`] for a word no button spells.
    ///
    /// Walks `ALL` through [`wire_str`](Self::wire_str) rather than re-listing the words, so this
    /// admits exactly what the type spells and what the wire publishes — the defect
    /// `AgentState::from_wire` was carrying at R352b, avoided here at birth.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }
}

sprag_vt::wire_words!(MouseButton: wire_str);

sprag_vt::closed_set! {
    /// The kind of pointer edge a report describes.
    ///
    /// A [`closed_set!`](sprag_vt::closed_set!) for [`MouseButton`]'s reason: the `mouse` action
    /// publishes this vocabulary, and it too was spelled once per side of the wire.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum MouseEventKind {
        /// A button went down.
        Press,
        /// A button came up.
        Release,
        /// The pointer moved while a button is held (button-event or any-event tracking).
        Drag,
        /// The pointer moved with no button held (any-event tracking only).
        Motion,
    }
}

impl MouseEventKind {
    /// This edge's word on the pane-input `mouse` action — the one spelling, for the reason
    /// [`MouseButton::wire_str`] gives.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Release => "release",
            Self::Drag => "drag",
            Self::Motion => "motion",
        }
    }

    /// The edge a `mouse` action's `kind` word names, or [`None`] for a word no edge spells.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }
}

sprag_vt::wire_words!(MouseEventKind: wire_str);

sprag_vt::closed_set! {
    /// WHICH EDGE of a key a `key` action reports — the `state` argument's two words.
    ///
    /// # Why the type exists at all
    ///
    /// The words lived inside the host's `parse_key_args` as two string literals, so the vocabulary
    /// had no definition anything could read: a client could not be told that `state` takes `down`
    /// or `up` and nothing else, and the pane surface published no grammar for `key` at all. That is
    /// exactly where `SplitDir` was before R352b, and the answer is the same one — a closed set, so
    /// the parser and the publication read one array.
    ///
    /// [`Up`](Self::Up) is a REPORTED edge that injects nothing: in the mode sprag drives, terminals
    /// emit no release, so the host accepts the edge and suppresses it rather than refusing a client
    /// that faithfully reports both halves of a keystroke.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum KeyEdge {
        /// The key went down — the edge that injects bytes, and the one a call means when it says
        /// nothing.
        Down,
        /// The key came up — accepted and suppressed (no terminal encoding exists for it).
        Up,
    }
}

impl KeyEdge {
    /// This edge's word in a `key` action's `state`.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
        }
    }

    /// The edge a `state` word names, or [`None`] for a word no edge spells (which the host reports
    /// as a malformed request rather than guessing at a press).
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }

    /// Whether this edge INJECTS anything — the question `inject_key` asks, on the type rather than
    /// at the call site, so a third edge would have to answer it.
    #[must_use]
    pub const fn injects(self) -> bool {
        matches!(self, Self::Down)
    }
}

sprag_vt::wire_words!(KeyEdge: wire_str);

/// A semantic pointer event addressed to a cell, before mode-gating and wire-encoding. A display
/// client converts a pixel position to a 0-based cell and fills this in; [`encode_mouse`] decides
/// whether the active tracking mode wants the event and, if so, serializes it. The coordinates are
/// 0-based cells (the wire report is 1-based — the encoder adds one).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MouseInput {
    /// The button (or wheel step, or [`MouseButton::None`] for a bare motion).
    pub button: MouseButton,
    /// The edge this event describes.
    pub kind: MouseEventKind,
    /// 0-based cell column.
    pub col: u16,
    /// 0-based cell row.
    pub row: u16,
    /// The keyboard modifiers held during the event (Shift / Alt / Ctrl reach the report).
    pub mods: Modifiers,
}

/// Encode a semantic mouse event into the PTY report bytes the focused child should receive, given
/// the terminal's current mouse [`InputModes`]. Returns `None` when the active
/// [`MouseProtocol`](sprag_vt::MouseProtocol) does not want this event — no tracking active, a bare
/// motion outside any-event tracking, or a drag outside button/any-event tracking — so a display
/// client's over-eager motion stream is filtered at this one authority (mirroring how the emulator's
/// `mode` is the authority for which modes are set). The encoding follows
/// [`InputModes::mouse_encoding`]: the legacy X10 byte form (coordinates clamped at 223) or the SGR
/// 1006 decimal form.
#[must_use]
pub fn encode_mouse(ev: MouseInput, modes: InputModes) -> Option<Vec<u8>> {
    let protocol = modes.mouse_protocol;
    // Gate the event against what the active tracking mode reports.
    let wanted = match ev.kind {
        MouseEventKind::Press | MouseEventKind::Release => protocol.is_active(),
        MouseEventKind::Drag => protocol.reports_drag(),
        MouseEventKind::Motion => protocol.reports_motion(),
    };
    if !wanted {
        return None;
    }
    let code = mouse_button_code(ev.button, ev.kind) | ev.mods.mouse_bits();
    Some(match modes.mouse_encoding {
        MouseEncoding::Sgr => encode_mouse_sgr(code, ev),
        MouseEncoding::X10 => encode_mouse_x10(code, ev),
    })
}

/// Encode a pane FOCUS change into the report bytes the child should receive, given the terminal's
/// [`InputModes`]. Returns `None` when the child has not enabled focus reporting (DEC private mode
/// 1004), so a display client may call it on every focus edge and let this one authority gate — the
/// same "mode authority at the boundary" as [`encode_mouse`] / key encoding. When enabled the report
/// is the fixed pair `ESC [ I` (focus IN / gained) or `ESC [ O` (focus OUT / lost) — no coordinates,
/// unaffected by the mouse encoding.
#[must_use]
pub fn encode_focus(focused: bool, modes: InputModes) -> Option<Vec<u8>> {
    if !modes.focus_tracking {
        return None;
    }
    Some(if focused {
        vec![ESC, b'[', b'I']
    } else {
        vec![ESC, b'[', b'O']
    })
}

/// The button portion of a report code (before the modifier bits): the low button bits (or the
/// wheel pseudo-button), plus the motion bit 32 for a drag or bare motion.
fn mouse_button_code(button: MouseButton, kind: MouseEventKind) -> u8 {
    let base = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    let motion = match kind {
        MouseEventKind::Drag | MouseEventKind::Motion => 32,
        MouseEventKind::Press | MouseEventKind::Release => 0,
    };
    base | motion
}

/// SGR 1006 form: `ESC [ < code ; col ; row (M|m)` — decimal, 1-based, `m` for a release (which,
/// unlike X10, keeps the released button in `code`). Coordinates are unbounded.
fn encode_mouse_sgr(code: u8, ev: MouseInput) -> Vec<u8> {
    let final_byte = if ev.kind == MouseEventKind::Release {
        'm'
    } else {
        'M'
    };
    format!(
        "\x1b[<{};{};{}{}",
        code,
        u32::from(ev.col) + 1,
        u32::from(ev.row) + 1,
        final_byte
    )
    .into_bytes()
}

/// X10 legacy form: `ESC [ M` + three `32 + value` bytes. A release does not carry which button (its
/// low button bits become 3, the modifier and motion bits are kept); a coordinate past 223 cannot be
/// represented and pins at the last byte (SGR has no such limit). 1-based coordinates.
fn encode_mouse_x10(code: u8, ev: MouseInput) -> Vec<u8> {
    let code = if ev.kind == MouseEventKind::Release {
        (code & !0b11) | 0b11
    } else {
        code
    };
    vec![
        ESC,
        b'[',
        b'M',
        x10_byte(u16::from(code)),
        x10_byte(ev.col.saturating_add(1)),
        x10_byte(ev.row.saturating_add(1)),
    ]
}

/// One byte of an X10 mouse report: `32 + value`, pinned at the 255 ceiling (the legacy form cannot
/// represent a value past 223).
fn x10_byte(value: u16) -> u8 {
    32u16.saturating_add(value).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(app_cursor: bool) -> InputModes {
        InputModes {
            application_cursor_keys: app_cursor,
            kitty_keyboard: sprag_vt::KittyKeyboardFlags::default(),
            ..InputModes::default()
        }
    }

    /// Input modes with the Kitty keyboard DISAMBIGUATE flag active.
    fn kitty_modes() -> InputModes {
        InputModes {
            application_cursor_keys: false,
            kitty_keyboard: sprag_vt::KittyKeyboardFlags::from_bits(
                sprag_vt::KittyKeyboardFlags::DISAMBIGUATE,
            ),
            ..InputModes::default()
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

    // ----- Mouse reporting -----

    use sprag_vt::MouseProtocol;

    /// Input modes with a mouse tracking protocol + encoding.
    fn mouse_modes(protocol: MouseProtocol, encoding: MouseEncoding) -> InputModes {
        InputModes {
            mouse_protocol: protocol,
            mouse_encoding: encoding,
            ..InputModes::default()
        }
    }

    /// A no-modifier mouse event at a cell.
    fn ev(button: MouseButton, kind: MouseEventKind, col: u16, row: u16) -> MouseInput {
        MouseInput {
            button,
            kind,
            col,
            row,
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn sgr_press_and_release_carry_the_button_and_1_based_cell() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::Sgr);
        // Left press at cell (col 4, row 2) -> code 0, 1-based (5, 3), final M.
        assert_eq!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Press, 4, 2), modes),
            Some(b"\x1b[<0;5;3M".to_vec()),
        );
        // Release keeps the button (SGR) and flips the final byte to m.
        assert_eq!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Release, 4, 2), modes),
            Some(b"\x1b[<0;5;3m".to_vec()),
        );
    }

    #[test]
    fn sgr_middle_and_right_buttons_are_codes_1_and_2() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::Sgr);
        assert_eq!(
            encode_mouse(ev(MouseButton::Middle, MouseEventKind::Press, 0, 0), modes),
            Some(b"\x1b[<1;1;1M".to_vec()),
        );
        assert_eq!(
            encode_mouse(ev(MouseButton::Right, MouseEventKind::Press, 0, 0), modes),
            Some(b"\x1b[<2;1;1M".to_vec()),
        );
    }

    #[test]
    fn x10_press_packs_32_plus_value_bytes_and_release_drops_the_button() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::X10);
        // Left press at (0,0): code 0, coords 1,1 -> bytes 32, 33, 33 after ESC [ M.
        assert_eq!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Press, 0, 0), modes),
            Some(vec![ESC, b'[', b'M', 32, 33, 33]),
        );
        // X10 release does not say which button: low bits become 3 -> code byte 32 + 3 = 35.
        assert_eq!(
            encode_mouse(ev(MouseButton::Right, MouseEventKind::Release, 0, 0), modes),
            Some(vec![ESC, b'[', b'M', 35, 33, 33]),
        );
    }

    #[test]
    fn x10_coordinate_past_223_pins_at_the_byte_ceiling() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::X10);
        // col 300 -> 32 + 301 = 333, clamped to 255 (the legacy form's documented limit).
        let bytes = encode_mouse(ev(MouseButton::Left, MouseEventKind::Press, 300, 0), modes)
            .expect("reported");
        assert_eq!(bytes[4], 255, "column clamps at the 255 ceiling");
        assert_eq!(bytes[5], 33, "row is unaffected");
    }

    #[test]
    fn modifiers_add_shift_4_alt_8_ctrl_16_to_the_code() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::Sgr);
        let ctrl = MouseInput {
            mods: CTRL,
            ..ev(MouseButton::Left, MouseEventKind::Press, 0, 0)
        };
        assert_eq!(encode_mouse(ctrl, modes), Some(b"\x1b[<16;1;1M".to_vec()));
        let shift = MouseInput {
            mods: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            ..ev(MouseButton::Left, MouseEventKind::Press, 0, 0)
        };
        assert_eq!(encode_mouse(shift, modes), Some(b"\x1b[<4;1;1M".to_vec()));
    }

    #[test]
    fn no_reporting_when_no_tracking_mode_is_active() {
        let modes = mouse_modes(MouseProtocol::None, MouseEncoding::Sgr);
        assert_eq!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Press, 0, 0), modes),
            None,
            "a press is dropped when no tracking mode is set",
        );
    }

    #[test]
    fn click_tracking_reports_presses_but_not_drag_or_motion() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::Sgr);
        assert!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Press, 0, 0), modes).is_some(),
            "press reported",
        );
        assert_eq!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Drag, 0, 0), modes),
            None,
            "1000 does not report drag",
        );
        assert_eq!(
            encode_mouse(ev(MouseButton::None, MouseEventKind::Motion, 0, 0), modes),
            None,
            "1000 does not report motion",
        );
    }

    #[test]
    fn button_event_reports_drag_and_any_event_reports_motion() {
        let sgr = MouseEncoding::Sgr;
        // 1002 reports a drag (motion bit 32) but still not a bare motion.
        let button = mouse_modes(MouseProtocol::ButtonEvent, sgr);
        assert_eq!(
            encode_mouse(ev(MouseButton::Left, MouseEventKind::Drag, 1, 1), button),
            Some(b"\x1b[<32;2;2M".to_vec()),
        );
        assert_eq!(
            encode_mouse(ev(MouseButton::None, MouseEventKind::Motion, 1, 1), button),
            None,
            "1002 does not report a bare motion",
        );
        // 1003 reports a bare motion: no button -> low bits 3, motion bit 32 -> code 35.
        let any = mouse_modes(MouseProtocol::AnyEvent, sgr);
        assert_eq!(
            encode_mouse(ev(MouseButton::None, MouseEventKind::Motion, 1, 1), any),
            Some(b"\x1b[<35;2;2M".to_vec()),
        );
    }

    #[test]
    fn wheel_steps_report_as_pseudo_buttons_64_and_65() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::Sgr);
        assert_eq!(
            encode_mouse(ev(MouseButton::WheelUp, MouseEventKind::Press, 0, 0), modes),
            Some(b"\x1b[<64;1;1M".to_vec()),
        );
        assert_eq!(
            encode_mouse(
                ev(MouseButton::WheelDown, MouseEventKind::Press, 0, 0),
                modes
            ),
            Some(b"\x1b[<65;1;1M".to_vec()),
        );
    }

    #[test]
    fn focus_reports_gate_on_1004_and_use_the_fixed_pair() {
        // Off by default: no report either way.
        let off = InputModes::default();
        assert_eq!(encode_focus(true, off), None);
        assert_eq!(encode_focus(false, off), None);
        // With 1004 on: ESC [ I on focus in, ESC [ O on focus out.
        let on = InputModes {
            focus_tracking: true,
            ..InputModes::default()
        };
        assert_eq!(encode_focus(true, on), Some(b"\x1b[I".to_vec()));
        assert_eq!(encode_focus(false, on), Some(b"\x1b[O".to_vec()));
    }

    #[test]
    fn a_wheel_step_carries_held_modifiers_into_the_button_code() {
        let modes = mouse_modes(MouseProtocol::Click, MouseEncoding::Sgr);
        // Ctrl+wheel-up: base 64 | ctrl bit 16 = 80 (a Ctrl+scroll a canvas-zooming app reads).
        let ctrl_wheel = MouseInput {
            mods: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            ..ev(MouseButton::WheelUp, MouseEventKind::Press, 0, 0)
        };
        assert_eq!(
            encode_mouse(ctrl_wheel, modes),
            Some(b"\x1b[<80;1;1M".to_vec()),
        );
    }

    /// Input modes with LNM (new-line mode) active.
    fn newline_modes(kitty: bool) -> InputModes {
        InputModes {
            kitty_keyboard: if kitty {
                sprag_vt::KittyKeyboardFlags::from_bits(sprag_vt::KittyKeyboardFlags::DISAMBIGUATE)
            } else {
                sprag_vt::KittyKeyboardFlags::default()
            },
            newline_mode: true,
            ..InputModes::default()
        }
    }

    #[test]
    fn newline_mode_enter_transmits_cr_lf() {
        // LNM (mode 20): an unmodified Return sends CR+LF instead of a bare CR.
        assert_eq!(
            encode("Enter", Modifiers::default(), newline_modes(false)),
            Some(b"\x0d\x0a".to_vec())
        );
    }

    #[test]
    fn newline_mode_applies_under_the_kitty_protocol_too() {
        // The LNM translation is checked before the protocol branch, so a plain Return sends CR+LF
        // even under the Kitty keyboard protocol.
        assert_eq!(
            encode("Enter", Modifiers::default(), newline_modes(true)),
            Some(b"\x0d\x0a".to_vec())
        );
    }

    #[test]
    fn newline_mode_leaves_a_modified_enter_alone() {
        // LNM is defined for the plain Return; a modified Enter keeps its combo encoding (Ctrl+Enter
        // on the legacy path stays a bare CR, not CR+LF).
        assert_eq!(
            encode("Enter", CTRL, newline_modes(false)),
            Some(b"\x0d".to_vec())
        );
    }

    /// **The drift guard between the vocabulary and the encoder**, stated as the exact boundary
    /// rather than as a one-way containment: every [`NAMED_KEYS`] entry encodes EXCEPT the function
    /// keys past F12, which are named for a client to bind and have no PTY encoding.
    ///
    /// Asserted as an equality so it fails from either side. Adding a name to the vocabulary that
    /// nothing encodes fails it; teaching `encode` F13 fails it too — and that second failure is the
    /// point, since it is the one a containment test would have let through silently.
    #[test]
    fn the_vocabulary_and_the_encoder_agree_on_exactly_the_function_keys_past_f12() {
        let unencodable: Vec<&str> = NAMED_KEYS
            .iter()
            .copied()
            .filter(|key| encode(key, Modifiers::default(), modes(false)).is_none())
            .collect();
        assert_eq!(
            unencodable,
            vec![
                "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24"
            ],
            "the vocabulary is wider than the encoder by exactly F13-F24 and nothing else",
        );
    }

    /// A character key needs no table: its W3C name IS the character, so the vocabulary accepts any
    /// single scalar and rejects a multi-character string that is not a named key.
    ///
    /// The empty string is the case worth pinning — `chars().next()` on it is `None`, and a
    /// vocabulary that accepted `""` would let a config bind a key no keyboard can press.
    #[test]
    fn the_vocabulary_accepts_any_single_scalar_and_no_invented_name() {
        for key in ["a", "%", "\"", " ", "가", "☃"] {
            assert!(is_key_name(key), "{key:?} is a character key");
        }
        assert!(!is_key_name(""), "the empty string names no key");
        // tmux's OWN named keys, which sprag deliberately does not adopt: it spells these
        // `ArrowUp` / `Backspace` / `Delete` / `PageDown`, and accepting both would be two
        // vocabularies with a mapping table between them.
        for key in ["Up", "BSpace", "DC", "IC", "NPage", "PgDn"] {
            assert!(!is_key_name(key), "{key:?} is tmux's spelling, not sprag's");
        }
    }
    /// ⚠⚠ **ONE SPELLING, AND `from_wire` IS ITS INVERSE OVER THE WHOLE TYPE** — the property that
    /// replaces two hand-written matches in two crates.
    ///
    /// # Why this is worth a test when `wire_words!` already projects the array
    ///
    /// The macro guarantees the PUBLISHED list is the type's own words. It says nothing about the
    /// PARSER: `from_wire` could have been written as a second match — which is exactly the defect
    /// `AgentState` was carrying at R352b, a `from_wire` re-listing the words while its own doc
    /// claimed one definition. So the claim here is the round trip over `ALL`, plus the refusal of a
    /// word no member spells, plus the length coming from the type rather than from a number.
    #[test]
    fn every_wire_vocabulary_round_trips_through_one_spelling() {
        for button in MouseButton::ALL {
            assert_eq!(MouseButton::from_wire(button.wire_str()), Some(button));
        }
        for kind in MouseEventKind::ALL {
            assert_eq!(MouseEventKind::from_wire(kind.wire_str()), Some(kind));
        }
        for edge in KeyEdge::ALL {
            assert_eq!(KeyEdge::from_wire(edge.wire_str()), Some(edge));
        }
        assert_eq!(MouseButton::WIRE_WORDS.len(), MouseButton::ALL.len());
        assert_eq!(MouseEventKind::WIRE_WORDS.len(), MouseEventKind::ALL.len());
        assert_eq!(KeyEdge::WIRE_WORDS.len(), KeyEdge::ALL.len());

        // A word outside the vocabulary is refused rather than folded onto a default — which is what
        // makes the published list a CONSTRAINT and not documentation beside the parser.
        for stranger in ["", "LEFT", "wheel", "sideways", "pressed", "downwards"] {
            assert_eq!(MouseButton::from_wire(stranger), None, "{stranger:?}");
            assert_eq!(MouseEventKind::from_wire(stranger), None, "{stranger:?}");
            assert_eq!(KeyEdge::from_wire(stranger), None, "{stranger:?}");
        }
    }

    /// The edge that INJECTS is a property of the type, and exactly one edge has it.
    ///
    /// ⚠ `up` is accepted and writes nothing — terminals emit no release in the mode sprag drives —
    /// so "which edge does something" is a question the type answers rather than a `match` at the
    /// call site. A third edge would have to answer it too, which is the point.
    #[test]
    fn exactly_one_key_edge_injects() {
        let injecting: Vec<KeyEdge> = KeyEdge::ALL
            .into_iter()
            .filter(|edge| edge.injects())
            .collect();
        assert_eq!(injecting, [KeyEdge::Down]);
    }
}
