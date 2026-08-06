//! The slice-3 decoder verification: what a terminal SENDS, back out of the encoder that receives
//! it.
//!
//! `bytes -> termwiz::InputParser -> sprag_tui::wire_key -> sprag_input::encode -> bytes`, with no
//! terminal, no daemon and no hand-built key event anywhere in it. Both ends of that chain are the
//! real ones a keystroke crosses in production: the parser is the same one `sprag-tui`'s event loop
//! reads its terminal through, and the encoder is the same one the daemon runs at the PTY boundary.
//!
//! # Why the bytes are the input and not a `KeyEvent`
//!
//! [`wire_key`]'s own unit tests build `KeyEvent`s directly, which is the only way to reach the
//! codes a unix terminal cannot produce (the keypad, the bare modifiers). For everything a terminal
//! CAN produce that is the weaker test, for the reason slice 2 learned the hard way: a hand-built
//! event is one the author invented, and it agrees with them. `ESC O A` is `ArrowUp` because
//! termwiz says so, not because this file says so — and if termwiz ever changed its mind, these
//! tests would move with it while a hand-built battery would keep passing.
//!
//! # What the round trip proves, and what it deliberately does not
//!
//! An identity means the client did not CORRUPT the keystroke. It does not mean the client is a
//! passthrough — [`the_child_s_mode_decides_the_bytes_not_the_client_s`] is the test that shows the
//! two are different claims, and the design's whole reason for decoding at all.

use sprag_input::encode;
use sprag_tui::wire_key;
use sprag_vt::{InputModes, KittyKeyboardFlags};
use termwiz::input::{InputEvent, InputParser};

/// Run `bytes` through the whole chain and return what a child would receive, or `None` if any key
/// in them has no wire spelling or no encoding.
///
/// `maybe_more = false` says the sequence is complete, which is what a terminal read that returned
/// these bytes means. It matters for exactly one case and an important one: a lone `ESC` resolves
/// as `Escape` rather than sitting in the parser waiting to see whether an Alt-combination follows.
fn round_trip(bytes: &[u8], modes: InputModes) -> Option<Vec<u8>> {
    let mut events = Vec::new();
    InputParser::new().parse(bytes, |event| events.push(event), false);
    let mut out = Vec::new();
    for event in &events {
        let InputEvent::Key(event) = event else {
            panic!("{bytes:?} produced something that is not a key: {event:?}");
        };
        let key = wire_key(event)?;
        let mut scratch = [0u8; 4];
        out.extend_from_slice(&encode(key.name(&mut scratch), key.mods(), modes)?);
    }
    Some(out)
}

/// The bytes a child receives for `bytes` typed at a terminal in the ordinary modes, or a panic
/// naming the key that fell through.
fn typed(bytes: &[u8]) -> Vec<u8> {
    round_trip(bytes, InputModes::default())
        .unwrap_or_else(|| panic!("{bytes:?} has no wire spelling"))
}

/// Every sequence an unremarkable terminal emits, and the child receives it unchanged.
///
/// The corpus is the vocabulary `sprag-input` documents it encodes, entered from the other side:
/// text, the C0 control codes, the cursor and edit keys, the `~`-terminated keypad block, `SS3`
/// function keys, and the PC-style modifier parameter on each family that takes one.
#[test]
fn what_a_terminal_sends_arrives_unchanged() {
    let corpus: &[&[u8]] = &[
        // Text, including a multi-byte cluster — the case a byte-oriented decoder would split.
        b"a",
        b"Z",
        b"~",
        "\u{ac00}".as_bytes(), // 가
        // C0: Ctrl with a letter, at both ends of the range.
        &[0x01], // Ctrl-A
        &[0x1a], // Ctrl-Z
        // The named keys that ARE control bytes.
        &[0x09], // Tab
        &[0x0d], // Enter
        &[0x1b], // Escape
        &[0x7f], // Backspace
        // metaSendsEscape, in both the cases termwiz reaches by different routes: the uppercase
        // pair is a keymap entry, the lowercase one falls out of its Escape-then-key state machine.
        b"\x1bA",
        b"\x1ba",
        // Cursor and positional keys, `CSI` form.
        b"\x1b[A",
        b"\x1b[B",
        b"\x1b[C",
        b"\x1b[D",
        b"\x1b[H",
        b"\x1b[F",
        // ...and with the PC-style modifier parameter: Ctrl, Shift, Alt.
        b"\x1b[1;5A",
        b"\x1b[1;2D",
        b"\x1b[1;3B",
        // Back-tab, the one key whose Shift form is a different sequence rather than a parameter.
        b"\x1b[Z",
        // The `~`-terminated edit / keypad block, plain and modified.
        b"\x1b[2~",
        b"\x1b[3~",
        b"\x1b[5~",
        b"\x1b[6~",
        b"\x1b[3;5~",
        // F1-F4 (`SS3`) and F5-F12 (`~`), the two function-key families.
        b"\x1bOP",
        b"\x1bOQ",
        b"\x1bOR",
        b"\x1bOS",
        b"\x1b[15~",
        b"\x1b[17~",
        b"\x1b[18~",
        b"\x1b[19~",
        b"\x1b[20~",
        b"\x1b[21~",
        b"\x1b[23~",
        b"\x1b[24~",
        // A modified F1: the family that switches introducer when a modifier is held.
        b"\x1b[1;5P",
    ];
    for bytes in corpus {
        assert_eq!(
            typed(bytes),
            *bytes,
            "{bytes:?} did not survive the round trip",
        );
    }
}

/// The four sequences that come back DIFFERENT, each because two byte spellings name one key.
///
/// This is the test that would catch a decoder mapping a key onto the wrong name: the identity
/// battery above cannot, because an identity is also what a byte passthrough would produce. Here
/// the expected output is not the input, so it can only be right if the NAME in the middle was.
#[test]
fn the_aliases_normalise_onto_one_spelling() {
    let pairs: &[(&[u8], &[u8], &str)] = &[
        (
            b"\n",
            b"\r",
            "LF and CR are both Return; which one a terminal sends is its line discipline's \
             choice, and which one the child should receive is the child's LNM mode — so the \
             host, which owns that mode, is the right place for the decision",
        ),
        (
            &[0x08],
            &[0x7f],
            "BS and DEL are both Backspace, a split terminals have carried since the VT100; the \
             child's `stty erase` decides which it wants, and the host encodes the modern one",
        ),
        (
            b"\x1b[1~",
            b"\x1b[H",
            "the Linux console spells Home as a keypad code and xterm spells it as a cursor key",
        ),
        (b"\x1b[7~", b"\x1b[H", "and rxvt spells it as a third thing"),
    ];
    for (sent, received, why) in pairs {
        assert_eq!(typed(sent), *received, "{sent:?}: {why}");
    }
}

/// **The design's claim, as a test.** The same keystroke encodes differently for different panes,
/// because the mode that decides belongs to the CHILD and not to this client's terminal.
///
/// A byte passthrough is exactly what would fail here — it would deliver `ESC [ A` to a `vim` that
/// asked for `ESC O A`, and `ESC O A` to a shell that did not, whichever mode the user's local
/// terminal happened to be in. Both directions are asserted, because a decoder that ignored the
/// mode entirely would still pass one of them.
#[test]
fn the_child_s_mode_decides_the_bytes_not_the_client_s() {
    let application = InputModes {
        application_cursor_keys: true,
        ..InputModes::default()
    };

    // A terminal in the ORDINARY mode types an arrow; a child that asked for application cursor
    // keys receives the `SS3` form.
    assert_eq!(
        round_trip(b"\x1b[A", application).expect("a cursor key encodes"),
        b"\x1bOA",
    );
    // ...and the reverse: a terminal in APPLICATION mode types the same arrow, and a child that
    // did not ask for it receives the plain `CSI` form.
    assert_eq!(typed(b"\x1bOA"), b"\x1b[A");
}

/// The same argument on a second mode, so the claim rests on the encoder's rule rather than on one
/// lucky table entry: under the Kitty keyboard protocol's disambiguate flag a `Ctrl` combination
/// becomes an unambiguous `CSI u` code, and the client — which has never heard of the flag —
/// produces it anyway because it sent a NAME.
#[test]
fn a_kitty_pane_gets_kitty_bytes_from_the_same_keystroke() {
    let kitty = InputModes {
        kitty_keyboard: KittyKeyboardFlags::from_bits(KittyKeyboardFlags::DISAMBIGUATE),
        ..InputModes::default()
    };
    // Ctrl-A: the legacy byte 0x01 in, a `CSI 97 ; 5 u` code out.
    assert_eq!(
        round_trip(&[0x01], kitty).expect("a control key encodes"),
        b"\x1b[97;5u",
    );
    // The same bytes, an ordinary pane, the legacy encoding — the control.
    assert_eq!(typed(&[0x01]), &[0x01]);
}

/// **MEASURED, not assumed: this termwiz has no focus event, and a focus report comes back as two
/// KEYSTROKES** — which is why `sprag_tui::focus` exists and why asking a terminal for DEC private
/// mode 1004 without it would type garbage into somebody's shell.
///
/// `InputEvent` has no focus variant at all (checked in `termwiz-0.23.3/src/input.rs`), so `CSI I` /
/// `CSI O` fall through the parser's keymap and are resolved by its Meta rule: an `ESC` with more
/// data behind it becomes an ALT modifier on the next key. What a person would see, on a client that
/// enabled the mode and routed what it read, is `^[[I` appearing at their prompt every time they
/// switched windows.
///
/// This is a claim about a DEPENDENCY, so it is pinned rather than trusted: a termwiz that grew a
/// focus event would fail here, which is the notice sprag needs to delete its own decoder rather than
/// keep a second one running beside it.
///
/// The last case is the one the decoder's whole discriminator rests on: a report followed by ordinary
/// text arrives as ONE parse, so both halves are available with no further read.
#[test]
fn a_host_terminals_focus_report_arrives_as_two_keystrokes() {
    let bracket = InputEvent::Key(termwiz::input::KeyEvent {
        key: termwiz::input::KeyCode::Char('['),
        modifiers: termwiz::input::Modifiers::ALT,
    });
    let letter = |letter: char| {
        InputEvent::Key(termwiz::input::KeyEvent {
            key: termwiz::input::KeyCode::Char(letter),
            modifiers: termwiz::input::Modifiers::NONE,
        })
    };
    for (bytes, second) in [(&b"\x1b[I"[..], 'I'), (&b"\x1b[O"[..], 'O')] {
        let mut got = Vec::new();
        InputParser::new().parse(bytes, |event| got.push(event), false);
        assert_eq!(
            got,
            vec![bracket.clone(), letter(second)],
            "{bytes:?} must be the pair sprag_tui::focus decodes",
        );
        // ...and sprag's decoder reads that pair back as the report it was.
        assert!(sprag_tui::focus::opens_report(&got[0]));
        assert_eq!(
            sprag_tui::focus::edge(&got[0], got.get(1)),
            Some(match second {
                'I' => sprag_tui::focus::Person::Here,
                _ => sprag_tui::focus::Person::Away,
            }),
        );
    }

    // A report with text behind it: one parse, every event queued — the property that lets a
    // zero-wait read-ahead tell a terminal's report from a person's two keystrokes.
    let mut mixed = Vec::new();
    InputParser::new().parse(b"\x1b[Oab", |event| mixed.push(event), false);
    assert_eq!(
        mixed,
        vec![bracket, letter('O'), letter('a'), letter('b')],
        "a report and the keys behind it arrive from ONE parse",
    );
}
