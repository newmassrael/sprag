//! The resize -> PTY reflow [`Effect`]: keep the pane's `(cols, rows)` live as
//! the window resizes by consuming pinion's R1006 viewport-size seam. See the
//! crate-root "Winsize" module docs.

use crate::terminal::{grid_dims, use_terminal};
use pinion_core::reactive::{Effect, Owner};
use std::rc::Rc;

/// `Owner::cache` key for the resize -> reflow [`Effect`] (kept alive across
/// frames by the cache — a dropped [`Effect`] handle stops firing).
const REFLOW_KEY: &str = "sprag_gui.reflow";

/// Holds the resize -> PTY reflow [`Effect`] so the `Owner::cache` keeps it
/// alive across frames (a dropped [`Effect`] handle drops its subscription and
/// stops firing). It carries no data — only the live subscription.
pub(crate) struct ReflowMarker {
    _effect: Effect,
}

/// Install (once) the resize -> PTY reflow [`Effect`]: it subscribes to the
/// pinion R1006 viewport-size [`Signal`](pinion_core::reactive::Signal) and, on
/// every OS window resize, re-derives `(cols, rows)` from the new viewport and
/// the measured cell ([`grid_dims`]) and reflows the boot pane (`TIOCSWINSZ`
/// via [`Workspace::resize`](sprag_terminal::Workspace::resize)). It skips the
/// `(0, 0)` "viewport unknown" boot value (so it never reflows to `1x1` before
/// the window is sized) and any no-change frame (so a same-`(cols, rows)`
/// resize issues no ioctl).
///
/// The viewport `Signal` and the terminal are BOTH `Owner::cache`-backed, so
/// they are resolved *before* this cache factory (the nested-factory guard):
/// the Effect closure captures the resolved handles and only calls
/// [`Signal::get`](pinion_core::reactive::Signal::get) (tracked) in its run —
/// never a `use_*` hook, which would re-enter `Owner::cache` and panic. Reading
/// the captured `Signal` directly (not `use_viewport_size`) also means the
/// Effect never resolves `Owner::current`, so the synchronous `set`-driven
/// re-run is robust regardless of scope (the shell still `set`s inside its
/// `root_owner`, per the R1006 contract). Registered from
/// [`WidgetCore::create_extra_externals`](pinion_core::WidgetCore::create_extra_externals)
/// so it is live before the first paint and off the pure `view`.
pub(crate) fn install_reflow() -> Rc<ReflowMarker> {
    let owner = Owner::current().expect("install_reflow() requires an active Owner scope");
    // Resolve both cache-backed deps BEFORE the factory (nested-factory guard).
    let terminal = use_terminal();
    let viewport = owner.viewport_size_signal();
    let owner_for_effect = owner.clone();
    owner.cache(REFLOW_KEY, move || {
        let terminal = Rc::clone(&terminal);
        let viewport = viewport.clone();
        let effect = Effect::new(&owner_for_effect, move || {
            let size = viewport.get(); // tracked read: re-fires on every resize
            if size == (0, 0) {
                return; // "viewport unknown" (boot, before resume) — no reflow
            }
            let target = grid_dims(size, terminal.metric);
            // Reflow the boot pane only when the derived size actually changed,
            // so an unchanged frame issues no ioctl.
            let pane = terminal.boot_pane();
            let pane_to_reflow = (pane.session().dimensions() != target).then(|| pane.id());
            if let Some(id) = pane_to_reflow {
                let _ = terminal.workspace.resize(id, target.0, target.1);
            }
        });
        ReflowMarker { _effect: effect }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WINDOW_H, WINDOW_W};
    use pinion_core::reactive::Signal;
    use pinion_core::CellMetric;

    /// End-to-end reflow: install the real reflow [`Effect`] over a real PTY,
    /// drive pinion's viewport [`Signal`](pinion_core::reactive::Signal), and
    /// assert the pane reflows. Exercises the whole R1006 consumer chain
    /// headlessly — the (0,0) boot skip, the resize on change, the emulator-SSOT
    /// dimensions, and the equality-skip no-op — without a window.
    #[test]
    fn reflow_resizes_the_boot_pane_when_the_viewport_changes() {
        // No shell seeded: use_repaint_sink -> NullRepaintSink and
        // measured_monospace_cell -> None -> CellMetric::DEFAULT (what
        // use_terminal measures here). Seed a real viewport Signal so the reflow
        // Effect install_reflow registers subscribes to it.
        let owner = Owner::new();
        let metric = CellMetric::DEFAULT;
        let viewport = Signal::new((0_u32, 0_u32));
        owner.provide_viewport_size_signal(viewport.clone());
        // Boot the pane + install the reflow Effect. The eager run sees (0,0)
        // and skips, so the boot dims (window / cell) stand. Spawns a real
        // long-lived shell PTY; dropping `owner` at end of test reaps it.
        let _reflow = owner.run(install_reflow);
        let terminal = owner.run(use_terminal);
        let dims = || {
            terminal
                .workspace
                .panes()
                .first()
                .expect("boot pane present")
                .session()
                .dimensions()
        };
        let boot = dims();
        assert_eq!(
            boot,
            grid_dims((WINDOW_W, WINDOW_H), metric),
            "boot dims derive from the window (the (0,0) eager run skipped)",
        );
        // Publish a new viewport inside the owner scope (mirrors the shell's
        // R1006 set inside root_owner): the synchronous Effect re-run reflows.
        owner.run(|| viewport.set((400, 200)));
        let after = dims();
        assert_eq!(after, grid_dims((400, 200), metric), "pane reflowed to the new viewport");
        assert_ne!(after, boot, "the reflow actually changed the dims");
        // A repaint at the same viewport is inert (Signal equality-skip).
        owner.run(|| viewport.set((400, 200)));
        assert_eq!(dims(), after, "a same-size frame is a no-op");
    }
}
