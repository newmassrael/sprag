//! The per-pane resize -> PTY reflow [`Effect`]s: keep each tiled pane's
//! `(cols, rows)` live as the window resizes (or a future splitter drags) by
//! consuming pinion's R1012 per-pane viewport seam
//! ([`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)). See the
//! crate-root "Winsize" module docs.

use crate::terminal::{TerminalView, grid_dims, pane_cache_key, pane_tag, use_terminal};
use pinion_core::CellMetric;
use pinion_core::reactive::{Effect, Owner};
use sprag_terminal::fit_window;
use std::rc::Rc;

/// `Owner::cache` key for pane `index`'s reflow [`Effect`] (kept alive across
/// frames by the cache — a dropped [`Effect`] handle stops firing). Minted via the
/// one per-pane key site [`pane_cache_key`] so the index suffix cannot drift.
fn reflow_key(index: usize) -> String {
    pane_cache_key("reflow", index)
}

/// Holds one pane's resize -> PTY reflow [`Effect`] so the `Owner::cache` keeps it
/// alive across frames (a dropped [`Effect`] handle drops its subscription and
/// stops firing). It carries no data — only the live subscription.
pub(crate) struct ReflowMarker {
    _effect: Effect,
}

/// The `(cols, rows)` a pane should reflow to for a measured pixel rect, or
/// `None` when the rect is the `(0, 0)` "pane unmeasured" sentinel (pinion R1012,
/// before the first layout) or otherwise zero-area — a reflow then would size the
/// PTY to a spurious `1 x 1`. A measured rect derives via [`grid_dims`] (the §3
/// winsize SSOT, floored at `1 x 1`), identical to the boot derivation, so the
/// reflowed size and the painted grid agree. Pure, so it is unit-testable without
/// a shell.
fn reflow_target(measured: (u32, u32), metric: CellMetric) -> Option<(u16, u16)> {
    let (w, h) = measured;
    if w == 0 || h == 0 {
        return None;
    }
    Some(grid_dims((w, h), metric))
}

/// Install (once) the resize -> PTY reflow [`Effect`] for **every** tiled pane:
/// each subscribes to its own pinion R1012 per-pane viewport
/// [`Signal`](pinion_core::reactive::Signal) via
/// [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)`(pane_tag(i))`
/// and, when that pane's measured sub-rect changes (an OS window resize re-divides
/// the tiles; a future splitter drag moves one boundary), re-derives
/// `(cols, rows)` from the new rect ([`reflow_target`] -> [`grid_dims`]) and
/// reflows that pane (`TIOCSWINSZ` via
/// [`Workspace::resize`](sprag_terminal::Workspace::resize)). It skips the
/// `(0, 0)` "unmeasured" rect (so it never reflows to `1 x 1` before the first
/// layout) and any no-change frame (so a same-`(cols, rows)` resize issues no
/// ioctl).
///
/// Unlike the single-pane R1006 window seam, the per-pane rect is only known
/// **after** layout, so each Effect reads the
/// [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) hook
/// (which resolves [`Owner::current`] on every re-run) rather than a captured
/// signal handle; the shell's R1012 publish `set`s inside its `root_owner` scope
/// (the load-bearing contract), so the synchronous re-run resolves the owner. The
/// terminal is `Owner::cache`-backed, so it is resolved BEFORE the per-pane cache
/// factories (the nested-factory guard) and the Effect captures the resolved
/// handle. Registered from
/// [`WidgetCore::create_extra_externals`](pinion_core::WidgetCore::create_extra_externals)
/// so it is live before the first paint and off the pure `view`. The eager run
/// reads each tag's `(0, 0)` boot value (registering the tag) and skips, so the
/// boot dims stand until the first layout publishes the real rects.
pub(crate) fn install_reflow() {
    let owner = Owner::current().expect("install_reflow() requires an active Owner scope");
    // Resolve the cache-backed terminal BEFORE the per-pane cache factories
    // (nested-factory guard). The Effect bodies read use_pane_viewport_size
    // (the registry, another cache slot), but only at RUN time — the factory
    // itself stays cache-free.
    let terminal = use_terminal();
    install_report(&owner, &terminal);
    for index in terminal.slots.occupied_slots() {
        install_pane_reflow(&owner, &terminal, index);
    }
}

/// `Owner::cache` key for the client-size report [`Effect`] — one per client, not one per pane,
/// because what it reports is this CLIENT's area.
const REPORT_KEY: &str = "sprag_gui.client_size_report";

/// Holds the client-size report [`Effect`] so the `Owner::cache` keeps it alive across frames.
pub(crate) struct ReportMarker {
    _effect: Effect,
}

/// The cells this client can give the session's ARRANGEMENT, or `None` while it cannot say.
///
/// Not the window, and not the dock surface either. sprag's chrome is PER PANE — every dock panel
/// carries a header, and a scrollbar beside its grid — so the cells available to the arrangement
/// depend on the shape of the tiling rather than on the size of the window: two stacked panes lose
/// two headers where two side-by-side ones lose one. Reporting the surface would hand the session a
/// window larger than this client can draw, and every pane's grid would be sized past the widget
/// holding it.
///
/// So it reports what it MEASURED. pinion publishes each pane's laid-out rect (R1012), which is the
/// grid area with the header, the scrollbar, the divider and the rounding already taken out by
/// pinion's own layout — this client therefore models none of its chrome. Those measurements fold
/// back into one window through [`fit_window`], the inverse of the tiler that will consume it.
///
/// `None` when the host shows nothing, or while any DISPLAYED pane is still at pinion's `(0, 0)`
/// pre-layout sentinel: a report is a CLAIM about what this client can show, and a client that has
/// not been laid out yet has none to make. Reporting only the panes it happens to know would name a
/// window missing a pane's worth of cells.
///
/// The walk is over the PROJECTION rather than over the arrangement, both times it is used. Under a
/// zoom this client draws ONE pane, so the panes the zoom hides have no surface to measure — asking
/// for their rects would collect the pre-layout sentinel for each and report nothing at all, which
/// would silently freeze the window at whatever it was before the zoom.
fn reported_window(terminal: &TerminalView) -> Option<(u16, u16)> {
    let layout = terminal.slots.layout();
    let projection = layout.projection();
    // ⚠⚠⚠⚠⚠ **WITH NO PANES TO FOLD, THIS CLIENT ANSWERS FROM ITS OWN SURFACE** — register item
    // 462, and without it the client and the daemon wait for each other for ever.
    //
    // The daemon derives a layout FROM THE ATTACHED CLIENT'S SIZE, so a client that has not
    // reported one is sent `panes: []` — and the fold below has nothing to measure, so it reports
    // nothing, so the daemon still has no size. **Neither side can move first.** Measured on the
    // wire: a first attach sits at `size: null`, `layout.tree = {nodes: [], root: null}`, and the
    // window paints **87.6% white** while the very same introspect lists the session, the window
    // and the pane. Forcing the client to attach AGAIN registers `{cols: 161, rows: 33}` and the
    // terminal appears — so the content was never missing, only the rectangle to draw it in.
    //
    // ⚠⚠⚠⚠ **AND IT IS THE CLIENT'S TIE TO BREAK, NOT THE DAEMON'S.** *What can this client show*
    // is a fact about its own surface; the fold below is a REFINEMENT of that, not its only source.
    // A daemon inventing a size for a client that has not spoken would be a number nobody chose —
    // and it would be wrong for every headless reader of the same wire.
    //
    // ⚠⚠ IT IS AN APPROXIMATION AND SAYS SO: the whole viewport still carries the chrome that
    // pinion's layout takes out of a pane's rect, so this over-reports by the header and the
    // scrollbar. That is a bootstrap claim, corrected on the very next run of this Effect — which
    // re-fires the moment a pane rect exists to fold. ⚠ And `use_viewport_size` is a TRACKED read,
    // so an OS window resize now re-fires this too; measured before the fix, resizing the window
    // did NOT lift the deadlock, because nothing this Effect read had changed.
    //
    // ⚠ `(0, 0)` — a shell that has not sized the window yet — still answers `None` through
    // `reflow_target`, which is the honest *I cannot say* rather than a fabricated 1x1.
    //
    // ⚠⚠⚠⚠⚠ **READ UNCONDITIONALLY, BEFORE THE FOLD, BECAUSE THE SUBSCRIPTION IS HALF THE FIX.**
    // A tracked read is what re-runs this Effect, and in the deadlock every pane rect is stuck at
    // the sentinel — so nothing this Effect read could ever change and it ran ONCE, at boot, for
    // ever. Measured with a probe inside the client: two runs total, and the second happened only
    // because the probe itself had read the viewport. Reading it here means the first real window
    // size re-fires this, which is exactly when the client first has something to say.
    let surface = pinion_core::use_viewport_size();
    let mut measured = Vec::new();
    let mut unmeasured = false;
    for pane in projection.panes() {
        let slot = terminal.slots.slot_of(pane)?;
        // A TRACKED read, and the reason this Effect re-runs: any pane's rect moving can change
        // what this client can give the arrangement.
        let rect = pinion_core::use_pane_viewport_size(pane_tag(slot));
        match reflow_target(rect, terminal.metric) {
            Some(cells) => measured.push((pane, cells)),
            None => unmeasured = true,
        }
    }
    // ⚠⚠⚠⚠⚠ NOT ONE PANE COULD BE MEASURED — the deadlock, and the only place this function may
    // answer from the surface. Measured live: the projection holds a pane, its rect is the
    // pre-layout sentinel, and it will never leave it, because the daemon lays out nothing until
    // this client reports and this client reports nothing until it is laid out.
    //
    // ⚠⚠⚠ THE OVER-REPORT THIS COSTS IS REAL AND IS THE LESSER FAULT. The doc above rejects the
    // surface for a good reason — sprag's chrome is PER PANE, so a window folded from the surface
    // is larger than this client can draw. That reasoning holds for a laid-out client and says
    // nothing about one that cannot be laid out at all: the choice here is not *accurate or
    // approximate*, it is *approximate for one frame or blank for ever*. The next run folds from
    // real rects and replaces it, and the host ignores a repeat of numbers it already has.
    if measured.is_empty() {
        return reflow_target(surface, terminal.metric);
    }
    // ⚠ A PARTIALLY LAID-OUT CLIENT STILL SAYS NOTHING, which is this function's own older rule:
    // folding only the panes it happens to know would name a window missing a pane's worth of
    // cells. That rule is kept whole — it is only the ALL-unmeasured case that changed.
    if unmeasured {
        return None;
    }
    fit_window(&projection, &measured)
}

/// Install (once) the [`Effect`] that keeps the host told how big this client is.
///
/// One Effect for the client rather than one per pane: the answer is folded from EVERY tiled pane's
/// rect, so a per-pane Effect would compute the same number once per pane and report it as many
/// times. It re-runs whenever any of those rects moves, which is exactly when the answer can move —
/// an OS window resize, a splitter drag settling, a pane arriving or leaving.
///
/// The host ignores a repeat of the same numbers, so a run that folds to what was already reported
/// costs one call and wakes nobody.
fn install_report(owner: &Owner, terminal: &Rc<TerminalView>) {
    if owner.cache_contains::<ReportMarker>(REPORT_KEY.to_owned()) {
        return;
    }
    let terminal = Rc::clone(terminal);
    let effect = Effect::new(owner, move || {
        if let Some((cols, rows)) = reported_window(&terminal) {
            terminal.slots.report_client_size(cols, rows);
        }
    });
    owner.cache(REPORT_KEY.to_owned(), move || ReportMarker {
        _effect: effect,
    });
}

/// Whether the pane in slot `index` is one the SESSION sizes, so this client must not.
///
/// True for a TILED pane of a session with an arbitrated window: its `(cols, rows)` is
/// `tile(tree, window)`, both of which are the daemon's, and the daemon derives it. A client
/// writing its own answer there is the second authority this front exists to remove — it is how a
/// GUI came to size a pane from its pixels while a terminal client sized the same pane from the
/// window, leaving whichever wrote last in force and the other painting a grid it had the wrong
/// dimensions for.
///
/// False in the two cases where the session says nothing, and then this client's own pixels are the
/// only fact available — the same fallback `sprag-tui` states for its own screen:
///
/// * A FLOATING pane. It has no leaf in the arrangement, so no tiling ever names it; its window is
///   its own surface and its size is that surface's business.
/// * A host with NO arbitrated window: an in-process host (one surface, so there is nothing to
///   arbitrate) or a daemon no client has reported an area to yet.
///
/// It asks the ARRANGEMENT rather than the projection, and that is deliberate: a pane a zoom is
/// hiding is still the session's to size. The daemon's tiling does not name it, so it simply keeps
/// the size it had — exactly as a pane the window is too small to show does — and a client that
/// took the gap as licence to size it would be the second authority again, contradicted the moment
/// the zoom ended. (It never reaches this in practice: a hidden pane has no surface, so its rect is
/// the pre-layout sentinel and the Effect returns before asking. The answer is right anyway,
/// because relying on the caller to not-ask would be relying on a coincidence.)
fn owns_its_own_size(terminal: &TerminalView, index: usize) -> bool {
    if terminal.slots.window_size().is_none() {
        return false;
    }
    terminal
        .slots
        .pane_at(index)
        .is_some_and(|pane| terminal.slots.layout().tree.panes().contains(&pane))
}

/// Install pane `index`'s reflow [`Effect`] exactly once (idempotent via the
/// `cache_contains` guard — created at view-or-boot scope, NOT in a cache factory,
/// whose eager run would re-enter `Owner::cache` and panic).
fn install_pane_reflow(owner: &Owner, terminal: &Rc<TerminalView>, index: usize) {
    let key = reflow_key(index);
    if owner.cache_contains::<ReflowMarker>(key.clone()) {
        return;
    }
    let terminal = Rc::clone(terminal);
    let tag = pane_tag(index);
    let effect = Effect::new(owner, move || {
        // R1012 tracked read: re-fires on every change to THIS pane's measured
        // rect (window resize re-division / splitter drag).
        let measured = pinion_core::use_pane_viewport_size(tag);
        let Some(target) = reflow_target(measured, terminal.metric) else {
            return; // pane unmeasured (boot, before layout) — no reflow
        };
        if owns_its_own_size(&terminal, index) {
            return;
        }
        // Reflow only on a real change, so an unchanged frame issues no ioctl.
        // Both the PTY-size read and the resize go through the host client.
        if terminal.slots.pane_grid_size(index) != target {
            // Carry the display's cell pixel geometry (the reflow metric) so the daemon's PTY
            // winsize gets real `ws_xpixel` / `ws_ypixel` and XTWINOPS pixel reports answer
            // truthfully. `u32 -> u16` saturates (a cell never spans near 65535 px).
            let cell_px = (
                u16::try_from(terminal.metric.cell_w()).unwrap_or(u16::MAX),
                u16::try_from(terminal.metric.cell_h()).unwrap_or(u16::MAX),
            );
            terminal.slots.resize(index, target.0, target.1, cell_px);
        }
    });
    owner.cache(key, move || ReflowMarker { _effect: effect });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WINDOW_H, WINDOW_W};
    use pinion_core::CellMetric;

    #[test]
    fn reflow_target_skips_unmeasured_and_derives_dims() {
        let metric = CellMetric::DEFAULT;
        // The (0, 0) "unmeasured" sentinel (and any zero axis) -> no reflow.
        assert_eq!(reflow_target((0, 0), metric), None);
        assert_eq!(reflow_target((0, 200), metric), None);
        assert_eq!(reflow_target((400, 0), metric), None);
        // A measured rect derives through the same SSOT the boot spawn uses.
        assert_eq!(
            reflow_target((400, 200), metric),
            Some(grid_dims((400, 200), metric))
        );
    }

    /// Installing the per-pane reflow Effects over the real boot panes does not
    /// panic, and with no shell publishing rects every pane reads its `(0, 0)`
    /// "unmeasured" value, so each eager run skips and the panes keep their boot
    /// dims (no spurious `1 x 1` reflow). The measured-rect -> reflow leg is proven
    /// end-to-end by pinion's `pane_viewport_seam.rs` (literally the sprag two-pane
    /// model) and the live-window smoke — there is no public per-pane-rect setter
    /// to drive that leg headlessly here (the registry is shell-internal).
    #[test]
    fn install_reflow_registers_panes_and_skips_unmeasured() {
        let owner = Owner::new();
        let metric = CellMetric::DEFAULT;
        // No shell seeded: use_repaint_sink -> NullRepaintSink and
        // measured_monospace_cell -> None -> CellMetric::DEFAULT (what
        // use_terminal measures here). Boot the panes + install the Effects; the
        // eager run sees (0,0) and skips. Spawns real shell PTYs; dropping `owner`
        // at end of test reaps them.
        owner.run(install_reflow);
        let terminal = owner.run(use_terminal);
        for i in terminal.slots.occupied_slots() {
            assert_eq!(
                terminal.slots.pane_grid_size(i),
                grid_dims((WINDOW_W, WINDOW_H), metric),
                "pane {i} keeps its boot dims (the (0,0) eager run skipped)",
            );
        }
    }
}
