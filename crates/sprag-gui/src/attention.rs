//! Per-pane ATTENTION state — the client-local half of the OSC 9 / 777 / 99 notification
//! feature (R-PR67 follow-on). The host latches each pane's most recent notification and a
//! monotonic `seq` (see [`sprag_host::PaneNotification`], surfaced through
//! [`SlotView::pane_notification`]); this module tracks how much of it THIS client has SEEN and
//! turns the delta into the "wants attention" marker [`crate::view::pane_display_title`] prefixes.
//!
//! ## The model (tmux's bell flag, with a payload)
//!
//! A pane has UNSEEN attention when its notification `seq` is greater than the last `seq` this
//! client ACKNOWLEDGED for that slot. Acknowledgement happens by VIEWING: [`ack_focused`] runs
//! each frame from the reconcile pass and acks the currently-focused pane to its live `seq`, so
//! the focused pane never shows a marker, and a pane the user has since left keeps its ack (the
//! marker stays cleared until a NEW notification bumps the `seq` past it). This is tmux's
//! monitor-bell flag — set on the escape, cleared on visit — with the notification text carried
//! alongside.
//!
//! ## Why per-SLOT, keyed like the scroll state
//!
//! The ack is client-local view state, exactly the shape of the per-slot scroll offset: it lives
//! in the binding-root [`Owner::cache`] under a [`pane_cache_key`] and is RESET when a slot is
//! freed ([`reset_pane_ack`], called from `reset_freed_slot`) so a reused slot does not inherit a
//! dead pane's ack. Keying on the slot (not the [`PaneId`](sprag_terminal::PaneId)) is safe
//! because a surviving pane keeps its slot (the `plan_slots` invariant) and gives free cleanup on
//! slot reuse.

use pinion_core::reactive::{Owner, Signal};

use crate::slotview::SlotView;
use crate::terminal::pane_cache_key;

/// The [`Owner::cache`] namespace for a slot's acknowledged-notification `seq`.
const ATTENTION_ACK_NAMESPACE: &str = "attention_ack";

/// The marker [`crate::view::pane_display_title`] prefixes onto a pane whose child raised a
/// notification this client has not yet viewed. A geometric dot (NOT an emoji — sprag keeps its
/// output emoji-free), the "unread" convention tab bars and tmux-like UIs use.
pub(crate) const ATTENTION_MARKER: &str = "\u{25CF} ";

/// Slot `slot`'s acknowledged-notification `seq` — the client-local view state, in the
/// binding-root [`Owner::cache`] under a [`pane_cache_key`] (the [`crate::scrollbar`] per-slot
/// pattern). `0` before the pane's first notification is viewed. Reading it in the paint
/// subscribes the view, so [`ack_focused`] writing it repaints the marker away.
fn use_pane_ack(slot: usize) -> Signal<u64> {
    Owner::current()
        .expect("use_pane_ack() requires an active Owner scope")
        .cache(pane_cache_key(ATTENTION_ACK_NAMESPACE, slot), || {
            Signal::new(0)
        })
        .as_ref()
        .clone()
}

/// The live notification `seq` of slot `slot` (`0` when it has raised none) — the host-mirror
/// side of the comparison.
fn pane_seq(slots: &SlotView, slot: usize) -> u64 {
    slots.pane_notification(slot).map_or(0, |note| note.seq)
}

/// Whether slot `slot` has an UNSEEN attention notification: its live `seq` exceeds the last this
/// client acknowledged. Reads the ack [`Signal`] (subscribing the caller), so both a fresh
/// notification (mirror `seq` grows) and a view acknowledging it (ack `seq` grows) repaint.
pub(crate) fn pane_has_unseen_attention(slots: &SlotView, slot: usize) -> bool {
    pane_seq(slots, slot) > use_pane_ack(slot).get()
}

/// ACK the currently-focused pane to its live notification `seq` — called once per frame from the
/// reconcile pass with the focus manager's focused slot (`None` when focus is off any pane). The
/// [`Signal::set`] EQUALITY-SKIPS, so a focused pane with no new notification is inert (no repaint
/// loop); a focused pane that just received one is acked here BEFORE the view runs, so it never
/// flashes its own marker. Every OTHER slot keeps its ack, so an unviewed pane's marker persists.
pub(crate) fn ack_focused(slots: &SlotView, focused_slot: Option<usize>) {
    if let Some(slot) = focused_slot {
        use_pane_ack(slot).set(pane_seq(slots, slot));
    }
}

/// Reset slot `slot`'s ack to `0` — called from `reset_freed_slot` so a slot reused by a NEW pane
/// starts unacknowledged (its first notification shows), never inheriting the dead pane's ack.
pub(crate) fn reset_pane_ack(slot: usize) {
    use_pane_ack(slot).set(0);
}
