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
//! ## OS windows vs dock-tree shape (R149)
//!
//! Two authorities, on opposite sides of the wire, and they do not overlap. The windows
//! signal ([`use_windows_topology`]) owns WHERE a floating pane's OS window sits — pixels,
//! this client's alone. The HOST owns WHICH panes are tiled and how ([`crate::split`]); a
//! float takes the pane out of the tiling, so its leaf collapses and the siblings reclaim
//! the space (the terminal-multiplexer fill). [`push_float`] / [`redock_pane`] therefore do
//! two things: move the OS window, and ASK the host to change the tiling — then adopt the
//! arrangement it answers with. Neither invents a tiling of its own; that is what R149
//! retired, along with the selectable placeholder model (see [`crate::split`]'s module docs).
//!
//! The docked set ([`crate::split::docked_pane_indices`], read by both paint and a11y) is
//! DERIVED from the projected tree's leaves, so it cannot disagree with what the host tiles.
//! The tree is also restructured by reorganize gestures (drag-to-dock + the cross-window
//! zone-redock via `resolve_drop`), which [`crate::split::sync_layout`] writes back.
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
//! `fill_definite_shrinkable`. **WIDTH and HEIGHT both reflow, including BELOW the pane's
//! boot content** — PINION-PR35 was DELIVERED at the R78 bump (pinion R1109 gave
//! `view_dock_panel`'s content wrapper the `view_splitter` idiom: `flex_basis:0 +
//! flex_grow:1 + min-height:0`), so `fill_definite_shrinkable` supplies the content-side
//! `min_size.height:0` that composes with it and a shrunk-below-open floating window
//! reflows instead of overflowing. Guarded by `undock_window_reflows_its_height_below_boot_content`.
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

/// The app-name token every window title shares. Single-sourced here so the pane-title
/// prefix ([`window_title_for_pane`]) has one home; the no-focus main title
/// [`MAIN_WINDOW_TITLE`] embeds the same token in its `"(interactive)"` form as a
/// `&'static str` (the window seed + [`WidgetCore::title`](crate::TerminalViewer) both need
/// one, and `const` cannot compose it from this), so a rebrand touches these two literals.
const APP_NAME: &str = "sprag terminal";

/// The MAIN window's title when no pane is focused: the seed, the no-focus fallback of
/// [`sync_main_title`], and the windowless [`WidgetCore::title`](crate::TerminalViewer)
/// value — the ONE home for THIS exact string (its three uses cannot drift). The
/// focused-pane form is composed separately from [`APP_NAME`] by [`window_title_for_pane`].
pub(crate) const MAIN_WINDOW_TITLE: &str = "sprag terminal (interactive)";

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
            // window are `decorations: false`, so the OS draws no title bar on either.
            // The MAIN window's title bar is the app's client chrome
            // ([`crate::TerminalViewer::window_chrome`]); a FLOATER's is its dock-panel
            // HEADER (R95 controls-in-header — `window_chrome` returns `None` for it).
            // `decorations` is create-time-only (pinion app.rs warns on a runtime flip),
            // so it must be declared here at the seed, not toggled later.
            Signal::new(vec![
                WindowSpec::new(
                    Cow::Borrowed(MAIN_WINDOW_ID),
                    MAIN_WINDOW_TITLE,
                    SizeStrategy::Fixed {
                        width: WINDOW_W,
                        height: WINDOW_H,
                    },
                )
                .with_decorations(false),
            ])
        })
}

/// An OS-window title naming pane `i`: its DISPLAY title
/// ([`crate::view::pane_display_title`], so it reads the same as the pane's dock header)
/// behind a stable [`APP_NAME`]` — ` prefix, since this string lands in the taskbar /
/// alt-tab switcher where a bare "vim README" loses the app. Shared by every OS-title site
/// (a floater's own window in [`sync_floating_titles`] / [`undock_window_spec`], and the
/// focused-pane MAIN title in [`sync_main_title`]) so all read one format.
///
/// ONE fallback policy: when the child has set no `OSC` title the display title is the
/// stable `terminal-{i}`, so this reads "sprag terminal — terminal-0" rather than
/// inventing a second naming rule.
fn window_title_for_pane(i: usize) -> String {
    let tv = use_terminal();
    format!(
        "{APP_NAME} — {}",
        crate::view::pane_display_title(&tv.slots, i)
    )
}

/// Retitle windows in place, hosting the diff-before-`set` discipline ONCE: recompute each
/// spec's desired title via `want_for` (returning `None` leaves that spec untouched) and
/// `set` the windows signal only if at least one title actually changed. A windows `set`
/// posts `AppEvent::WindowsDirty` (a deferred winit reconcile), so a blind per-frame `set`
/// would reconcile every paint — the diff is what keeps the pre-view `reconcile_frame`
/// callers ([`sync_main_title`] / [`sync_floating_titles`]) quiescent in steady state.
fn retitle_windows(want_for: impl Fn(&WindowSpec) -> Option<String>) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let mut changed = false;
    for spec in &mut windows {
        if let Some(want) = want_for(spec)
            && spec.title != want
        {
            spec.title = want;
            changed = true;
        }
    }
    if changed {
        signal.set(windows);
    }
}

/// (R132, pinion R1327 / PINION-PR53) Track the MAIN window's OS title to the FOCUSED
/// pane's display title — the tmux / gnome-terminal convention (the active pane names the
/// window). `focused_pane` is the tile the focus ring is on, or `None` when focus is off a
/// pane (nothing focused, or a non-pane element); then the title falls back to the plain
/// app name [`MAIN_WINDOW_TITLE`].
///
/// The focused pane names the main window ONLY when it is DOCKED. Focus is binding-wide (a
/// single tag across every window — pinion `focus_state`), and a torn-off pane keeps its
/// slot OCCUPIED, so a focused FLOATING pane would otherwise leak its title onto the main
/// window even though it is painted in its own window (which [`sync_floating_titles`]
/// already titles). The `!`[`is_pane_floating`] guard resolves "which window is that pane
/// in" via the sole float authority — a focused floater leaves the main window at the app
/// name (R133; the R132 test only covered the both-docked case).
///
/// This is the last title surface PINION-PR52/53 opened: the docked header + floater OS
/// title (R130) needed no focus, but the MAIN window tiles several panes, so "which pane's
/// title" is decided by FOCUS — which the binding could not read in the paint path until
/// pinion R1327's [`focus_state`](pinion_core::focus_state). The caller reads
/// `focus_state::focused()` in [`reconcile_frame`](crate::TerminalViewer) (root Owner
/// scope, so that read auto-subscribes and repaints on a focus change) and maps the tag to
/// a pane index.
pub(crate) fn sync_main_title(focused_pane: Option<usize>) {
    let want = match focused_pane {
        Some(i) if use_terminal().slots.is_pane_occupied(i) && !is_pane_floating(i) => {
            window_title_for_pane(i)
        }
        _ => MAIN_WINDOW_TITLE.to_owned(),
    };
    retitle_windows(|spec| (spec.id.as_ref() == MAIN_WINDOW_ID).then(|| want.clone()));
}

/// (R130, pinion R1319 / PINION-PR52-B) Keep every FLOATING window's OS title in sync with
/// its pane's live display title — a child that retitles to `vim README` renames its
/// taskbar entry too, the way a real terminal does. Pre-R1319 `WindowSpec::title` was
/// create-time-only; the shell now diffs spec titles and issues `Window::set_title` on an
/// OPEN window, and the binding is the sole title authority, so the sync is driven here
/// from the pre-view `reconcile_frame`. The MAIN window is skipped ([`window_title_for_pane`]
/// is keyed by [`pane_window_index`], `None` for the main id) — it is focus-driven by
/// [`sync_main_title`] instead.
pub(crate) fn sync_floating_titles() {
    retitle_windows(|spec| pane_window_index(spec.id.as_ref()).map(window_title_for_pane));
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
    let (cols, rows) = tv.slots.pane_grid_size(i);
    let (width, height) = crate::terminal::cell_px(tv.metric, cols, rows);
    let spec = WindowSpec::new(
        Cow::Owned(pane_window_id(i)),
        // Same title source as the live sync ([`sync_floating_titles`]), so the window
        // opens with the child's title already showing rather than a create-time name the
        // first sync then replaces.
        window_title_for_pane(i),
        SizeStrategy::OpenResizable {
            size: (width.max(1), height.max(1)),
            min: None,
        },
    );
    // Borderless (pinion R1115 / PINION-PR38 ②′): the floating window paints its OWN
    // dock-panel header (R74) as the drag surface, so the OS draws no redundant
    // title bar over it — and "drag the title bar" unifies on that app header (the VS Code
    // / Blender way). With the OS decoration gone, dragging the app header IS the window
    // move (R1116 `with_floating_window` in `create_extra_externals` → the `WINDOW_MOVE`
    // reducer arm). Since R95 that header is the floater's SOLE strip — it hosts the
    // window controls too (controls-in-header, `window_chrome == None`); the main window
    // stays borderless-with-app-chrome (R85, see `use_windows_topology`'s main seed).
    let spec = spec.with_decorations(false);
    if let Some((x, y)) = position {
        spec.with_position(x, y)
    } else {
        spec
    }
}

/// `true` iff pane `i` currently floats — its `pane-{i}` OS window exists in the
/// windows-signal.
///
/// This client's own view of the float, and it agrees with the host's by construction: a
/// floated pane has a window here AND no leaf in the projected tree, because the same call
/// ([`push_float`] / [`redock_pane`]) does both. Read by [`toggle_pane_floating`] to pick
/// its branch, and by [`sync_main_title`] to resolve which window a pane's title belongs to.
pub(crate) fn is_pane_floating(i: usize) -> bool {
    use_windows_topology()
        .get()
        .iter()
        .any(|w| w.id == pane_window_id(i))
}

/// Floating pane `i` would EMPTY the main dock if it is the ONLY docked pane: the float
/// removes its leaf and the tree empties to `None`. The PRIMARY
/// reason to refuse this is a product decision the user chose — tmux/zellij semantics: a
/// terminal window always shows at least one terminal, so the main window keeps at least
/// ONE docked pane and such a float is REFUSED (the last pane stays put). A secondary
/// motivation is that an empty dock is a poor drop surface — pinion synthesizes an
/// outer-dock band from the window rect (its 32px rim) but the interior is dead, so
/// dragging a floater back onto an empty main is rim-only, not the whole window.
///
/// SCOPE (R123): this is a FLOAT-GESTURE guard — it stops the USER stranding themselves by
/// floating their last docked pane. It is deliberately NOT a global "the main is never
/// empty" invariant: a host-side CLOSE ([`evict_pane`]) is authoritative (the pane is gone)
/// and may legitimately empty the dock — including the case where a floater survives — since
/// force-redocking the user's deliberately-floated pane would be more surprising than a
/// blank main (which the all-panes-closed path already produces). So there is ONE enforcer
/// of this guard ([`push_float`]); `evict_pane` is a different event class, not subject to it.
fn float_would_empty_the_dock(i: usize) -> bool {
    let docked = crate::split::docked_pane_indices();
    docked.len() == 1 && docked.first() == Some(&i)
}

/// Whether pane `i`'s dock-panel header may START a drag (tear-off / reorder). FALSE only
/// for the sole docked pane — the one [`float_would_empty_the_dock`]
/// refuses to float. Wired to pinion's `DockPanelExternal::with_movable` in
/// `create_extra_externals`: the INTENT is to block the drag AT THE SOURCE (`begin_drag`
/// returns `None` → no drag session → no drag-image chip and no drop-preview for a pane
/// that cannot move), fixing the "why does a pane that can't move show a preview?"
/// inconsistency the reducer-level refuse left.
///
/// **NOT LIVE YET — blocked on PINION-PR42.** `reconcile_externals` early-returns on an
/// unchanged tag set (sprag's tags are constant, R70), so the rebuilt external carrying
/// `movable=false` is DISCARDED and the boot value (both panes movable) sticks — the flag
/// is create-time-only. So today the chip/preview STILL (wrongly) shows; behavior stays
/// correct only via the `float_would_empty_the_dock` gate in the float primitives. This
/// predicate + wiring are the correct forward seam (sprag change 0 when PR-42 lands). The
/// factory re-runs per float/dock (R70) and computes the right flag — with 2+ docked panes
/// every pane is movable; float one and the sole pane computes non-movable — pinion just
/// does not yet APPLY it after boot.
pub(crate) fn pane_is_movable(i: usize) -> bool {
    !float_would_empty_the_dock(i)
}

/// Float pane `i`: push its undock [`WindowSpec`] ([`undock_window_spec`], at `position`
/// if given) onto `windows`. Returns `true` if it floated, `false` if REFUSED because the
/// float would empty the dock ([`float_would_empty_the_dock`], the tmux-semantics
/// invariant). The caller owns the `signal.set` + the `dock` diag, and must skip both when
/// this returns `false` (nothing changed).
///
/// The last-pane invariant lives HERE, in the one primitive that actually empties the dock
/// — so every float entry-point ([`open_floating`], [`float_pane_at`]) funnels through it
/// and a future third caller cannot forget the gate (the same single-enforcement-point
/// rule `view::compose` / `fill_definite` follow; the R55 undock bug was a forgotten fill).
///
/// It also asks the HOST to take the pane out of the tiling, and adopts the arrangement it
/// answers with — which is where the leaf collapses and the siblings reclaim the space. The
/// tiling is session state, so this client requests the change rather than making it: that is
/// what lets the float survive a detach (see [`crate::split`]).
#[must_use]
fn push_float(i: usize, position: Option<(i32, i32)>) -> bool {
    if float_would_empty_the_dock(i) {
        return false; // tmux semantics: the main window keeps its last docked pane
    }
    // Ask the host, then let the adopt OPEN the window ([`reconcile_float_windows`] is the
    // one window creator), then place it where the gesture wants it. This must NOT push a
    // window itself: the adopt writes the same signal, so a caller holding a snapshot across
    // this call would clobber it — and a float another client made in the meantime would be
    // dropped, leaving that pane with no leaf and no window, i.e. invisible. That is the bug
    // R150/B4 fixed on the reattach path; it must not come back on the float path.
    set_pane_floating(i, true);
    if let Some(pos) = position {
        place_float(i, pos);
    }
    true
}

/// Put pane `i`'s floating window at `pos` — where the CLIENT decides a float goes (pixels
/// are ours; the host stores none). Separate from opening it, so there is one creator and
/// one placer rather than two writers racing on the windows signal. A no-op if the pane has
/// no window, or is already there (no `set`, no repaint).
fn place_float(i: usize, pos: (i32, i32)) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    if let Some(spec) = windows.iter_mut().find(|w| w.id == target) {
        if spec.position == Some(pos) {
            return;
        }
        spec.position = Some(pos);
        signal.set(windows);
    }
}

/// Project the host's FLOAT set onto this client's OS windows: open a window for every pane
/// the host floats but we do not show, and drop any whose pane the host no longer floats.
///
/// This is what makes a REATTACH whole. The host's tree carries only TILED panes, so a
/// floated pane has no leaf; without this a freshly-attached client would render neither a
/// leaf nor a window for it and the pane would simply VANISH — alive in the session,
/// invisible to its user. It also keeps two attached clients in step: a float over there
/// opens a window over here.
///
/// Position is deliberately not restored — the host stores none, because where a window sits
/// on screen is pixels and belongs to whoever is drawing (see [`crate::split`]'s seam). A
/// restored float is therefore WM-placed, and a client on a different screen is not asked to
/// honour coordinates from someone else's.
///
/// `slot_of` maps a host pane to this client's display slot; a pane it cannot place yet (not
/// admitted) is skipped and picked up once it is — the same "renderable now" contract the
/// projection holds.
pub(crate) fn reconcile_float_windows(
    floating: &[sprag_terminal::PaneId],
    slot_of: &impl Fn(sprag_terminal::PaneId) -> Option<usize>,
) {
    let want: Vec<usize> = floating.iter().filter_map(|p| slot_of(*p)).collect();
    let signal = use_windows_topology();
    let windows = signal.get();
    let shown: Vec<usize> = windows
        .iter()
        .filter_map(|w| pane_window_index(&w.id))
        .collect();
    if shown.iter().all(|i| want.contains(i)) && want.iter().all(|i| shown.contains(i)) {
        return; // already showing exactly the host's floats — never `set` an unchanged value
    }
    let mut next: Vec<WindowSpec> = windows
        .into_iter()
        // Drop a window whose pane the host no longer floats (another client docked it back).
        .filter(|w| pane_window_index(&w.id).is_none_or(|i| want.contains(&i)))
        .collect();
    // Open one for each float we are not showing. WM-placed: this client was not the one
    // that chose where it went.
    for &i in &want {
        if !next.iter().any(|w| w.id == pane_window_id(i)) {
            next.push(undock_window_spec(i, None));
        }
    }
    tracing::debug!(target: "sprag_gui::dock", ?want, ?shown, "projecting the host's float set");
    signal.set(next);
}

/// Ask the host to take pane `i` out of the tiling (or put it back) and adopt the resulting
/// arrangement — the ONE place this client changes float state, shared by [`push_float`] and
/// [`redock_pane`].
///
/// Adopting the answer (rather than editing the tree here) is what keeps this client a
/// projection: the host decides where the leaf goes, so there is exactly one builder. A hole
/// (no pane at the slot) has nothing to float and is a no-op.
fn set_pane_floating(i: usize, floating: bool) {
    let terminal = use_terminal();
    if let Some(snapshot) = terminal.slots.set_floating(i, floating) {
        crate::split::adopt_layout(&snapshot, &terminal.slots);
    }
}

/// Open pane `i` as a floating window at `position` (`None` → WM-placed; the key path
/// has no cursor). The discrete, cursor-less float: one topology `get`/`set` + the
/// `dock` diag, over [`push_float`]. Precondition: pane `i` is docked
/// ([`toggle_pane_floating`] gates on [`is_pane_floating`]), so a fresh open always
/// grows the window list by one. Single-responsibility — create only; the live-follow
/// reposition is [`float_pane_at`]'s, never here.
fn open_floating(i: usize, position: Option<(i32, i32)>) {
    let before = use_windows_topology().get().len();
    if !push_float(i, position) {
        return; // refused (last docked pane) — nothing changed, no diag
    }
    let after = use_windows_topology().get().len();
    crate::diag::dock_toggle(i, true, before, after);
}

/// Dock pane `i` back: drop its floating window. Idempotent no-op when pane `i` is
/// already docked (R1094 emits a redock/restore for a snap-back too, so a pane this
/// gesture never floated is harmless). Shared by [`toggle_pane_floating`]'s dock-back
/// branch and the live redock/restore (pinion R1094 / PINION-PR31).
///
/// It also asks the HOST to put the pane back into the tiling, and adopts the answer. With
/// no gesture to place it, the host returns it at the arrangement's END. A cross-window ZONE
/// redock is different: the reducer arm redocks FIRST and then relocates the now-present leaf
/// to the resolved zone, and [`crate::split::sync_layout`] writes THAT back — so a dropped
/// pane lands where it was dropped. (pinion R1173 also made `dock_panel_at_resolved_zone`
/// total over an absent leaf, so the ordering is belt-and-braces rather than load-bearing.)
pub(crate) fn redock_pane(i: usize) {
    let signal = use_windows_topology();
    let mut windows = signal.get();
    let target = pane_window_id(i);
    let Some(idx) = windows.iter().position(|w| w.id == target) else {
        return; // already docked
    };
    let before = windows.len();
    windows.remove(idx);
    set_pane_floating(i, false);
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
        signal.set(windows);
    } else {
        // First escaped move: float at the cursor. [`push_float`] carries the last-pane
        // invariant — if it refuses (this is the sole docked pane, tmux semantics), the
        // tear-off does nothing and the pane stays put. It also OPENS and PLACES the window
        // itself (through the host), so the `windows` snapshot read above is stale from here
        // on and must not be written back.
        let before = use_windows_topology().get().len();
        if !push_float(i, Some(pos)) {
            return;
        }
        crate::diag::dock_toggle(i, true, before, use_windows_topology().get().len());
    }
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

/// A host pane VANISHED (Round 2b live delta): drop its floating `pane-{i}` window if it
/// has one, so no OS window outlives the pane. The dock LEAF needs no action here — the pane
/// left the host's pool, so the host's own reconcile collapses its leaf and the next
/// [`crate::split::sync_layout`] projects the result. Called
/// from [`TerminalViewer::reconcile_frame`](crate::TerminalViewer) for each slot [`crate::slotview::SlotView::reconcile`]
/// freed. The freed slot's per-slot GUI
/// state (scroll / wheel / interaction / preedit) is reset by the caller; this owns only
/// the window + dock-tree cleanup. Removing the LAST docked leaf empties the dock to a blank
/// main — accepted even when a floater survives (a host close is authoritative; see
/// [`float_would_empty_the_dock`]'s SCOPE note — that guard is float-gesture-only, not a
/// global no-empty-main invariant). Runs in the root owner (the pre-view `reconcile_frame`
/// hook), so [`use_windows_topology`] / the split-tree hooks resolve; the window drop is a
/// plain `Signal::set` the shell applies at its next safe `reconcile_windows` (deferred via
/// `AppEvent::WindowsDirty`), never a synchronous winit op mid-render.
pub(crate) fn evict_pane(i: usize) {
    // Drop the floating window if the pane was undocked, else a `pane-{i}` window would
    // outlive its pane (painting an empty slot / dangling OS window).
    let signal = use_windows_topology();
    let mut windows = signal.get();
    if let Some(idx) = windows.iter().position(|w| w.id == pane_window_id(i)) {
        windows.remove(idx);
        signal.set(windows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project the REAL host's arrangement into the dock surface — what `reconcile_frame`
    /// does every frame before the view, and what these tests need before a float gesture
    /// (the last-docked-pane guard reads the projected tree, so an unprojected surface would
    /// report nothing tiled).
    fn project_host_layout() {
        crate::split::sync_layout(&use_terminal().slots);
    }

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
            project_host_layout();
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

    /// R87 (tmux semantics, the user's choice): the main window keeps at least one docked
    /// pane — floating the LAST docked pane is refused, so the dock never empties to a state
    /// with no drop target (an empty dock cannot be dragged back into).
    ///
    /// Drives the REAL host: a float now asks it to take the pane out of the tiling, and the
    /// refusal must happen before that ask (a refused float must leave the session's
    /// arrangement untouched, not un-float it afterwards).
    #[test]
    fn floating_the_last_docked_pane_is_refused() {
        let owner = Owner::new();
        owner.run(|| {
            project_host_layout();
            let windows = use_windows_topology();
            let floating = |i: usize| windows.get().iter().any(|w| w.id == pane_window_id(i));

            // Float pane 0 (pane 1 still docked — allowed) ...
            toggle_pane_floating(0);
            assert!(
                floating(0),
                "pane 0 floats (pane 1 keeps the dock non-empty)"
            );
            assert_eq!(
                crate::split::docked_pane_indices(),
                vec![1],
                "the host took the floated pane out of the tiling",
            );
            // ... then floating pane 1, the LAST docked pane, is REFUSED.
            toggle_pane_floating(1);
            assert!(
                !floating(1),
                "the last docked pane stays put (tmux semantics: dock keeps >=1 pane)"
            );
            assert_eq!(
                crate::split::docked_pane_indices(),
                vec![1],
                "and the refused float never reached the host's arrangement",
            );

            // Dock pane 0 back: the host tiles it again.
            toggle_pane_floating(0);
            assert!(!floating(0) && !floating(1), "both docked again");
            assert_eq!(crate::split::docked_pane_indices().len(), 2);
        });
    }

    /// R90/R91: `pane_is_movable` computes the source-level tear-off lock — FALSE for the
    /// sole docked pane (its drag would empty the dock), TRUE otherwise. It is wired to
    /// `DockPanelExternal::with_movable` so pinion's `begin_drag` returns `None` for a
    /// locked pane (no drag → no chip/preview). This guard pins the PREDICATE (both docked
    /// → both movable; float one → the sole pane computes non-movable; re-dock → restored).
    /// NOTE: the LIVE source-level block is blocked on
    /// PINION-PR42 — `reconcile_externals` early-returns on an unchanged tag set (sprag's
    /// tags never change), discarding the rebuilt external, so the dynamic `movable=false`
    /// is not applied after boot (create-time-only). Behavior stays correct via the
    /// reducer/float gate; the misleading chip/preview needs the pinion fix.
    #[test]
    fn the_sole_docked_pane_computes_non_movable() {
        let owner = Owner::new();
        owner.run(|| {
            project_host_layout();
            // Both docked → both movable (either may float, leaving the other).
            assert!(
                pane_is_movable(0) && pane_is_movable(1),
                "with 2 docked panes, both dock headers can start a drag"
            );
            // Float pane 0 → pane 1 is now the SOLE docked pane → NOT movable.
            toggle_pane_floating(0);
            assert!(
                !pane_is_movable(1),
                "the sole docked pane COMPUTES non-movable (the intended source-block; live \
                 no chip/preview is PR-42-gated, see the fn doc)"
            );
            assert!(
                pane_is_movable(0),
                "a floating pane stays movable (its window drag, not the dock header)"
            );
            // Re-dock → both movable again (dynamic with the live docked set).
            toggle_pane_floating(0);
            assert!(
                pane_is_movable(0) && pane_is_movable(1),
                "re-docking restores movability for both"
            );
        });
    }

    /// R122/R149: when a host pane is CLOSED, `evict_pane` drops its floating window, so no
    /// OS window outlives its pane.
    ///
    /// It deliberately does NOT touch the dock leaf: the pane left the host's pool, so the
    /// host's own reconcile collapses the leaf and the projection follows. (Before R149 this
    /// removed the leaf itself — one of the two builders that could disagree.)
    #[test]
    fn evict_pane_drops_a_closed_panes_floating_window() {
        let owner = Owner::new();
        owner.run(|| {
            project_host_layout();
            open_floating(0, None); // pane 0 floats -> a pane-0 window
            assert!(is_pane_floating(0), "pane 0 floated");
            assert_eq!(
                crate::split::docked_pane_indices(),
                vec![1],
                "the host stopped tiling the floated pane"
            );

            evict_pane(0); // the host closed pane 0

            assert!(!is_pane_floating(0), "evict drops the floating window");
        });
    }

    /// REATTACH, the arc's payoff: a client that joins a session where a pane is already
    /// floated must OPEN that pane's window.
    ///
    /// This is the bug B4 found live. The host's tree carries only TILED panes, so a floated
    /// pane has no leaf; a fresh client rendered no leaf (right) and no window (wrong), and
    /// the pane VANISHED — alive in the session, invisible to its user. Projecting the host's
    /// float set is what puts it back.
    #[test]
    fn a_reattaching_client_restores_the_hosts_floats() {
        let owner = Owner::new();
        owner.run(|| {
            project_host_layout();
            let ids: Vec<_> = (0..2)
                .map(|i| use_terminal().slots.pane_at(i).unwrap())
                .collect();
            let shown = || {
                use_windows_topology()
                    .get()
                    .iter()
                    .filter_map(|w| pane_window_index(&w.id))
                    .collect::<Vec<_>>()
            };
            assert!(shown().is_empty(), "a fresh client floats nothing");

            // The session says pane 0 is floated (another client left it that way).
            reconcile_float_windows(&[ids[0]], &|p| use_terminal().slots.slot_of(p));
            assert_eq!(shown(), vec![0], "the reattaching client re-floats pane 0");

            // Idempotent: projecting the same set again must not churn the windows signal.
            let before = use_windows_topology().get().len();
            reconcile_float_windows(&[ids[0]], &|p| use_terminal().slots.slot_of(p));
            assert_eq!(use_windows_topology().get().len(), before);

            // The session says it was docked back (another client's gesture): drop the window.
            reconcile_float_windows(&[], &|p| use_terminal().slots.slot_of(p));
            assert!(
                shown().is_empty(),
                "a pane the host stopped floating loses its window"
            );
        });
    }

    /// Evicting a plain DOCKED pane (no floating window) is a clean no-op on the windows
    /// signal — the window-absent branch.
    #[test]
    fn evict_pane_of_a_docked_pane_touches_no_window() {
        let owner = Owner::new();
        owner.run(|| {
            project_host_layout();
            let before = use_windows_topology().get().len();
            assert!(!is_pane_floating(1), "pane 1 is docked, no window");
            evict_pane(1);
            assert_eq!(use_windows_topology().get().len(), before);
        });
    }
}
