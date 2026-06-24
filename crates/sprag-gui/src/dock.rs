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
//! ## Two distinct authorities: OS windows vs the docked-pane set
//!
//! This module's `Signal<Vec<WindowSpec>>` is the authority for **which OS windows
//! exist** — a `pane-{i}` window exists iff pane `i` is floating. That is a SEPARATE
//! fact from **which panes are docked in the main window**, which is the dock split-
//! tree's leaf set ([`crate::split::docked_pane_indices`]) — the one source both the
//! paint ([`crate::view::view_for_window`]) and a11y read, so they never disagree. The two
//! authorities are kept consistent at the single [`toggle_pane_floating`] site (a
//! pane floats iff its window exists AND its leaf is absent from the tree); a future
//! second mutation path (P2 drag-to-dock) must preserve that invariant — at which
//! point making the docked set a reactive projection of one authority (rather than
//! co-mutated here) is the cleanup.
//!
//! ## Why the undock window opens at the pane's intrinsic size (and now reflows)
//!
//! The undock window opens sized to the pane's intrinsic `(cols, rows) × cell`, so
//! the pane fits 1:1 at the moment it tears off — the correct intrinsic size, NOT
//! window-side split math (the SSOT trap). It then **reflows the pane to its own
//! window size, both axes, as the window resizes**: pinion R1021 publishes the
//! per-pane viewport rect for every painted window (the R1012 publish is no longer
//! `DEFAULT_WINDOW`-gated — see `pinion-shell` `compute_paint_scene_internal`,
//! "R1021 … published for EVERY painted window"), so the floated pane's existing
//! [`crate::reflow`] Effect (it subscribes to `use_pane_viewport_size(pane_tag(i))`)
//! fires on the secondary window's rect and `TIOCSWINSZ`-reflows the PTY. This is
//! the consumer of `claudedocs/PINION-PR10-PER-WINDOW-VIEWPORT.md` (DELIVERED). The
//! lone-pane scene is given a definite extent via
//! [`crate::view::fill_definite`] (the same fill the docked split-tree uses) so
//! it reflows in BOTH axes, not only width — see that fn's docs.
//!
//! ## Freely resizable: grow AND shrink (pinion R1059 / PINION-PR23)
//!
//! The undock window uses [`SizeStrategy::OpenResizable`]`{ size, min: None }`: it
//! opens at the pane's intrinsic `size` (1:1 tear-off) but the OS-resize floor is
//! the OS-native minimum (NOT the open size), so the user can GROW it (the pane
//! reflows larger) AND SHRINK it below the open size (reflows smaller) — both axes.
//! `Fixed` would pin the floor at the open size (shrink blocked); `OpenResizable`
//! decouples the open size from the floor, which is what a plain resizable window
//! wants. This consumes `claudedocs/PINION-PR23-RESIZABLE-WINDOW-MIN-FLOOR.md`
//! (DELIVERED as pinion R1059). Verified end-to-end with the live-surface capture
//! `scene/screenshot` (PINION-PR24, R1060–R1062): an undock window grown to 600×900
//! and shrunk to 300×360 reflows + renders with no white slack.

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

/// Toggle pane `i` between docked (tiled in the main window) and undocked (its
/// own OS window). Idempotent on the alternation:
///
/// * docked (no `pane-{i}` window) -> push an undock `WindowSpec` sized to the
///   pane's intrinsic `(cols, rows) × cell` (so it fits 1:1 at tear-off; resizing
///   the window then reflows the pane in both axes — see the module docs), and
///   remove its leaf from the dock split-tree ([`crate::split::float_pane`]) so the
///   remaining docked panes reclaim its space;
/// * floating (window exists) -> remove it (dock back) and re-insert its leaf into
///   the split-tree ([`crate::split::dock_pane`]). The shell drops the winit window;
///   the main layout repaints with pane `i` re-tiled (and it reflows to its new tile
///   via the main-window R1012 publish).
///
/// The window topology (the floating SSOT) and the dock split-tree are TWO signals;
/// this one toggle site mutates BOTH together so they never disagree (a pane floats
/// iff its `pane-{i}` window exists AND its leaf is absent from the tree).
///
/// Runs inside the shell root owner scope (called from `route_key`, itself
/// wrapped in `root_owner.run`), so [`use_terminal`] / [`use_windows_topology`] /
/// the split-tree hooks resolve. The pane's `(cols, rows)` is read from its live
/// session — its own authoritative dims, NOT a window-side split calc.
pub(crate) fn toggle_pane_floating(i: usize) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let before = windows.len();
    let target = pane_window_id(i);
    let undock;
    if let Some(idx) = windows.iter().position(|w| w.id == target) {
        windows.remove(idx); // dock back
        crate::split::dock_pane(i); // re-insert the leaf into the split-tree
        undock = false;
    } else {
        let tv = use_terminal();
        let (cols, rows) = tv.pane(i).session().dimensions();
        // Open at the pane's intrinsic (cols, rows) x cell — the grid_dims
        // inverse ([`cell_px`]) — so it fits 1:1 at tear-off. `OpenResizable`
        // (pinion R1059 / PINION-PR23) decouples the open size from the OS-resize
        // floor: `min: None` leaves the floor at the OS-native minimum, so the
        // user can freely GROW (the pane reflows larger) AND SHRINK below the open
        // size (reflows smaller) — both axes via the R1021 per-window publish.
        // `Fixed` would pin the floor at the open size (shrink blocked).
        let (width, height) = crate::terminal::cell_px(tv.metric, cols, rows);
        windows.push(WindowSpec::new(
            Cow::Owned(target),
            format!("sprag terminal — pane {i}"),
            SizeStrategy::OpenResizable {
                size: (width.max(1), height.max(1)),
                min: None,
            },
        ));
        crate::split::float_pane(i); // remove the leaf so the rest reclaim its space
        undock = true;
    }
    let after = windows.len();
    signal.set(windows);
    crate::diag::dock_toggle(i, undock, before, after);
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
            // "pane i has an OS window" = its `pane-{i}` window exists.
            let floating = |i: usize| windows.get().iter().any(|w| w.id == pane_window_id(i));
            assert_eq!(windows.get().len(), 1, "boots with the main window only");
            assert!(!floating(0), "pane 0 starts docked");

            // Undock pane 0: a second window appears and pane 0 reads as floating.
            toggle_pane_floating(0);
            assert_eq!(windows.get().len(), 2, "undock adds a window");
            assert!(floating(0), "pane 0 is now floating");
            assert!(!floating(1), "pane 1 stays docked");

            // Dock back: the window is removed, pane 0 docked again.
            toggle_pane_floating(0);
            assert_eq!(windows.get().len(), 1, "dock-back removes the window");
            assert!(!floating(0), "pane 0 is docked again");
        });
    }
}
