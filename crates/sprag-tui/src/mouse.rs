//! Turning this terminal's mouse reports into the semantic pointer edges the wire carries.
//!
//! The two ends disagree about what a mouse event IS, and reconciling them is the whole of this
//! module. termwiz hands over a STATE — where the pointer is and which buttons are down at that
//! moment — because that is what it can read off an SGR report without remembering anything.
//! [`MouseInput`] is an EDGE: a press, a release, a drag, a motion. A state stream becomes an edge
//! stream only by remembering the previous state, so this is the one input path in the client that
//! is not a pure function of its argument.
//!
//! The edge is what the wire wants because it is what a child wants: `\x1b[<0;5;3M` says "button 0
//! went down at (5,3)", never "button 0 is down". sprag's encoder
//! ([`encode_mouse`](sprag_input::encode_mouse)) speaks that language and the host gates it against
//! the pane's tracking mode, so a client that guessed here would produce reports no program asked
//! for rather than reports it could not send.

use sprag_input::{Modifiers, MouseButton, MouseEventKind, MouseInput};
use termwiz::input::{MouseButtons, MouseEvent};

/// The pointer state this terminal was last seen in, kept so the next report can be read as a
/// change rather than as a snapshot.
///
/// One per client, not one per pane: the pointer is a property of the TERMINAL, and a decoder per
/// pane would see a button pressed in one pane and released in another as two unrelated states —
/// losing the release, which is the edge a drag-select depends on ending with.
#[derive(Debug, Default)]
pub struct MouseEdges {
    /// The buttons held as of the last event, WHEEL BITS EXCLUDED (see [`MouseEdges::edges`]).
    held: MouseButtons,
    /// The cell the pointer was last in, 0-based, or `None` before the first event.
    at: Option<(u16, u16)>,
}

impl MouseEdges {
    /// Read one terminal mouse event as the edges it represents, in 0-based SCREEN cells.
    ///
    /// # The coordinates are 1-based on the way in
    ///
    /// termwiz passes the SGR report's numbers through unchanged, and those are 1-based by the
    /// protocol — its own parser test reads `\x1b[<66;42;12M` as `x: 42, y: 12`. [`MouseInput`] is
    /// 0-based (the host's encoder adds the one back on the way out), so the subtraction happens
    /// here, exactly once. `saturating_sub` rather than `- 1` because a terminal that reported a
    /// zero would otherwise wrap to the far edge of the screen, turning a malformed report into a
    /// click somewhere plausible.
    ///
    /// # A wheel notch is a press with no release, and what guarantees that is the button TABLE
    ///
    /// xterm reports the wheel as pseudo-buttons 64/65 and sends NO release for them — a notch is
    /// one event. termwiz surfaces that as a `VERT_WHEEL` bit set on the notch's event and absent
    /// from the next one, which a state diff over ALL bits would read as a button being released;
    /// [`MouseButton::WheelUp`] with [`MouseEventKind::Release`] is a report no terminal sends.
    ///
    /// What prevents it is the `BUTTONS` table, which lists the three REAL buttons and nothing else: a
    /// bit with no entry there cannot produce an edge in either direction. MEASURED — removing the
    /// wheel mask below leaves every test in this module green, so the mask is NOT what is holding
    /// this up and a comment claiming it was would be pointing at the wrong guard.
    ///
    /// The mask is kept anyway, for the narrower reason that it makes the remembered `held` set mean
    /// what its name says. The drag-or-motion decision at the end reads `held` as "is a button
    /// down", and that reading is only sound while a wheel notch cannot leave a bit in it.
    ///
    /// # What is dropped, and why dropping is right
    ///
    /// A HORIZONTAL wheel (xterm's buttons 6/7) has no [`MouseButton`] to become. Inventing one
    /// here would mean encoding a report the host cannot describe; the honest fix is a variant in
    /// `sprag-input` and an encoder arm beside it, which is a change to the shared vocabulary
    /// rather than to this client.
    pub fn edges(&mut self, event: &MouseEvent) -> Vec<MouseInput> {
        // `MouseButtons` is a bitflags set that is not `Copy` in this termwiz, so the masks are
        // cloned rather than moved out of the borrowed event; the arithmetic below is unchanged by
        // that and the clones are of one byte each.
        let wheels = MouseButtons::VERT_WHEEL | MouseButtons::HORZ_WHEEL;
        let col = event.x.saturating_sub(1);
        let row = event.y.saturating_sub(1);
        let mods = modifiers(event);
        let at = (col, row);
        let moved = self.at.is_some_and(|last| last != at);
        self.at = Some(at);

        // A constructor rather than a pushing closure: the "is anything reported yet" test below
        // reads `out`, which a closure holding it mutably would forbid.
        let edge = |button, kind| MouseInput {
            button,
            kind,
            col,
            row,
            mods,
        };
        let mut out = Vec::new();

        // The wheel first and on its own: it is a notch, not a state, so it neither reads from nor
        // writes to `held`.
        if event.mouse_buttons.contains(MouseButtons::VERT_WHEEL) {
            let up = event.mouse_buttons.contains(MouseButtons::WHEEL_POSITIVE);
            let button = if up {
                MouseButton::WheelUp
            } else {
                MouseButton::WheelDown
            };
            out.push(edge(button, MouseEventKind::Press));
        }

        // The buttons genuinely HELD: the wheel bits and the direction flag are not buttons, and
        // leaving them in would make `held` answer yes to "is something down" after a notch. See
        // the doc above for why this is not what keeps a wheel from being released.
        let now = event.mouse_buttons.clone() - wheels - MouseButtons::WHEEL_POSITIVE;
        let was = std::mem::replace(&mut self.held, now.clone());

        for (bit, button) in BUTTONS {
            match (was.contains(bit.clone()), now.contains(bit.clone())) {
                (false, true) => out.push(edge(button, MouseEventKind::Press)),
                (true, false) => out.push(edge(button, MouseEventKind::Release)),
                _ => {}
            }
        }

        // A motion is what is left when the button state did not change and the cell did. Reported
        // as a DRAG while anything is held, which is the distinction the tracking modes are built
        // on: button-event tracking wants the drag and not the bare motion.
        if out.is_empty() && moved {
            let kind = if now.is_empty() {
                MouseEventKind::Motion
            } else {
                MouseEventKind::Drag
            };
            out.push(edge(held_button(&now), kind));
        }
        out
    }
}

/// The buttons that carry a press/release edge, in report order.
const BUTTONS: [(MouseButtons, MouseButton); 3] = [
    (MouseButtons::LEFT, MouseButton::Left),
    (MouseButtons::MIDDLE, MouseButton::Middle),
    (MouseButtons::RIGHT, MouseButton::Right),
];

/// Which button a drag is attributed to when several are held: the lowest-numbered, which is what
/// xterm reports. A bare motion has [`MouseButton::None`].
fn held_button(held: &MouseButtons) -> MouseButton {
    BUTTONS
        .iter()
        .find(|(bit, _)| held.contains(bit.clone()))
        .map_or(MouseButton::None, |(_, button)| *button)
}

/// termwiz's modifier set as sprag's. `sup` is absent from a mouse report by protocol — xterm's
/// modifier bits are shift/meta/control only — so it is false rather than unknown.
fn modifiers(event: &MouseEvent) -> Modifiers {
    use termwiz::input::Modifiers as Term;
    Modifiers {
        ctrl: event.modifiers.contains(Term::CTRL),
        alt: event.modifiers.contains(Term::ALT),
        shift: event.modifiers.contains(Term::SHIFT),
        sup: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(buttons: MouseButtons, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            x,
            y,
            mouse_buttons: buttons,
            modifiers: termwiz::input::Modifiers::NONE,
        }
    }

    /// The 1-based report becomes a 0-based cell, once. The top-left corner is the case that makes
    /// it visible: a client that forwarded the report's own numbers would put the terminal's first
    /// cell at (1, 1), which is a cell the pane at the origin also has — so every click would land
    /// one down and one right, plausibly enough to look like a rendering problem.
    #[test]
    fn a_report_arrives_one_based_and_leaves_zero_based() {
        let mut edges = MouseEdges::default();
        let out = edges.edges(&at(MouseButtons::LEFT, 1, 1));
        assert_eq!(out.len(), 1, "one press: {out:?}");
        assert_eq!(
            (out[0].col, out[0].row),
            (0, 0),
            "the terminal's first cell"
        );
        assert_eq!(out[0].kind, MouseEventKind::Press);
        assert_eq!(out[0].button, MouseButton::Left);
    }

    /// Press then release: the release is the state going away, which only a decoder that remembers
    /// can see. A stateless reading of the second event says "no buttons are down", which is true
    /// and is not an event.
    #[test]
    fn a_button_going_away_is_a_release() {
        let mut edges = MouseEdges::default();
        edges.edges(&at(MouseButtons::LEFT, 5, 3));
        let out = edges.edges(&at(MouseButtons::NONE, 5, 3));
        assert_eq!(out.len(), 1, "one release: {out:?}");
        assert_eq!(out[0].kind, MouseEventKind::Release);
        assert_eq!(out[0].button, MouseButton::Left, "the button that left");
    }

    /// A wheel notch is a PRESS and nothing else — and, critically, the next event does not read as
    /// its release. That is the rule the state diff would get wrong on its own, and the report it
    /// would invent (`WheelUp` + `Release`) is one no terminal sends.
    #[test]
    fn a_wheel_notch_is_a_press_that_is_never_released() {
        let mut edges = MouseEdges::default();
        let notch = edges.edges(&at(
            MouseButtons::VERT_WHEEL | MouseButtons::WHEEL_POSITIVE,
            9,
            9,
        ));
        assert_eq!(notch.len(), 1, "one edge for a notch: {notch:?}");
        assert_eq!(notch[0].button, MouseButton::WheelUp);
        assert_eq!(notch[0].kind, MouseEventKind::Press);

        // The pointer moves on, with the wheel bit gone. The whole edge list is asserted, not just
        // the absence of a wheel release: what follows a notch must be an ordinary bare motion, so
        // a wheel added to `BUTTONS` — the table that is actually holding this up — would fail here
        // rather than quietly start releasing a button nobody pressed.
        let after = edges.edges(&at(MouseButtons::NONE, 10, 9));
        assert_eq!(after.len(), 1, "one edge after a notch: {after:?}");
        assert_eq!(after[0].kind, MouseEventKind::Motion);
        assert_eq!(after[0].button, MouseButton::None, "nothing was held");
    }

    /// A downward notch is the same event with the direction flag clear, and the flag must not be
    /// read as a button: `WHEEL_POSITIVE` set on one notch and clear on the next would otherwise be
    /// a press and a release of a button that does not exist.
    #[test]
    fn the_wheel_direction_flag_is_not_a_button() {
        let mut edges = MouseEdges::default();
        let up = edges.edges(&at(
            MouseButtons::VERT_WHEEL | MouseButtons::WHEEL_POSITIVE,
            4,
            4,
        ));
        let down = edges.edges(&at(MouseButtons::VERT_WHEEL, 4, 4));
        assert_eq!(up.len(), 1, "one edge up: {up:?}");
        assert_eq!(down.len(), 1, "one edge down: {down:?}");
        assert_eq!(down[0].button, MouseButton::WheelDown);
        assert_eq!(down[0].kind, MouseEventKind::Press);
    }

    /// Moving with a button held is a DRAG; moving with none is a MOTION. The two are separate
    /// tracking levels on the wire (button-event wants the first and not the second), so a client
    /// that reported one as the other would hand a program events it declined.
    #[test]
    fn moving_is_a_drag_while_held_and_a_motion_while_not() {
        let mut edges = MouseEdges::default();
        edges.edges(&at(MouseButtons::LEFT, 2, 2));
        let dragged = edges.edges(&at(MouseButtons::LEFT, 3, 2));
        assert_eq!(dragged.len(), 1, "one drag: {dragged:?}");
        assert_eq!(dragged[0].kind, MouseEventKind::Drag);
        assert_eq!(dragged[0].button, MouseButton::Left, "the held button");
        assert_eq!((dragged[0].col, dragged[0].row), (2, 1));

        edges.edges(&at(MouseButtons::NONE, 3, 2));
        let moved = edges.edges(&at(MouseButtons::NONE, 4, 2));
        assert_eq!(moved.len(), 1, "one motion: {moved:?}");
        assert_eq!(moved[0].kind, MouseEventKind::Motion);
        assert_eq!(moved[0].button, MouseButton::None);
    }

    /// Standing still reports nothing. Terminals repeat a report on a sub-cell movement, and a
    /// client that forwarded each one would send a stream of identical motions to a program that
    /// asked to be told when the pointer MOVED.
    #[test]
    fn a_repeated_report_at_the_same_cell_is_not_an_event() {
        let mut edges = MouseEdges::default();
        edges.edges(&at(MouseButtons::NONE, 7, 7));
        let again = edges.edges(&at(MouseButtons::NONE, 7, 7));
        assert!(
            again.is_empty(),
            "nothing moved and nothing changed: {again:?}"
        );
    }

    /// The modifiers ride along, because the report carries them and a program reads them: a
    /// ctrl-click in a pager is a different gesture from a click.
    #[test]
    fn the_reports_modifiers_reach_the_edge() {
        let mut edges = MouseEdges::default();
        let out = edges.edges(&MouseEvent {
            x: 2,
            y: 2,
            mouse_buttons: MouseButtons::RIGHT,
            modifiers: termwiz::input::Modifiers::CTRL | termwiz::input::Modifiers::SHIFT,
        });
        assert_eq!(out.len(), 1, "one press: {out:?}");
        assert!(out[0].mods.ctrl && out[0].mods.shift && !out[0].mods.alt);
    }
}
