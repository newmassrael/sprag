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
//! ## Two ORTHOGONAL authorities: OS windows vs dock-tree shape (R72 placeholder model)
//!
//! This module's `Signal<Vec<WindowSpec>>` is the **sole floating authority** — a
//! `pane-{i}` window exists IFF pane `i` floats. The dock split-tree
//! ([`crate::split::use_dock_topology`]) is the **sole shape/ratio authority** and ALWAYS
//! holds every pane's leaf (floating or not). The two are ORTHOGONAL and **never
//! co-mutated**: floating a pane only pushes/removes a `WindowSpec` ([`push_float`] /
//! [`redock_pane`] are window-only); the leaf stays put, and the view paints a
//! [`view_floating_placeholder`](pinion_widget_paint::dock::view_floating_placeholder)
//! for a floating leaf (holding its slot). All entry points route through the two
//! window-only primitives: [`open_floating`] (key path) and [`float_pane_at`]'s create
//! branch call [`push_float`]; both [`toggle_pane_floating`] and the live redock call
//! [`redock_pane`].
//!
//! The docked set ([`crate::split::docked_pane_indices`], read by both paint and a11y) is
//! now DERIVED — the tree's leaves filtered by [`is_pane_floating`] — so it can never
//! disagree with the windows-signal. This lands R61's deferred "membership is a
//! projection of one authority" cleanup: the tree is no longer co-mutated to track float
//! state, it is filtered. The tree is restructured ONLY by a reorganize gesture
//! (drag-to-dock + the cross-window zone-redock `apply_zone_redock`), never by a plain
//! float/dock. Mirrors pinion's `hello-dock-panels-editor` reference consumer.
//!
//! ## Why the undock window opens at the pane's intrinsic size (and now reflows)
//!
//! The undock window opens sized to the pane's intrinsic `(cols, rows) × cell`, so
//! the pane fits 1:1 at the moment it tears off — the correct intrinsic size, NOT
//! window-side split math (the SSOT trap). It then reflows toward its own window size
//! as the window resizes: pinion R1021 publishes the per-pane viewport rect for every
//! painted window (the R1012 publish is no longer `DEFAULT_WINDOW`-gated — see
//! `pinion-shell` `compute_paint_scene_internal`, "R1021 … published for EVERY painted
//! window"), so the floated pane's existing [`crate::reflow`] Effect (it subscribes to
//! `use_pane_viewport_size(pane_tag(i))`) fires on the secondary window's rect and
//! `TIOCSWINSZ`-reflows the PTY. This is the consumer of
//! `claudedocs/PINION-PR10-PER-WINDOW-VIEWPORT.md` (DELIVERED). R74 wraps the lone pane
//! in a [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel) header (the
//! drag-back source) with its content given a definite extent via `view`'s
//! `fill_definite_shrinkable`. **WIDTH reflows; HEIGHT does not reflow
//! below the pane's boot content** — pinion's `view_dock_panel` content wrapper lacks
//! the main-axis `min_size:0` that `view_splitter` carries (PINION-PR35), so a
//! shrunk-below-open floating window overflows vertically (fits 1:1 at the open size).
//! See `fill_definite_shrinkable`'s docs.
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
    if let Some((x, y)) = position {
        spec.with_position(x, y)
    } else {
        spec
    }
}

/// `true` iff pane `i` currently floats — its `pane-{i}` OS window exists in the
/// windows-signal. In the placeholder model (R72) this is the SOLE floating authority:
/// the dock split-tree always holds the pane's leaf (floating or not), so window
/// existence alone decides float state. Read by [`crate::split::docked_pane_indices`]
/// to derive the docked set, and by [`toggle_pane_floating`] to pick its branch.
pub(crate) fn is_pane_floating(i: usize) -> bool {
    use_windows_topology()
        .get()
        .iter()
        .any(|w| w.id == pane_window_id(i))
}

/// Float pane `i`: push its undock [`WindowSpec`] ([`undock_window_spec`], at
/// `position` if given) onto `windows`. The caller owns the `signal.set` + the `dock`
/// diag.
///
/// Window-only (R72 placeholder model): the dock split-tree is NOT touched — the pane's
/// leaf stays in the topology and the view paints a placeholder for it while it floats.
/// The windows-signal is the SOLE floating authority; pushing the window IS the float.
/// Shared by the key-path [`open_floating`] and the live-follow create branch of
/// [`float_pane_at`].
fn push_float(windows: &mut Vec<WindowSpec>, i: usize, position: Option<(i32, i32)>) {
    windows.push(undock_window_spec(i, position));
}

/// Open pane `i` as a floating window at `position` (`None` → WM-placed; the key path
/// has no cursor). The discrete, cursor-less float: one topology `get`/`set` + the
/// `dock` diag, over [`push_float`]. Precondition: pane `i` is docked
/// ([`toggle_pane_floating`] gates on [`is_pane_floating`]), so a fresh open always
/// grows the window list by one. Single-responsibility — create only; the live-follow
/// reposition is [`float_pane_at`]'s, never here.
fn open_floating(i: usize, position: Option<(i32, i32)>) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let before = windows.len();
    push_float(&mut windows, i, position);
    let after = windows.len();
    signal.set(windows);
    crate::diag::dock_toggle(i, true, before, after);
}

/// Dock pane `i` back: drop its floating window. Idempotent no-op when pane `i` is
/// already docked (R1094 emits a redock/restore for a snap-back too, so a pane this
/// gesture never floated is harmless). Shared by [`toggle_pane_floating`]'s dock-back
/// branch and the live redock/restore (pinion R1094 / PINION-PR31).
///
/// Window-only (R72 placeholder model): the leaf never left the topology, so de-floating
/// is just removing the window — the view stops painting the placeholder for that leaf
/// and paints its content instead, re-tiled in place (it reflows via the main-window
/// R1012 publish). For a redock-over-a-ZONE the reducer ([`crate::TerminalViewer`]'s
/// `WidgetCore::update`) relocates the leaf to the drop zone via the reorganizer's
/// `apply_zone_redock` BEFORE calling this; here we only drop the window. (This is why
/// the placeholder model moots PINION-PR34: the source leaf survives, so the zone
/// relocate can't reject on an absent leaf.)
pub(crate) fn redock_pane(i: usize) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    let Some(idx) = windows.iter().position(|w| w.id == target) else {
        return; // already docked
    };
    let before = windows.len();
    windows.remove(idx);
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

/// Live-follow tear-off (pinion R1094 / PINION-PR31): float pane `i` on the first
/// escaped drag move and track the cursor on every move after. `cursor` is the
/// MAIN-window-logical pointer the [`DockPanelExternal`](pinion_widget_paint::dock::DockPanelExternal)
/// forwards; it is desktop-converted ([`cursor_to_desktop`]) and written as the
/// floating window's outer position.
///
/// ONE topology borrow: the main-window origin (read by [`cursor_to_desktop`]) and the
/// window being repositioned come from the SAME snapshot, so a concurrent
/// `WindowEvent::Moved` write-back can't make the computed position stale against the
/// list it is written into. Two phases over that snapshot:
/// * docked (no `pane-{i}` window) → [`push_float`] at the cursor (a window push + the
///   `dock` diag; the leaf stays, R72);
/// * floating → move the window (position only, no `dock` diag); a stationary cursor
///   equality-skips the `set` (no repaint).
///
/// Non-toggling: a per-move re-emit only repositions, it can never flip the window
/// away (the R1071–R1078 double-toggle lesson, sprag side). Key/AI dock-back is
/// [`redock_pane`].
pub(crate) fn float_pane_at(i: usize, cursor: (f64, f64)) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let pos = cursor_to_desktop(&windows, cursor);
    let target = pane_window_id(i);
    if let Some(spec) = windows.iter_mut().find(|w| w.id == target) {
        // Floating: reposition only.
        if spec.position == Some(pos) {
            return; // stationary cursor -> no set, no repaint
        }
        spec.position = Some(pos);
    } else {
        // First escaped move: float at the cursor.
        let before = windows.len();
        push_float(&mut windows, i, Some(pos));
        crate::diag::dock_toggle(i, true, before, windows.len());
    }
    signal.set(windows);
}

/// Toggle pane `i` between docked (tiled in the main window) and undocked (its own OS
/// window) — the discrete `Ctrl+Shift+Enter` key path and the release-driven
/// `tear_off` fallback (pinion fires the legacy `tear_off` only when no drag cursor
/// was forwarded). Dispatches on [`is_pane_floating`] to the non-toggling primitives:
/// [`redock_pane`] (dock-back) or [`open_floating`]`(None)` (WM-placed float, the key
/// path has no cursor). So all three entry points — toggle, [`float_pane_at`],
/// [`redock_pane`] — drive the windows-signal only ([`push_float`] window push /
/// [`redock_pane`] window drop); the split-tree is untouched (R72).
///
/// Runs inside the shell root owner scope (called from `route_key`, itself wrapped
/// in `root_owner.run`), so [`use_terminal`] / [`use_windows_topology`] / the
/// split-tree hooks resolve.
pub(crate) fn toggle_pane_floating(i: usize) {
    if is_pane_floating(i) {
        redock_pane(i);
    } else {
        open_floating(i, None);
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
