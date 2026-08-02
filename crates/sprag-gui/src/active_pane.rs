//! This client's focus, reconciled with the SESSION's active pane (H7).
//!
//! The daemon holds which pane a session is on ([`sprag_host::wire::SELECT_PANE_ACTION`]) — session
//! state every attached client follows, the way they already follow the current window. This module
//! is the GUI's half of that: it FOLLOWS the daemon when the fact moves without this client acting,
//! and PUBLISHES the user's own moves so the other clients — and every pane verb given no target —
//! land where the user is.
//!
//! ## Why an EDGE, and not a level
//!
//! The pane mirror lags a wake behind a write ([`SlotView::active_pane`] reads it without a socket
//! call, which is what lets this run every frame). A client that adopted the mirrored value
//! unconditionally would fight the user: click pane 3, and the next frame — reading a mirror that
//! still says pane 1 — would drag the focus ring back before the poll caught up. So the daemon's
//! answer is adopted when it CHANGES, which is exactly when it carries news: another client's
//! select, a `sprag select-pane` from a shell, a split landing on its new pane, or a close handing
//! off. While the mirror merely lags, this client keeps its own focus. The terminal client reached
//! the same shape for the same reason, and a live test there is what found it.
//!
//! ## Why the publish is DEDUPED
//!
//! A publish is a socket round trip and this runs on the paint path, so re-sending the same pane on
//! every frame until the mirror caught up would put one wire call per frame on a resting client.
//! The last published id is remembered and a repeat is skipped; a move to any other pane sends.
//!
//! ## What it deliberately does not touch
//!
//! DEC 1004 focus reporting ([`crate::focus_report`]) stays a separate fact: that one tells a CHILD
//! that the OS window its pane sits in gained or lost the user's attention, and it is intersected
//! with the window manager's answer. This one is about which pane the SESSION is on, which is true
//! whether or not any window is focused — alt-tabbing the app away must not move it.

use pinion_core::reactive::{Owner, Signal};
use sprag_terminal::PaneId;

use crate::slotview::SlotView;

/// The [`Owner::cache`] key for the daemon's active pane AS THIS CLIENT LAST SAW IT — the edge this
/// module triggers on. Client-wide (not per-slot) because its subject is the session, not a pane.
const SEEN_KEY: &str = "sprag_gui.active_pane.seen";

/// The [`Owner::cache`] key for the pane this client last PUBLISHED, so a mirror that has not yet
/// caught up cannot make it publish the same move on every frame.
const SENT_KEY: &str = "sprag_gui.active_pane.sent";

/// What one frame's reconcile has to DO — the whole decision, taken as a pure function of four
/// `Option<PaneId>`s so the rule is testable without a host, a slot map or an Owner scope (the
/// split `crate::focus_report` and the MCP's pane summary already use).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Move {
    /// The session moved onto this pane without this client asking: adopt it.
    Follow(PaneId),
    /// The user moved this client onto a pane the daemon has not been told about: publish it.
    Publish(PaneId),
    /// Nothing to do.
    Rest,
}

/// Decide one frame.
///
/// * `daemon` — the session's active pane as this client's mirror reports it.
/// * `seen` — what `daemon` was the last time this ran, which is what makes this an edge.
/// * `here` — the pane THIS client's focus is on, if any.
/// * `sent` — the pane this client last published.
///
/// The two directions are mutually exclusive by construction: on a frame where the daemon's answer
/// MOVED this client follows and publishes nothing (the daemon is where the news came from), and on
/// any other frame a local focus the daemon has not been told about is published.
pub(crate) fn decide(
    daemon: Option<PaneId>,
    seen: Option<PaneId>,
    here: Option<PaneId>,
    sent: Option<PaneId>,
) -> Move {
    if daemon != seen {
        // A window that has just emptied answers `None`, and there is nothing to follow onto —
        // the focus stays where it is rather than being blanked.
        return daemon.map_or(Move::Rest, Move::Follow);
    }
    match here {
        // Focus off every pane (a sidebar, a palette) says nothing about where the SESSION is: the
        // user has not left the pane, they are talking to the client's own chrome.
        None => Move::Rest,
        Some(pane) if Some(pane) != daemon && Some(pane) != sent => Move::Publish(pane),
        Some(_) => Move::Rest,
    }
}

/// One client-wide `Option<PaneId>` cell in the binding-root [`Owner::cache`] (the shape
/// [`crate::attention`]'s per-slot ack uses, one level up: this state is not a pane's).
fn cell(key: &'static str) -> Signal<Option<PaneId>> {
    Owner::current()
        .expect("the active-pane cells require an active Owner scope")
        .cache(key.to_owned(), || Signal::new(None))
        .as_ref()
        .clone()
}

/// Apply [`decide`] to this frame — called from the reconcile pass with `focused`, the slot the
/// focus manager reports.
///
/// `seen` is written from what was OBSERVED rather than from what was applied, so a follow onto a
/// pane this client is not showing yet (a hole while the slot map catches up) is not retried
/// forever — the next real move is the next edge.
pub(crate) fn reconcile(slots: &SlotView, focused: Option<usize>) {
    let daemon = slots.active_pane();
    let seen = cell(SEEN_KEY);
    let sent = cell(SENT_KEY);
    let here = focused.and_then(|slot| slots.id(slot));
    let decision = decide(daemon, seen.get(), here, sent.get());
    seen.set(daemon);
    match decision {
        Move::Follow(pane) => {
            if let Some(slot) = slots.slot_of(pane) {
                pinion_core::focus_request::request(crate::terminal::pane_tag(slot));
            }
        }
        Move::Publish(pane) => {
            sent.set(Some(pane));
            // A refusal is not this client's to repair: the daemon owns the pane set, so a pane it
            // will not select is one that has left, and the next edge answers with what IS.
            slots.select_pane(pane);
        }
        Move::Rest => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: u64) -> Option<PaneId> {
        Some(PaneId(id))
    }

    /// The FOLLOW direction and the reason it is an edge. The second case is the CONTROL: the same
    /// daemon answer, now already seen, must not be adopted again — a level-triggered rule would
    /// re-request it every frame and drag a user's click back before the mirror caught up.
    #[test]
    fn a_moved_answer_is_followed_and_an_unmoved_one_is_left_alone() {
        assert_eq!(
            decide(pane(2), pane(1), pane(1), None),
            Move::Follow(PaneId(2)),
            "the session moved onto pane 2 without this client asking",
        );
        assert_eq!(
            decide(pane(2), pane(2), pane(2), None),
            Move::Rest,
            "THE CONTROL: the same answer, already seen, is not news",
        );
        assert_eq!(
            decide(pane(2), pane(2), pane(3), None),
            Move::Publish(PaneId(3)),
            "...and a client whose own focus has since moved publishes instead",
        );
    }

    /// The lagging mirror, which is the case that made this an edge rather than a level: the user
    /// has clicked pane 3, the daemon's answer has not caught up, and the client must NOT be
    /// dragged back to pane 1. It publishes once and then rests until something changes.
    #[test]
    fn a_local_move_publishes_once_while_the_mirror_catches_up() {
        assert_eq!(
            decide(pane(1), pane(1), pane(3), None),
            Move::Publish(PaneId(3)),
        );
        assert_eq!(
            decide(pane(1), pane(1), pane(3), pane(3)),
            Move::Rest,
            "the same move is not sent again on the next frame",
        );
        assert_eq!(
            decide(pane(3), pane(1), pane(3), pane(3)),
            Move::Follow(PaneId(3)),
            "and when the mirror does catch up the follow is a no-op onto where it already is",
        );
        assert_eq!(
            decide(pane(3), pane(3), pane(1), pane(3)),
            Move::Publish(PaneId(1)),
            "moving somewhere else publishes again — the dedupe is per pane, not once for good",
        );
    }

    /// The two `None`s, which mean different things and must not be conflated. A daemon with no
    /// active pane (an emptied window) has nothing to follow onto; a client focused on its own
    /// chrome has not said the user left the pane.
    #[test]
    fn an_absent_answer_and_an_unfocused_client_both_rest() {
        assert_eq!(
            decide(None, pane(1), pane(1), None),
            Move::Rest,
            "an emptied window does not blank this client's focus",
        );
        assert_eq!(
            decide(pane(1), pane(1), None, None),
            Move::Rest,
            "focus on the sidebar is not the user leaving the pane",
        );
        assert_eq!(
            decide(pane(1), None, pane(1), None),
            Move::Follow(PaneId(1)),
            "and the FIRST frame follows, which is how a fresh client inherits where the session is",
        );
    }
}
