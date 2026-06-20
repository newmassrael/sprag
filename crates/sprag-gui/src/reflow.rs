//! The per-pane resize -> PTY reflow [`Effect`]s: keep each tiled pane's
//! `(cols, rows)` live as the window resizes (or a future splitter drags) by
//! consuming pinion's R1012 per-pane viewport seam
//! ([`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)). See the
//! crate-root "Winsize" module docs.

use crate::terminal::{TerminalView, grid_dims, pane_tag, use_terminal};
use pinion_core::CellMetric;
use pinion_core::reactive::{Effect, Owner};
use std::rc::Rc;

/// `Owner::cache` key for pane `index`'s reflow [`Effect`] (kept alive across
/// frames by the cache — a dropped [`Effect`] handle stops firing).
fn reflow_key(index: usize) -> String {
    format!("sprag_gui.reflow.{index}")
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
/// **after** layout, so each Effect reads the [`use_pane_viewport_size`] hook
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
    for index in 0..terminal.pane_count() {
        install_pane_reflow(&owner, &terminal, index);
    }
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
    let pane_id = terminal.pane(index).id();
    let tag = pane_tag(index);
    let effect = Effect::new(owner, move || {
        // R1012 tracked read: re-fires on every change to THIS pane's measured
        // rect (window resize re-division / splitter drag).
        let measured = pinion_core::use_pane_viewport_size(tag);
        let Some(target) = reflow_target(measured, terminal.metric) else {
            return; // pane unmeasured (boot, before layout) — no reflow
        };
        // Reflow only on a real change, so an unchanged frame issues no ioctl.
        if terminal.pane(index).session().dimensions() != target {
            let _ = terminal.workspace.resize(pane_id, target.0, target.1);
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
        for i in 0..terminal.pane_count() {
            assert_eq!(
                terminal.pane(i).session().dimensions(),
                grid_dims((WINDOW_W, WINDOW_H), metric),
                "pane {i} keeps its boot dims (the (0,0) eager run skipped)",
            );
        }
    }
}
