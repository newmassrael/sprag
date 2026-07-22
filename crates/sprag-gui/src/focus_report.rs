//! DEC 1004 focus reporting (mouse-tracking Stage 5): as pane focus moves, tell each pane's child
//! whether it is now focused so an app that enabled focus reporting (`ESC [ ? 1004 h` — vim checking
//! for external edits, a TUI dimming when inactive) receives `ESC [ I` on gaining focus and
//! `ESC [ O` on losing it. The GUI knows the focus (`pinion_core::focus_state::focused()`, the same
//! SSOT the window title tracks); the mode-gating + byte encoding live at the host PTY boundary
//! ([`sprag_host::focus`]), so the client reports only the semantic edge.
//!
//! Mapped to PANE focus, not OS-window focus: a pane's child is "focused" exactly when it is the
//! active pane. The whole-window blur dimension (the app losing OS keyboard focus while a pane stays
//! active) is a documented bound — `focus_state` exposes only the focused pane tag, not a
//! window-focus read, so it would need a pinion signal. Pane focus is the multiplexer-core behaviour
//! and a complete vertical on its own (tmux forwards focus at the same granularity).

use std::cell::Cell;

use pinion_core::reactive::Owner;

use crate::slotview::SlotView;

/// The per-client `Owner::cache` key holding the last-focused slot, so a report fires only on a
/// CHANGE (this reconcile runs every frame and must be idempotent).
const LAST_FOCUS_KEY: &str = "sprag_gui.focus_report_last";

/// The focus reports to emit for a focus change from `prev` to `next`: `(slot, false)` = focus LOST
/// for the pane left, `(slot, true)` = focus GAINED for the pane entered. Empty when unchanged. Pure
/// — the SSOT for the transition, unit-testable without a host. Order is OUT-then-IN so a child that
/// somehow observes both sees the leave before the enter.
fn focus_transitions(prev: Option<usize>, next: Option<usize>) -> Vec<(usize, bool)> {
    if prev == next {
        return Vec::new();
    }
    let mut reports = Vec::new();
    if let Some(old) = prev {
        reports.push((old, false));
    }
    if let Some(new) = next {
        reports.push((new, true));
    }
    reports
}

/// Emit focus reports for the current `focused` pane, diffing against the last-focused slot cached
/// per client. Runs in the pre-view [`reconcile_frame`](crate::TerminalViewer) (Owner scope), AFTER
/// the pane-set reconcile so a freed slot's `focus` is a harmless no-op host-side. `None` = no pane
/// focused (the app chrome / a non-pane focus target).
pub(crate) fn reconcile_focus(slots: &SlotView, focused: Option<usize>) {
    let owner = Owner::current().expect("reconcile_focus requires an active Owner scope");
    let last = owner.cache(LAST_FOCUS_KEY, || Cell::new(Option::<usize>::None));
    let prev = last.get();
    for (slot, gained) in focus_transitions(prev, focused) {
        // A report the child's mode does not want (1004 off), or a freed slot, is a no-op success.
        let _ = slots.focus(slot, gained);
    }
    last.set(focused);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_transitions_emit_out_then_in_only_on_a_change() {
        // Boot: nothing -> pane 0 focuses in.
        assert_eq!(focus_transitions(None, Some(0)), vec![(0, true)]);
        // A move between panes: leave the old, enter the new.
        assert_eq!(
            focus_transitions(Some(0), Some(1)),
            vec![(0, false), (1, true)]
        );
        // Focus leaving all panes (app chrome): the last pane focuses out.
        assert_eq!(focus_transitions(Some(1), None), vec![(1, false)]);
        // No change: no reports (idempotent per frame).
        assert!(focus_transitions(Some(2), Some(2)).is_empty());
        assert!(focus_transitions(None, None).is_empty());
    }
}
