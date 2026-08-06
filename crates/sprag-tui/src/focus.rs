//! Whether the PERSON is looking at the terminal this client borrowed.
//!
//! # The one fact this client could not learn, and why it needs it
//!
//! Everything else about this client's world arrives from the daemon: which panes exist, what they
//! are showing, who else is attached, what somebody asked it to say. This is the one fact only the
//! terminal knows — and the one that decides whether a message reaching the status row reaches a
//! PERSON. A row painted into a window nobody is looking at is R318's defect one layer out: the
//! words are there and nothing is obliged to see them.
//!
//! # `termwiz 0.23.3` has no focus event, MEASURED
//!
//! DEC private mode 1004 makes a terminal report `CSI I` when its window gains focus and `CSI O`
//! when it loses it. termwiz's [`InputEvent`] has no variant for either
//! (`a_host_terminals_focus_report_arrives_as_two_keystrokes` pins what it does instead), and
//! `UnixTerminal::poll_input` reads the tty and feeds its own parser with no seam in between — so
//! there is nowhere to intercept the BYTES. What comes out is the pair
//!
//! ```text
//! Key(Char('[') + ALT), Key(Char('I'))
//! ```
//!
//! because the parser resolves `ESC` followed by more data as a Meta-modified next key. Enabling
//! mode 1004 without this module would therefore TYPE `Alt-[` and `I` into whatever the person left
//! running in the focused pane, every time they switched windows. That is why the decode is not
//! optional garnish: it is what makes asking for the reports safe at all.
//!
//! # What separates a report from a person typing those two keys
//!
//! Nothing, in the bytes — `ESC [ I` is `ESC [ I` — so the discriminator is that a focus report is
//! ONE WRITE by the terminal and therefore arrives in ONE read. termwiz parses a read whole and
//! queues every event it found, and `poll_input` drains that queue before it polls anything: so a
//! second event that is available with NO WAIT came from the same read as the first, and a person's
//! second keystroke did not. [`opens_report`] arms a zero-wait read-ahead, [`edge`] resolves the
//! pair, and a pair that is not a report is routed as the two keystrokes it is.
//!
//! **The bound, stated rather than hoped**: a person who could land `Alt-[` and `I` in the same tty
//! read — inside the microseconds between `poll(2)` returning and `read(2)` running — would have
//! their keystrokes read as a focus change. Nothing in a human's hands does that. What CAN produce
//! it is a program pasting those bytes, which bracketed paste already separates
//! ([`InputEvent::Paste`]), and a terminal that split its own report across two reads, which
//! degrades the other way: the two keys are typed, once, into the pane. Both are bounded, neither
//! loses state, and the alternative — holding `Alt-[` until the NEXT keystroke, whenever that is —
//! would delay a real binding indefinitely to remove a case a person cannot reach.
//!
//! # Why this is not gated behind a capability check
//!
//! A terminal that does not implement mode 1004 sends no reports, so this decoder never fires and
//! the person is simply never known to have left. That is the honest degradation, and it is why
//! [`Person::Here`] is the starting value: a terminal reports a CHANGE, so silence means "nothing
//! has changed since you asked", not "unknown".

use termwiz::input::{InputEvent, KeyCode, KeyEvent, Modifiers};

/// Where the PERSON is — [`sprag_host::outward::Person`], re-exported here because this decoder is
/// what ANSWERS it for a terminal client.
///
/// The type moved to the host crate when `sprag-gui` gained an outward of its own: a window manager
/// answers the same question a DEC 1004 report does, and two spellings of *here* and *away* is the
/// duplication the option's own policy already avoids. What stays here is the terminal's way of
/// finding out.
pub use sprag_host::outward::Person;

/// Whether `event` could be the FIRST half of a focus report — the gate on the read-ahead.
///
/// `Alt-[` and nothing else, which is what termwiz makes of the `ESC [` a report opens with (see the
/// module docs for the measurement). Deliberately narrow: a caller reads ahead only for this exact
/// event, so every other keystroke is routed the instant it arrives and no key acquires latency.
#[must_use]
pub fn opens_report(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char('['),
            modifiers,
        }) if *modifiers == Modifiers::ALT
    )
}

/// Where the person is, if `opened` and `next` are a focus report — [`None`] when they are two
/// ordinary keystrokes and must be routed as such.
///
/// `next` is [`None`] when the read-ahead found nothing waiting, which is the case that PROVES the
/// pair was not a report: the terminal writes `CSI I` in one piece, so a second byte that had not
/// arrived yet cannot have been part of it.
///
/// The final letter is the whole vocabulary — `I` in, `O` out, xterm's own spelling — and it is
/// matched case-sensitively because that is what the sequence is. A lowercase `i` is a person
/// typing.
#[must_use]
pub fn edge(opened: &InputEvent, next: Option<&InputEvent>) -> Option<Person> {
    if !opens_report(opened) {
        return None;
    }
    match next? {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char(letter),
            modifiers,
        }) if *modifiers == Modifiers::NONE => match letter {
            'I' => Some(Person::Here),
            'O' => Some(Person::Away),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two events termwiz makes of `CSI I` / `CSI O`, built the way the parser emits them.
    fn opened() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char('['),
            modifiers: Modifiers::ALT,
        })
    }

    fn letter(letter: char) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char(letter),
            modifiers: Modifiers::NONE,
        })
    }

    /// The pair a terminal sends when the person arrives and when they leave, in both directions —
    /// asserted TOGETHER so a decoder that read the bracket and ignored the letter would fail rather
    /// than answer one of them plausibly.
    #[test]
    fn the_two_reports_a_terminal_sends_are_the_two_places_a_person_can_be() {
        assert_eq!(edge(&opened(), Some(&letter('I'))), Some(Person::Here));
        assert_eq!(edge(&opened(), Some(&letter('O'))), Some(Person::Away));
    }

    /// **A person's own `Alt-[` survives**, and that is the claim the whole read-ahead exists to
    /// keep: nothing waiting behind it means the bracket was typed, so it must be routed as a key.
    ///
    /// This is the case that would have broken a keymap. `Alt-[` is bindable, and a decoder that
    /// swallowed it whenever a letter happened to follow would eat a binding a person uses.
    #[test]
    fn a_bracket_with_nothing_behind_it_is_a_keystroke_and_not_a_report() {
        assert_eq!(edge(&opened(), None), None);
    }

    /// A letter that is not the protocol's is not a report either — including the LOWERCASE of the
    /// two that are, because `ESC [ i` is a person typing and `ESC [ I` is a terminal speaking.
    #[test]
    fn only_the_protocols_own_two_letters_are_a_report() {
        for not in ['i', 'o', 'A', 'Z', '1', 'M'] {
            assert_eq!(
                edge(&opened(), Some(&letter(not))),
                None,
                "ESC [ {not} is not a focus report",
            );
        }
    }

    /// A modifier on the second half rules it out: `CSI I` carries none, so `Alt-[` followed by
    /// `Ctrl-I` is two keystrokes however fast they arrived.
    #[test]
    fn a_modified_second_half_is_not_a_report() {
        let ctrl_i = InputEvent::Key(KeyEvent {
            key: KeyCode::Char('I'),
            modifiers: Modifiers::CTRL,
        });
        assert_eq!(edge(&opened(), Some(&ctrl_i)), None);
    }

    /// The read-ahead is armed by `Alt-[` and by nothing else — so no other keystroke pays for this
    /// decoder with a zero-wait poll, and no other keystroke can be swallowed by it.
    #[test]
    fn nothing_but_the_bracket_opens_a_report() {
        assert!(opens_report(&opened()));
        for other in [
            letter('['),
            letter('I'),
            InputEvent::Key(KeyEvent {
                key: KeyCode::Char('['),
                modifiers: Modifiers::CTRL,
            }),
            InputEvent::Key(KeyEvent {
                key: KeyCode::Escape,
                modifiers: Modifiers::NONE,
            }),
            InputEvent::Wake,
            InputEvent::Paste("\u{1b}[I".to_owned()),
        ] {
            assert!(!opens_report(&other), "{other:?} must not arm a read-ahead");
            // ...and the resolver agrees, whatever follows: a report that was never opened cannot be
            // closed by a letter that happens to be an `I`.
            assert_eq!(edge(&other, Some(&letter('I'))), None);
        }
    }

    /// A client that has been told nothing has been told nothing has CHANGED — the starting value,
    /// pinned because every silence in this feature rests on it.
    #[test]
    fn a_person_who_has_said_nothing_is_here() {
        assert_eq!(Person::default(), Person::Here);
    }
}
