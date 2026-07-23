//! DEC 1004 focus reporting (mouse-tracking Stage 5): as pane focus moves, tell each pane's child
//! whether it is now focused so an app that enabled focus reporting (`ESC [ ? 1004 h` — vim checking
//! for external edits, a TUI dimming when inactive) receives `ESC [ I` on gaining focus and
//! `ESC [ O` on losing it. The GUI knows the focus (`pinion_core::focus_state::focused()`, the same
//! SSOT the window title tracks); the mode-gating + byte encoding live at the host PTY boundary
//! ([`sprag_host::focus`]), so the client reports only the semantic edge.
//!
//! Focus has TWO axes and a pane's child is focused only when BOTH hold: (1) WHICH pane is active
//! within the app (pane<->pane, `pinion_core::focus_state::focused()`), and (2) whether the OS
//! window CONTAINING that pane holds the OS keyboard focus (`window_focus_state::os_focused_window()`,
//! pinion R1419/PR73). [`os_gated_focus`] intersects them before [`reconcile_focus`], so alt-tabbing
//! the whole app away emits `ESC [ O` to the active pane's child (vim re-runs its external-edit check
//! on return, a TUI dims while the app is blurred) and returning emits `ESC [ I`.
//!
//! tmux-superior: tmux forwards its single client terminal's DEC 1004 focus to whichever pane is
//! active, so every pane shares ONE outer-terminal focus signal; sprag reports each OS window — the
//! main tiling window AND each tear-off floating window — with its OWN OS-focus accuracy (the R1421
//! window-IDENTITY read: a floating pane reports blur when a DIFFERENT window of the same app holds
//! focus, which a single shared bool could not express).

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

/// The effective DEC 1004 focus for the current frame: the within-app `focused_pane` gated on
/// OS-window focus (pinion R1419–R1421 / PINION-PR73). The focused pane's child holds keyboard
/// focus only while the OS window CONTAINING that pane is the one the window manager has activated,
/// so this keeps `focused_pane` iff `os_focused_window` names the pane's own window (`window_id_of`:
/// the main tiling window, or the pane's `pane-{i}` tear-off) — the R1421 window-IDENTITY read.
///
/// `None` `os_focused_window` (the whole app is blurred, or OS focus is not yet known — headless, or
/// before the first focus event) collapses to no effective focus, so the active pane's child gets
/// `ESC [ O`. Pure — the SSOT for the OS-focus gate, unit-testable without a live window: `main.rs`
/// resolves the reactive `window_focus_state::os_focused_window()` (auto-subscribing the reconcile)
/// and the live pane->window mapping, then delegates the decision here.
pub(crate) fn os_gated_focus(
    focused_pane: Option<usize>,
    os_focused_window: Option<&str>,
    window_id_of: impl Fn(usize) -> String,
) -> Option<usize> {
    focused_pane.filter(|&i| os_focused_window == Some(window_id_of(i).as_str()))
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

    /// Stand-in pane->window map: pane 2 floats (its own `pane-2` window), every other pane lives
    /// in the main tiling window — the shape `main.rs` builds from `dock::is_pane_floating`.
    fn window_of(i: usize) -> String {
        if i == 2 {
            "pane-2".to_owned()
        } else {
            "main".to_owned()
        }
    }

    #[test]
    fn os_focus_gate_intersects_pane_and_window_focus() {
        // A tiled pane whose window (main) holds OS focus: the child is focused.
        assert_eq!(os_gated_focus(Some(0), Some("main"), window_of), Some(0));
        // Whole-app blur (os focus None): the active pane's child focuses OUT.
        assert_eq!(os_gated_focus(Some(0), None, window_of), None);
        // A floating pane whose own tear-off window holds OS focus: focused.
        assert_eq!(os_gated_focus(Some(2), Some("pane-2"), window_of), Some(2));
    }

    #[test]
    fn os_focus_gate_is_per_window_identity_not_a_shared_bool() {
        // The app IS focused, but on a DIFFERENT window than the one holding the focused pane: the
        // focused pane's child is NOT focused (R1421 window-identity — a shared bool would miss this).
        // Focus on the main window while pane 2 floats -> pane 2's child focuses out.
        assert_eq!(os_gated_focus(Some(2), Some("main"), window_of), None);
        // Focus on the floating window while a tiled pane is within-app focused -> that pane out.
        assert_eq!(os_gated_focus(Some(0), Some("pane-2"), window_of), None);
    }

    #[test]
    fn os_focus_gate_is_none_when_no_pane_is_focused() {
        // No within-app focus (app chrome) stays None regardless of OS-window focus.
        assert_eq!(os_gated_focus(None, Some("main"), window_of), None);
        assert_eq!(os_gated_focus(None, None, window_of), None);
    }
}
