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
//! and the dock/undock entry points: the discrete [`toggle_pane_floating`] (the
//! `Ctrl+Shift+Enter` key path + the release-driven `tear_off` fallback) and the
//! live drag-follow [`float_pane_at`] / [`redock_pane`] (pinion R1094 / PINION-PR31's
//! `tear_off_follow` / `tear_off_redock` seam — non-toggling, so a per-move re-emit
//! only repositions or restores, never flips).
//!
//! ## Two distinct authorities: OS windows vs the docked-pane set
//!
//! This module's `Signal<Vec<WindowSpec>>` is the authority for **which OS windows
//! exist** — a `pane-{i}` window exists iff pane `i` is floating. That is a SEPARATE
//! fact from **which panes are docked in the main window**, which is the dock split-
//! tree's leaf set ([`crate::split::docked_pane_indices`]) — the one source both the
//! paint ([`crate::view::view_for_window`]) and a11y read, so they never disagree. The two
//! authorities are kept consistent at the two co-mutation sites in this module —
//! [`ensure_pane_floating`] (float: push window + remove leaf) and [`redock_pane`]
//! (dock: drop window + re-insert leaf) — so a pane floats iff its window exists AND
//! its leaf is absent from the tree. All three entry points route through them
//! ([`toggle_pane_floating`] picks one by the current state; [`float_pane_at`] is
//! `ensure_pane_floating` with a position). PR-31's live drag-follow (R1094) is the
//! "second mutation path" the prior note anticipated; it preserves the invariant by
//! reusing these same two sites. Making the docked set a reactive projection of one
//! authority (rather than co-mutated) is still the eventual cleanup.
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

/// Build an undock `WindowSpec` for pane `i`, optionally opened at a declared outer
/// `position`. Shared by the two create paths: [`toggle_pane_floating`] passes
/// `None` (WM-placed, the key path declares no position) and [`float_pane_at`]
/// passes the live-follow desktop cursor. Sized to the pane's intrinsic
/// `(cols, rows) × cell` — the [`cell_px`](crate::terminal::cell_px) inverse — so it
/// fits 1:1 at tear-off. `OpenResizable` (pinion R1059 / PINION-PR23) decouples the
/// open size from the OS-resize floor (`min: None` leaves the floor at the OS-native
/// minimum), so the window grows AND shrinks below the open size — both axes via the
/// R1021 per-window publish; `Fixed` would pin the floor at the open size (shrink
/// blocked). The pane's `(cols, rows)` is read from its live session — its own
/// authoritative dims, NOT a window-side split calc.
fn undock_window_spec(i: usize, position: Option<(i32, i32)>) -> WindowSpec {
    let tv = use_terminal();
    let (cols, rows) = tv.pane(i).session().dimensions();
    let (width, height) = crate::terminal::cell_px(tv.metric, cols, rows);
    let spec = WindowSpec::new(
        Cow::Owned(pane_window_id(i)),
        format!("sprag terminal — pane {i}"),
        SizeStrategy::OpenResizable {
            size: (width.max(1), height.max(1)),
            min: None,
        },
    );
    match position {
        Some((x, y)) => spec.with_position(x, y),
        None => spec,
    }
}

/// `true` iff pane `i` currently floats — its `pane-{i}` OS window exists in the
/// topology. The window-existence half of the float fact (the split-tree leaf's
/// absence is the other half, kept consistent at the co-mutation sites below).
fn is_pane_floating(i: usize) -> bool {
    use_windows_topology()
        .get()
        .iter()
        .any(|w| w.id == pane_window_id(i))
}

/// Ensure pane `i` floats, optionally (re)positioning its window at desktop
/// `position`. Non-toggling and idempotent:
///
/// * docked (no `pane-{i}` window) -> push an undock [`WindowSpec`]
///   ([`undock_window_spec`], at `position` if given) and remove its leaf from the
///   dock split-tree ([`crate::split::float_pane`]) so the remaining docked panes
///   reclaim its space;
/// * already floating -> if `position` is `Some`, move the window there (the live-
///   follow reposition — no tree change, no `dock` diag); a `None` reposition (the
///   toggle never repositions) is a no-op.
///
/// The shared worker behind [`toggle_pane_floating`]'s float branch (`None`) and
/// [`float_pane_at`] (the live cursor). The two authorities — the window topology
/// (the floating SSOT) and the dock split-tree — are mutated together at this one
/// create site so they never disagree (a pane floats iff its window exists AND its
/// leaf is absent from the tree).
fn ensure_pane_floating(i: usize, position: Option<(i32, i32)>) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    if let Some(spec) = windows.iter_mut().find(|w| w.id == target) {
        // Already floating: a live-follow reposition only (no tree change).
        let Some(pos) = position else { return };
        if spec.position == Some(pos) {
            return; // stationary cursor -> skip the redundant set + repaint
        }
        spec.position = Some(pos);
        signal.set(windows);
        return;
    }
    let before = windows.len();
    windows.push(undock_window_spec(i, position));
    crate::split::float_pane(i); // remove the leaf so the rest reclaim its space
    let after = windows.len();
    signal.set(windows);
    crate::diag::dock_toggle(i, true, before, after);
}

/// Ensure pane `i` is docked: drop its floating window and re-insert its leaf into
/// the dock split-tree ([`crate::split::dock_pane`]). Idempotent no-op when pane `i`
/// is already docked — R1094 emits a redock/restore for a snap-back too, so a pane
/// this gesture never floated must be harmless. Non-toggling; the second authority-
/// co-mutation site (with [`ensure_pane_floating`]). The shell drops the winit
/// window; the main layout repaints with pane `i` re-tiled (it reflows to its new
/// tile via the main-window R1012 publish). Shared by [`toggle_pane_floating`]'s
/// dock-back branch and the live redock/restore (pinion R1094 / PINION-PR31).
pub(crate) fn redock_pane(i: usize) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    let Some(idx) = windows.iter().position(|w| w.id == target) else {
        return; // already docked
    };
    let before = windows.len();
    windows.remove(idx);
    crate::split::dock_pane(i); // re-insert the leaf into the split-tree
    let after = windows.len();
    signal.set(windows);
    crate::diag::dock_toggle(i, false, before, after);
}

/// Desktop outer position for pane `i`'s floating window from a MAIN-window-logical
/// `cursor` (the frame the [`DockPanelExternal`](pinion_widget_paint::dock::DockPanelExternal)
/// reports — the widget layer must not know about OS windows): the main window's
/// declared outer origin + the cursor, so the floating window opens at the desktop
/// point under the pointer. A WM-placed main window pinion never learned a position
/// for falls back to the desktop origin `(0, 0)` — the window still *tracks* the
/// cursor, just offset from the origin rather than the real frame. Mirrors the
/// R1094 reference consumer's `follow_desktop_position`; the main origin is read
/// from the topology (the R1088 `WindowEvent::Moved` write-back keeps it current).
#[allow(
    clippy::cast_possible_truncation,
    reason = "logical-pixel cursor -> i32 outer position; sub-pixel is irrelevant to window placement"
)]
fn cursor_to_desktop(windows: &[WindowSpec], cursor: (f64, f64)) -> (i32, i32) {
    let (ox, oy) = windows
        .iter()
        .find(|w| w.id.as_ref() == MAIN_WINDOW_ID)
        .and_then(|w| w.position)
        .unwrap_or((0, 0));
    (ox + cursor.0.round() as i32, oy + cursor.1.round() as i32)
}

/// Live-follow tear-off (pinion R1094 / PINION-PR31): ensure pane `i` floats and
/// track the drag. `cursor` is the MAIN-window-logical pointer the
/// [`DockPanelExternal`](pinion_widget_paint::dock::DockPanelExternal) forwards on
/// each escaped drag move (and the escape-release); it is desktop-converted
/// ([`cursor_to_desktop`]) and written as the floating window's outer position.
/// Non-toggling — the first escaped move creates the window, every later move only
/// repositions it (the equality-skip in [`ensure_pane_floating`] collapses a
/// stationary cursor to no repaint). The key/AI dock-back stays on
/// [`toggle_pane_floating`] / [`redock_pane`], so a per-move re-emit can never flip
/// the window away (the R1071-R1078 double-toggle lesson, on the sprag side).
pub(crate) fn float_pane_at(i: usize, cursor: (f64, f64)) {
    let pos = cursor_to_desktop(&use_windows_topology().get(), cursor);
    ensure_pane_floating(i, Some(pos));
}

/// Toggle pane `i` between docked (tiled in the main window) and undocked (its own
/// OS window) — the discrete `Ctrl+Shift+Enter` key path and the release-driven
/// `tear_off` fallback (pinion fires the legacy `tear_off` only when no drag cursor
/// was forwarded). Delegates to the non-toggling [`redock_pane`] / [`ensure_pane_floating`]
/// primitives (a `None` position → WM-placed, as the key path has no cursor), so all
/// three entry points (toggle, [`float_pane_at`], [`redock_pane`]) share the one
/// authority-co-mutation seam.
///
/// Runs inside the shell root owner scope (called from `route_key`, itself wrapped
/// in `root_owner.run`), so [`use_terminal`] / [`use_windows_topology`] / the
/// split-tree hooks resolve.
pub(crate) fn toggle_pane_floating(i: usize) {
    if is_pane_floating(i) {
        redock_pane(i);
    } else {
        ensure_pane_floating(i, None);
    }
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
