//! Dock / undock: WHICH OS window paints each pane (orthogonal to `terminal`,
//! which owns *creating/holding* the panes). A pane is **docked** (tiled in the
//! main window) or **undocked** (painted alone in its own OS window). See the
//! crate-root "Dock / undock" docs.
//!
//! ## The seam (pinion multi-window)
//!
//! pinion drives runtime windows from a reactive
//! [`Signal<Vec<WindowSpec>>`](pinion_core::reactive::Signal) the binding returns
//! via [`WidgetView::windows_signal`](pinion_shell::WidgetView::windows_signal):
//! the shell's `reconcile_windows` Effect diffs each `set` and adds/drops winit
//! windows. [`view_for_window`](crate::view::view_for_window) then paints each
//! window by id. This module owns that topology Signal ([`use_windows_topology`])
//! and the dock/undock toggle ([`toggle_pane_floating`]).
//!
//! ## Floating SSOT — the topology Signal itself
//!
//! There is no separate "which panes float" set: a pane floats **iff** its undock
//! window (`pane-{i}`) exists in the topology ([`is_pane_floating`]). One source
//! of truth, read by the main-window tiling, the per-window paint dispatch, and
//! a11y (the hello-dock-panels model).
//!
//! ## Why the undock window is FIXED-size (an honest v1 bound — pinion gap)
//!
//! The undock window opens sized to the pane's intrinsic `(cols, rows) × cell`,
//! so the pane fits 1:1 with **no reflow** needed. This is deliberate: pinion's
//! R1006 / R1012 viewport publishes are gated to `DEFAULT_WINDOW`
//! (`pinion-shell` `compute_paint_scene_internal`, "per-window signal deferred"),
//! so a secondary window publishes neither its viewport size nor its pane rects —
//! an undocked pane therefore **cannot reflow to its own window**. `Fixed` stays
//! OS-resizable, but dragging the undock border does NOT reflow the pane (slack
//! shows the surface fill). Sizing the window to the pane's own dims is the
//! correct intrinsic size, NOT window-side split math (the SSOT trap). Resizable
//! undock windows need the per-window viewport publish — reported as a pinion
//! requirement (`claudedocs/PINION-PR10-PER-WINDOW-VIEWPORT.md`).

use crate::terminal::{MAX_PANES, use_terminal};
use crate::{WINDOW_H, WINDOW_W};
use pinion_core::reactive::{Owner, Signal};
use pinion_shell::{SizeStrategy, WindowSpec};
use std::borrow::Cow;
use std::rc::Rc;

/// `Owner::cache` key for the runtime window topology Signal.
const WINDOWS_KEY: &str = "sprag_gui.windows";

/// The canonical main-window id (maps to pinion's `DEFAULT_WINDOW`).
pub(crate) const MAIN_WINDOW_ID: &str = "main";

/// The undock-window id prefix; an undock window for pane `i` is `pane-{i}`.
const UNDOCK_WINDOW_PREFIX: &str = "pane-";

/// The undock-window id for pane `i` (`pane-{i}`). A DISTINCT namespace from the
/// scene/focus [`pane_tag`](crate::terminal::pane_tag) (`sprag_gui.pane.{i}`):
/// both keyed by the tile index `i`, but one is a pinion window id and the other
/// a scene tag — not interchangeable.
pub(crate) fn pane_window_id(i: usize) -> String {
    format!("{UNDOCK_WINDOW_PREFIX}{i}")
}

/// The pane index an undock-window id addresses, or `None` for the main window /
/// an unknown id / an out-of-range index. A total function (validates
/// `< `[`MAX_PANES`]) so a malformed window id can never index a pane out of
/// range — the [`crate::terminal::pane_index_of`] discipline for window ids.
pub(crate) fn pane_window_index(window_id: &str) -> Option<usize> {
    window_id
        .strip_prefix(UNDOCK_WINDOW_PREFIX)?
        .parse::<usize>()
        .ok()
        .filter(|&i| i < MAX_PANES)
}

/// The runtime window topology Signal — the floating SSOT. Cached in the root
/// owner (the view fns + `windows_signal` resolve the same shared slot), seeded
/// with just the main window. [`toggle_pane_floating`] pushes/removes undock
/// windows; the shell subscribes this Signal and reconciles winit windows on each
/// `set`.
pub(crate) fn use_windows_topology() -> Rc<Signal<Vec<WindowSpec>>> {
    Owner::current()
        .expect("use_windows_topology() requires an active Owner scope")
        .cache(WINDOWS_KEY, || {
            Signal::new(vec![WindowSpec::new(
                Cow::Borrowed(MAIN_WINDOW_ID),
                "sprag terminal (interactive)",
                SizeStrategy::Fixed {
                    width: WINDOW_W,
                    height: WINDOW_H,
                },
            )])
        })
}

/// `true` iff pane `i`'s undock window currently exists in `windows` (i.e. pane
/// `i` is floating). The single docked/floating predicate, consulted by the
/// main-window tiling, the paint dispatch, and a11y.
pub(crate) fn is_pane_floating(windows: &[WindowSpec], i: usize) -> bool {
    let target = pane_window_id(i);
    windows.iter().any(|w| w.id == target)
}

/// Toggle pane `i` between docked (tiled in the main window) and undocked (its
/// own OS window). Idempotent on the alternation:
///
/// * docked (no `pane-{i}` window) -> push an undock `WindowSpec` sized to the
///   pane's intrinsic `(cols, rows) × cell` (so it fits 1:1; see the module docs
///   on why it is fixed-size);
/// * floating (window exists) -> remove it (dock back). The shell drops the winit
///   window; the main layout repaints with pane `i` re-tiled (and it reflows to
///   its new tile via the main-window R1012 publish).
///
/// Runs inside the shell root owner scope (called from `route_key`, itself
/// wrapped in `root_owner.run`), so [`use_terminal`] / [`use_windows_topology`]
/// resolve. The pane's `(cols, rows)` is read from its live session — its own
/// authoritative dims, NOT a window-side split calc.
pub(crate) fn toggle_pane_floating(i: usize) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    if let Some(idx) = windows.iter().position(|w| w.id == target) {
        windows.remove(idx); // dock back
    } else {
        let tv = use_terminal();
        let (cols, rows) = tv.pane(i).session().dimensions();
        // Open fixed to the pane's intrinsic (cols, rows) x cell — the grid_dims
        // inverse ([`cell_px`]) — so it fits 1:1 with no reflow needed.
        let (width, height) = crate::terminal::cell_px(tv.metric, cols, rows);
        windows.push(WindowSpec::new(
            Cow::Owned(target),
            format!("sprag terminal — pane {i}"),
            SizeStrategy::Fixed {
                width: width.max(1),
                height: height.max(1),
            },
        ));
    }
    signal.set(windows);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_window_id_round_trips_through_index() {
        for i in 0..MAX_PANES {
            assert_eq!(pane_window_index(&pane_window_id(i)), Some(i));
        }
        // The main window and unknown / out-of-range ids are not pane windows.
        assert_eq!(pane_window_index(MAIN_WINDOW_ID), None);
        assert_eq!(pane_window_index("pane-"), None); // no index
        assert_eq!(pane_window_index("pane-x"), None); // non-numeric
        assert_eq!(pane_window_index("nope"), None);
        assert_eq!(pane_window_index(&pane_window_id(MAX_PANES)), None); // out of range
    }

    #[test]
    fn toggle_round_trips_the_topology_signal() {
        // Boots the real panes (use_terminal spawns shells); dropping the owner
        // reaps them. The topology starts with the lone main window.
        let owner = Owner::new();
        owner.run(|| {
            let windows = use_windows_topology();
            assert_eq!(windows.get().len(), 1, "boots with the main window only");
            assert!(!is_pane_floating(&windows.get(), 0), "pane 0 starts docked");

            // Undock pane 0: a second window appears and pane 0 reads as floating.
            toggle_pane_floating(0);
            assert_eq!(windows.get().len(), 2, "undock adds a window");
            assert!(
                is_pane_floating(&windows.get(), 0),
                "pane 0 is now floating"
            );
            assert!(!is_pane_floating(&windows.get(), 1), "pane 1 stays docked");

            // Dock back: the window is removed, pane 0 docked again.
            toggle_pane_floating(0);
            assert_eq!(windows.get().len(), 1, "dock-back removes the window");
            assert!(
                !is_pane_floating(&windows.get(), 0),
                "pane 0 is docked again"
            );
        });
    }
}
