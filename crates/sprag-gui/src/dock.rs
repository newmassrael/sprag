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
//! ## OS windows vs dock-tree shape — the relationship depends on [`DockMode`] (R77)
//!
//! This module's `Signal<Vec<WindowSpec>>` is the **floating authority** — a `pane-{i}`
//! window exists IFF pane `i` floats. Its relationship to the dock split-tree
//! ([`crate::split::use_dock_topology`], the shape/ratio authority) is mode-dependent:
//!
//!  - **[`DockMode::Collapse`] (default):** float/dock CO-MUTATE both — [`push_float`]
//!    pushes the window AND removes the leaf ([`crate::split::float_pane`]) so the siblings
//!    reclaim the space; [`redock_pane`] drops the window AND re-inserts the leaf
//!    ([`crate::split::dock_pane`]). The tree tracks float state (the terminal-multiplexer
//!    fill).
//!  - **[`DockMode::Placeholder`] (opt-in):** float/dock are WINDOW-ONLY ([`push_float`] /
//!    [`redock_pane`] don't touch the tree); the leaf stays and the view paints a
//!    [`view_floating_placeholder`](pinion_widget_paint::dock::view_floating_placeholder)
//!    holding its slot. The two authorities are then ORTHOGONAL (R76) — this is R61's
//!    deferred "membership derived, not co-mutated" cleanup, available as the opt-in mode.
//!
//! All entry points route through the two primitives: [`open_floating`] (key path) and
//! [`float_pane_at`]'s create branch call [`push_float`]; both [`toggle_pane_floating`]
//! and the live redock call [`redock_pane`]. The docked set
//! ([`crate::split::docked_pane_indices`], read by both paint and a11y) is DERIVED in both
//! modes (the tree's leaves filtered by [`is_pane_floating`]), so it can never disagree
//! with the windows-signal. The tree is also restructured by reorganize gestures
//! (drag-to-dock + the cross-window zone-redock via `resolve_drop`) in both modes.
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
use std::cell::Cell;
use std::rc::Rc;

/// `Owner::cache` key for the runtime window topology Signal.
const WINDOWS_KEY: &str = "sprag_gui.windows";

/// The canonical main-window id (maps to pinion's `DEFAULT_WINDOW`).
pub(crate) const MAIN_WINDOW_ID: &str = "main";

/// The dock layout model — how a floated pane's slot in the main window is treated.
/// Selected by the `SPRAG_GUI_DOCK_MODE` env var ([`DockMode::from_env`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DockMode {
    /// R60, the DEFAULT: floating a pane REMOVES its leaf from the split-tree
    /// ([`crate::split::float_pane`]), so the remaining panes reclaim the freed space —
    /// the terminal-multiplexer fill (tmux/zellij). Docking back re-inserts the leaf
    /// index-relative ([`crate::split::dock_pane`]). float/dock CO-MUTATE the windows-signal
    /// + the split-tree (the tree tracks float state).
    Collapse,
    /// R72, opt-in (`SPRAG_GUI_DOCK_MODE=placeholder`): the floated pane's leaf STAYS in
    /// the tree; the view paints a placeholder holding its slot ([`crate::view`]). The
    /// windows-signal is the SOLE float authority, ORTHOGONAL to the tree (R76). This is
    /// the only mode where zone-honoring cross-window redock works (the surviving leaf is
    /// what the reducer's `resolve_drop` SSOT relocates to the drop zone), at the cost
    /// that siblings do NOT reclaim a floated pane's slot.
    Placeholder,
}

impl DockMode {
    /// Parse the `SPRAG_GUI_DOCK_MODE` env value: `placeholder` → [`Self::Placeholder`],
    /// anything else (incl. unset / unknown) → [`Self::Collapse`] (the default).
    fn from_env() -> Self {
        match std::env::var("SPRAG_GUI_DOCK_MODE").ok().as_deref() {
            Some("placeholder") => DockMode::Placeholder,
            _ => DockMode::Collapse,
        }
    }
}

/// `Owner::cache` key for the active dock-model Signal.
const DOCK_MODE_KEY: &str = "sprag_gui.dock_mode";

/// The active dock-model cell — `Owner::cache`d (per-owner), seeded once from the env
/// ([`DockMode::from_env`], default [`DockMode::Collapse`]). A `Cell` rather than a
/// process-global so the test suite can select EITHER mode in its own `Owner` scope
/// (`set_dock_mode`); a live run reads the env once and never changes it. (Not a
/// reactive `Signal` — the mode is a boot constant, not a paint input.)
fn use_dock_mode_cell() -> Rc<Cell<DockMode>> {
    Owner::current()
        .expect("use_dock_mode_cell() requires an active Owner scope")
        .cache(DOCK_MODE_KEY, || Cell::new(DockMode::from_env()))
}

/// The active dock model (default [`DockMode::Collapse`]). Read by [`push_float`] /
/// [`redock_pane`] to gate whether float/dock also mutate the split-tree.
pub(crate) fn dock_mode() -> DockMode {
    use_dock_mode_cell().get()
}

/// Test seam: force the dock model in the current `Owner` scope (call before any
/// float/dock op the test exercises).
#[cfg(test)]
pub(crate) fn set_dock_mode(mode: DockMode) {
    use_dock_mode_cell().set(mode);
}

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
            // Borderless main too (R85): both the main window and every floating pane
            // window are `decorations: false`, so the app's own client chrome
            // ([`crate::TerminalViewer::window_chrome`]) is the SOLE title bar — the two
            // windows look identical (no OS frame on one and an app strip on the other).
            // `decorations` is create-time-only (pinion app.rs warns on a runtime flip),
            // so it must be declared here at the seed, not toggled later.
            Signal::new(vec![
                WindowSpec::new(
                    Cow::Borrowed(MAIN_WINDOW_ID),
                    "sprag terminal (interactive)",
                    SizeStrategy::Fixed {
                        width: WINDOW_W,
                        height: WINDOW_H,
                    },
                )
                .with_decorations(false),
            ])
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
    // Borderless (pinion R1115 / PINION-PR38 ②′): the floating window paints its OWN
    // `view_dock_panel` header (R74) as the drag surface, so the OS draws no redundant
    // title bar over it — and "drag the title bar" unifies on that app header (the VS Code
    // / Blender way). With the OS decoration gone, dragging the app header IS the window
    // move (R1116 `with_floating_window` in `create_extra_externals` → the `WINDOW_MOVE`
    // reducer arm). The main window keeps the default `decorations: true`.
    let spec = spec.with_decorations(false);
    if let Some((x, y)) = position {
        spec.with_position(x, y)
    } else {
        spec
    }
}

/// `true` iff pane `i` currently floats — its `pane-{i}` OS window exists in the
/// windows-signal. The window-existence float authority in BOTH dock models: in
/// [`DockMode::Placeholder`] it is the SOLE authority (the tree always holds the leaf);
/// in [`DockMode::Collapse`] it agrees with the tree (a floated pane has no leaf AND a
/// window). Read by [`crate::split::docked_pane_indices`] and [`toggle_pane_floating`].
pub(crate) fn is_pane_floating(i: usize) -> bool {
    use_windows_topology()
        .get()
        .iter()
        .any(|w| w.id == pane_window_id(i))
}

/// In [`DockMode::Collapse`], floating pane `i` would EMPTY the main dock if it is the
/// ONLY docked pane: collapse removes its leaf, the tree empties to `None`, and an empty
/// dock has no interior drop target (only its 32px rim — pinion synthesizes an outer-dock
/// band from the window rect, but the interior is dead), so a floated pane could never be
/// dragged back. The user chose tmux/zellij semantics — the main window always keeps at
/// least ONE docked pane — so such a float is REFUSED (the last pane stays put). In
/// [`DockMode::Placeholder`] the leaf survives (the slot is held), so the dock never truly
/// empties and every pane may float.
fn float_would_empty_the_dock(i: usize) -> bool {
    if dock_mode() != DockMode::Collapse {
        return false;
    }
    let docked = crate::split::docked_pane_indices();
    docked.len() == 1 && docked.first() == Some(&i)
}

/// Float pane `i`: push its undock [`WindowSpec`] ([`undock_window_spec`], at
/// `position` if given) onto `windows`. The caller owns the `signal.set` + the `dock`
/// diag.
///
/// In [`DockMode::Collapse`] (default) it ALSO removes the pane's leaf from the split-
/// tree ([`crate::split::float_pane`]) so the siblings reclaim the space (the two
/// authorities co-mutate). In [`DockMode::Placeholder`] it is window-only — the leaf
/// stays and the view paints a placeholder. Shared by the key-path [`open_floating`] and
/// the live-follow create branch of [`float_pane_at`].
fn push_float(windows: &mut Vec<WindowSpec>, i: usize, position: Option<(i32, i32)>) {
    windows.push(undock_window_spec(i, position));
    if dock_mode() == DockMode::Collapse {
        crate::split::float_pane(i); // collapse: remove the leaf so the rest reclaim space
    }
}

/// Open pane `i` as a floating window at `position` (`None` → WM-placed; the key path
/// has no cursor). The discrete, cursor-less float: one topology `get`/`set` + the
/// `dock` diag, over [`push_float`]. Precondition: pane `i` is docked
/// ([`toggle_pane_floating`] gates on [`is_pane_floating`]), so a fresh open always
/// grows the window list by one. Single-responsibility — create only; the live-follow
/// reposition is [`float_pane_at`]'s, never here.
fn open_floating(i: usize, position: Option<(i32, i32)>) {
    if float_would_empty_the_dock(i) {
        return; // tmux semantics: the main window keeps its last docked pane (see helper)
    }
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
/// In [`DockMode::Collapse`] (default) it ALSO re-inserts the pane's leaf into the
/// split-tree index-relative ([`crate::split::dock_pane`]) — the leaf was removed on
/// float. For a redock-over-a-ZONE the reducer's `resolve_drop` relocate ran first but
/// REJECTED (the leaf is absent), so the pane lands at its INDEX home, not the drop zone
/// (the PINION-PR34 v1 bound — zone-honoring redock needs [`DockMode::Placeholder`]).
/// In [`DockMode::Placeholder`] it is window-only: the leaf never left, so de-floating
/// just drops the window (the view stops painting the placeholder and paints content,
/// re-tiled in place); a redock-over-a-zone was already relocated by the `resolve_drop`
/// SSOT (the surviving leaf is what it moves — this is why placeholder moots PINION-PR34).
pub(crate) fn redock_pane(i: usize) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    let Some(idx) = windows.iter().position(|w| w.id == target) else {
        return; // already docked
    };
    let before = windows.len();
    windows.remove(idx);
    if dock_mode() == DockMode::Collapse {
        crate::split::dock_pane(i); // collapse: re-insert the leaf (it was removed on float)
    }
    let after = windows.len();
    signal.set(windows);
    crate::diag::dock_toggle(i, false, before, after);
}

/// Live-follow tear-off (pinion R1094 / PINION-PR31): float pane `i` on the first
/// escaped drag move and track the cursor on every move after. `cursor` is the pointer
/// the [`DockPanelExternal`](pinion_widget_paint::dock::DockPanelExternal) forwards,
/// measured in `source_window`'s frame (pinion R1107); it is desktop-converted by
/// pinion's [`desktop_position_from`](pinion_shell::desktop_position_from) — the SSOT
/// pinion R1107.1 lifted to the shell so the consumer needn't re-derive it — and written
/// as the floating window's outer position.
///
/// **`source_window` is load-bearing for dragging a SETTLED floating window:** its header
/// reports its OWN `pane-{i}` frame (not main), so the conversion must add THAT window's
/// origin. `None` → main (the docked-pane tear-off case). Hardcoding main here was the
/// "undocked window won't drag" bug (R78): a settled floater's local cursor + main origin
/// = a bogus desktop point, so the window jumped/froze.
///
/// ONE topology borrow: the source-window origin (`desktop_position_from`) and the window
/// being repositioned come from the SAME snapshot, so a concurrent `WindowEvent::Moved`
/// write-back can't make the position stale against the list it is written into. Two
/// phases over that snapshot:
/// * docked (no `pane-{i}` window) → [`push_float`] at the cursor (a window push + the
///   `dock` diag; the leaf stays in placeholder mode, R72);
/// * floating → move the window (position only, no `dock` diag); a stationary cursor
///   equality-skips the `set` (no repaint).
///
/// Non-toggling: a per-move re-emit only repositions, it can never flip the window
/// away (the R1071–R1078 double-toggle lesson, sprag side). Key/AI dock-back is
/// [`redock_pane`].
pub(crate) fn float_pane_at(i: usize, source_window: Option<&str>, cursor: (f64, f64)) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let pos = pinion_shell::desktop_position_from(&windows, source_window, cursor);
    let target = pane_window_id(i);
    if let Some(spec) = windows.iter_mut().find(|w| w.id == target) {
        // Floating: reposition only.
        if spec.position == Some(pos) {
            return; // stationary cursor -> no set, no repaint
        }
        spec.position = Some(pos);
    } else {
        // First escaped move: float at the cursor — UNLESS this is the last docked pane
        // in collapse mode (keep >=1 docked, the tmux semantics the user chose). Then the
        // tear-off does nothing and the pane stays put.
        if float_would_empty_the_dock(i) {
            return;
        }
        let before = windows.len();
        push_float(&mut windows, i, Some(pos));
        crate::diag::dock_toggle(i, true, before, windows.len());
    }
    signal.set(windows);
}

/// Borderless title-bar window move (pinion R1116/R1118 / PINION-PR38 ②): relocate pane
/// `i`'s floating window by a grab-relative `delta` (the window's header was dragged, so
/// `new_pos = current_pos + delta` keeps the grabbed point under the cursor). Distinct
/// from [`float_pane_at`], which PLACES a torn-off pane AT an absolute cursor; this moves
/// an already-floating window by a displacement. Idempotent no-op if pane `i` isn't
/// floating (a stray move with no window). Mirrors the editor reference's
/// `move_floating_window`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "logical-pixel displacement f64 -> i32 outer position; sub-pixel is irrelevant to window placement"
)]
pub(crate) fn move_floating_window(i: usize, delta: (f64, f64)) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    if let Some(spec) = windows.iter_mut().find(|w| w.id == target) {
        let (x, y) = spec.position.unwrap_or((0, 0));
        let next = (x + delta.0.round() as i32, y + delta.1.round() as i32);
        if spec.position == Some(next) {
            return; // zero delta -> no set, no repaint
        }
        spec.position = Some(next);
        signal.set(windows);
    }
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

    /// R87 (tmux semantics, user's choice): in COLLAPSE mode the main window keeps at
    /// least one docked pane — floating the LAST docked pane is refused, so the dock never
    /// empties to a state with no drop target (an empty dock can't be dragged back into).
    /// PLACEHOLDER mode keeps the leaf (held slot), so the dock never truly empties and
    /// every pane may float.
    #[test]
    fn collapse_refuses_to_float_the_last_docked_pane_placeholder_allows_it() {
        let owner = Owner::new();
        owner.run(|| {
            let windows = use_windows_topology();
            let floating = |i: usize| windows.get().iter().any(|w| w.id == pane_window_id(i));

            // COLLAPSE (default): float pane 0 (pane 1 still docked — allowed) ...
            set_dock_mode(DockMode::Collapse);
            toggle_pane_floating(0);
            assert!(
                floating(0),
                "pane 0 floats (pane 1 keeps the dock non-empty)"
            );
            // ... then floating pane 1, the LAST docked pane, is REFUSED.
            toggle_pane_floating(1);
            assert!(
                !floating(1),
                "the last docked pane stays put (tmux semantics: dock keeps >=1 pane)"
            );
            // Restore: dock pane 0 back for a clean placeholder run.
            toggle_pane_floating(0);
            assert!(!floating(0) && !floating(1), "both docked again");

            // PLACEHOLDER: the leaf survives, so even the last pane may float.
            set_dock_mode(DockMode::Placeholder);
            toggle_pane_floating(0);
            toggle_pane_floating(1);
            assert!(
                floating(0) && floating(1),
                "placeholder mode lets every pane float (the held leaf is the drop target)"
            );
        });
    }
}
