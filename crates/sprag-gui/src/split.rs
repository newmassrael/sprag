//! The dock split-tree layout (R60): the docked panes are arranged by a pinion
//! [`DockTopology`] — an identity-keyed recursive binary split-tree — replacing
//! the former flat row/grid model (R38 row + R40 grid + position-keyed dividers).
//! This module owns the topology MODEL (the tree + its mutation on dock/undock and
//! the per-split ratio Signals); the view layer ([`crate::view`]) lowers it to
//! pixels via [`view_dock_surface`](pinion_widget_paint::dock::view_dock_surface),
//! and [`create_extra_externals`](crate::TerminalViewer) registers one drag
//! `SplitterExternal` per Split.
//!
//! ## Why a split-TREE (the v1 bound this retires)
//!
//! The old `split.rs` keyed dividers by docked POSITION (and held grid slots on
//! undock) because pinion's External registration was boot-only — a divider's
//! drag-axis was welded at boot, so the layout could not reshape at runtime
//! without the boot Externals driving the wrong divider. pinion R689
//! [`external_set_is_dynamic`](pinion_core::WidgetCore::external_set_is_dynamic)
//! lifts that constraint: the binding walks the LIVE topology each reconcile and a
//! runtime-minted Split auto-registers its `SplitterExternal`. So the canonical
//! shape the old docs deferred to — a real split-tree whose Split ids are STABLE
//! across mutations (a ratio follows its boundary, never reshuffles) — is now
//! reachable, and is the foundation drag-to-dock (P2) and tear-off (P3) build on.
//!
//! ## The topology tracks the DOCKED panes (collapse on undock)
//!
//! [`use_dock_topology`] holds `Signal<Option<DockTopology>>` — the tree of panes
//! currently DOCKED in the main window. Undocking a pane removes its leaf
//! ([`float_pane`]): its sibling sub-tree promotes into the parent Split's place,
//! so the remaining panes reclaim the freed space (the natural dock behaviour).
//! Docking back re-inserts the leaf ([`dock_pane`]), preserving left-to-right pane
//! INDEX order. `None` = no panes docked (every pane floated) — the main window
//! paints empty. The floating SSOT is still [`crate::dock`]'s window topology; the
//! one site that toggles a pane ([`toggle_pane_floating`](crate::dock::toggle_pane_floating))
//! mutates BOTH signals together so they never disagree.
//!
//! ## Identity-keyed ids
//!
//! Each leaf carries a stable [`panel_id`] (`terminal-{i}`, mapping 1:1 to the tile
//! index via [`pane_index_of_panel`]); each Split a stable id ([`boot_split_id`]
//! for the boot tree, a fresh `sprag_gui.split.dock.{seq}` for a dock-back insert).
//! The per-split ratio Signal ([`use_split_ratio`]) is `Owner::cache`-keyed by that
//! id and SHARED between the view (`view_dock_surface`) and the drag
//! `SplitterExternal`, so a drag re-weights the painted panes. A Split id is the
//! panel-id-distinct tag the [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel)
//! header / `SplitterStyle` carry, so the scene tags never collide with the
//! per-pane [`pane_tag`](crate::terminal::pane_tag) (`sprag_gui.pane.{i}`) the grid
//! inside each leaf carries.

use crate::terminal::{MAX_PANES, pane_count};
use pinion_core::reactive::{Owner, Signal};
use pinion_widget_paint::dock::{DockNode, DockSplitPosition, DockTopology, TopologyError};
use pinion_widget_paint::splitter::SplitterOrientation;
use std::borrow::Cow;
use std::rc::Rc;

/// The `panel_id` prefix — a leaf's stable id is `terminal-{i}` for tile index `i`.
/// A readable token (it is the [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel)
/// header title) that doubles as the panel's scene tag, kept DISTINCT from the
/// per-pane [`pane_tag`](crate::terminal::pane_tag) (`sprag_gui.pane.{i}`) the inner
/// grid carries (the dock panel wraps the grid, so the two tags must not collide)
/// and from the [`pane_window_id`](crate::dock::pane_window_id) (`pane-{i}`) window
/// namespace.
const PANEL_ID_PREFIX: &str = "terminal-";

/// The stable `panel_id` of the pane at tile `index` — the dock-tree leaf identity
/// + the panel's header title + scene tag. Inverse of [`pane_index_of_panel`].
pub(crate) fn panel_id(index: usize) -> String {
    format!("{PANEL_ID_PREFIX}{index}")
}

/// The tile index a `panel_id` addresses, or `None` if it is not a pane panel id
/// (validates `< `[`MAX_PANES`] so a malformed id can never index out of range) —
/// the inverse of [`panel_id`], so the view's `panel_content` callback maps a leaf
/// back to its pane.
pub(crate) fn pane_index_of_panel(panel_id: &str) -> Option<usize> {
    panel_id
        .strip_prefix(PANEL_ID_PREFIX)?
        .parse::<usize>()
        .ok()
        .filter(|&i| i < MAX_PANES)
}

/// The stable Split id of the boot tree's divider whose LEFT child is the leaf at
/// tile `k` (the right-nested row: divider `k` separates pane `k` from everything to
/// its right). Stable for the life of the boot tree; a dock-back insert mints a
/// fresh `sprag_gui.split.dock.{seq}` id instead ([`next_dock_split_id`]) so the two
/// id spaces never collide.
pub(crate) fn boot_split_id(k: usize) -> String {
    format!("sprag_gui.split.{k}")
}

/// The default divider ratio — even (left/top share `0.5`), matching the former
/// even tiling so the boot layout is unchanged.
const SPLIT_RATIO_DEFAULT: f32 = 0.5;

/// Split `id`'s ratio `Signal` (left/top share, `[0, 1]`), an `Owner::cache`-backed
/// `Rc<Signal<f32>>` SHARED between the read side (the view's `view_dock_surface`
/// `split_state` callback) and the write side (the `SplitterExternal` registered at
/// the same id) — both resolve the same root-owner slot keyed on the Split id, so a
/// drag (`set`) re-weights the painted panes. `initial` seeds the slot on first
/// resolution (the topology's declared ratio, threaded through by the walker), so
/// the topology stays the single source of the initial value.
pub(crate) fn use_split_ratio(id: impl Into<Cow<'static, str>>, initial: f32) -> Rc<Signal<f32>> {
    Owner::current()
        .expect("use_split_ratio() requires an active Owner scope")
        .cache(id, move || Signal::new(initial))
}

/// `Owner::cache` key for the held dock-tree topology Signal.
const TOPOLOGY_KEY: &str = "sprag_gui.dock_topology";

/// The dock-tree topology Signal — the layout of the DOCKED panes (`None` = none
/// docked). Cached in the root owner (the view fn, the splitter-External
/// registration, and the dock/undock toggle resolve the same shared slot), seeded
/// once with the boot tree ([`build_boot_topology`] over all [`pane_count`] panes,
/// all docked at boot). [`float_pane`] / [`dock_pane`] mutate it; the shell's
/// `view` subscribes it (a `set` repaints) and `reconcile_externals` re-walks it to
/// register a dock-back-minted Split's drag External (pinion R689).
pub(crate) fn use_dock_topology() -> Rc<Signal<Option<DockTopology>>> {
    Owner::current()
        .expect("use_dock_topology() requires an active Owner scope")
        .cache(TOPOLOGY_KEY, || {
            Signal::new(build_boot_topology(pane_count()))
        })
}

/// `Owner::cache` key for the dock-back Split-id sequence counter.
const SPLIT_SEQ_KEY: &str = "sprag_gui.dock_split_seq";

/// Mint a fresh, never-reused Split id for a dock-back insert
/// (`sprag_gui.split.dock.{seq}`). A monotonic `Owner::cache`-backed counter, so a
/// re-inserted divider gets a unique id (its ratio starts at the default — a
/// dock/undock cycle does not remember the old boundary's ratio, a deliberate v1
/// bound; P2 drag-to-dock gives precise control). Distinct from the boot ids
/// ([`boot_split_id`]) so the two spaces never collide.
fn next_dock_split_id() -> String {
    let seq = Owner::current()
        .expect("next_dock_split_id() requires an active Owner scope")
        .cache(SPLIT_SEQ_KEY, || Signal::new(0_u64));
    let n = seq.get();
    seq.set(n + 1);
    format!("sprag_gui.split.dock.{n}")
}

/// Build the boot dock tree over tiles `0..n`: a right-nested all-Horizontal split
/// (divider `k` separates pane `k` from everything to its right), so the boot layout
/// is byte-identical to the former even row tiling at any pane count. `n == 0` →
/// `None` (no docked panes); `n == 1` → a single leaf, no split. Pure.
fn build_boot_topology(n: usize) -> Option<DockTopology> {
    (n >= 1).then(|| DockTopology::new(build_row_node(0, n)))
}

/// Right-nested Horizontal sub-tree over tiles `start..end` (`end > start`): the leaf
/// at `start` beside the recursively-built remainder, divided by [`boot_split_id`]`(start)`.
fn build_row_node(start: usize, end: usize) -> DockNode {
    debug_assert!(end > start, "build_row_node needs a non-empty range");
    if end - start == 1 {
        DockNode::leaf(panel_id(start))
    } else {
        DockNode::split_horizontal(
            boot_split_id(start),
            SPLIT_RATIO_DEFAULT,
            DockNode::leaf(panel_id(start)),
            build_row_node(start + 1, end),
        )
    }
}

/// Undock pane `index`: remove its leaf from the dock tree so the remaining panes
/// reclaim the space (the sibling sub-tree promotes into the parent Split's place,
/// and that Split's ratio drops). Floating the LAST docked pane empties the tree
/// (`None`); floating a pane already absent is a no-op (defensive — the
/// [`toggle_pane_floating`](crate::dock::toggle_pane_floating) alternation never
/// asks for it). Runs in the root owner scope (the toggle is called there).
pub(crate) fn float_pane(index: usize) {
    let topo = use_dock_topology();
    let Some(current) = topo.get() else {
        return; // nothing docked
    };
    match current.remove_leaf(&panel_id(index)) {
        Ok(next) => topo.set(Some(next)),
        // RootRemoval = pane `index` was the sole docked leaf -> the main window
        // now has no docked panes.
        Err(TopologyError::RootRemoval) => topo.set(None),
        // PanelNotFound (already floated) — leave the tree unchanged.
        Err(_) => {}
    }
}

/// Dock pane `index` back: re-insert its leaf, preserving left-to-right pane INDEX
/// order — split the first docked pane whose index exceeds `index` (the new leaf
/// takes the `First`/left slot), or append after the last docked pane (the `Second`/
/// right slot) when `index` is the largest. An empty tree (`None`) becomes the single
/// leaf. The new divider gets a fresh id ([`next_dock_split_id`]) at the default
/// ratio. A topology error (the pane already present — defensive, the toggle never
/// asks for it) leaves the tree unchanged. Runs in the root owner scope.
pub(crate) fn dock_pane(index: usize) {
    let topo = use_dock_topology();
    let next = match topo.get() {
        None => DockTopology::single(panel_id(index)),
        Some(current) => {
            let docked: Vec<usize> = current
                .panel_ids()
                .iter()
                .filter_map(|p| pane_index_of_panel(p))
                .collect();
            let split_id = next_dock_split_id();
            let inserted = match docked.iter().find(|&&j| j > index) {
                // Insert before the first higher-indexed docked pane (keeps order).
                Some(&j) => current.split_leaf_into(
                    &panel_id(j),
                    panel_id(index),
                    split_id,
                    SplitterOrientation::Horizontal,
                    SPLIT_RATIO_DEFAULT,
                    DockSplitPosition::First,
                ),
                // `index` is the largest docked index -> append on the right. A
                // `Some` topology always has >= 1 docked leaf, so `last` resolves.
                None => current.split_leaf_into(
                    &panel_id(*docked.last().expect("a Some topology has >= 1 docked pane")),
                    panel_id(index),
                    split_id,
                    SplitterOrientation::Horizontal,
                    SPLIT_RATIO_DEFAULT,
                    DockSplitPosition::Second,
                ),
            };
            match inserted {
                Ok(t) => t,
                Err(_) => return, // defensive: leave the tree unchanged
            }
        }
    };
    topo.set(Some(next));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    #[test]
    fn panel_id_round_trips_through_pane_index_of_panel() {
        for i in 0..MAX_PANES {
            assert_eq!(pane_index_of_panel(&panel_id(i)), Some(i));
            assert_eq!(panel_id(i), format!("terminal-{i}"));
        }
        // Non-panel ids (the splitter tag, a window id, garbage) are not panes.
        assert_eq!(pane_index_of_panel(&boot_split_id(0)), None);
        assert_eq!(pane_index_of_panel("pane-0"), None); // window-id namespace
        assert_eq!(pane_index_of_panel("terminal-"), None); // no index
        assert_eq!(pane_index_of_panel("terminal-x"), None); // non-numeric
        assert_eq!(pane_index_of_panel(&panel_id(MAX_PANES)), None); // out of range
    }

    #[test]
    fn boot_topology_is_a_right_nested_horizontal_row() {
        // 0 panes -> no topology.
        assert!(build_boot_topology(0).is_none());

        // 1 pane -> a single leaf, no split.
        let one = build_boot_topology(1).expect("one pane docks");
        assert_eq!(one.leaf_count(), 1);
        assert_eq!(one.split_count(), 0);
        assert_eq!(one.panel_ids(), vec![panel_id(0)]);

        // 2 panes -> one Horizontal split (== the old row default at 2 panes).
        let two = build_boot_topology(2).expect("two panes dock");
        assert_eq!(two.leaf_count(), 2);
        assert_eq!(two.split_count(), 1);
        assert_eq!(two.split_ids(), vec![boot_split_id(0)]);
        assert_eq!(two.panel_ids(), vec![panel_id(0), panel_id(1)]);
        let DockNode::Split { orientation, .. } = two.root() else {
            panic!("two-pane root is a Split");
        };
        assert_eq!(*orientation, SplitterOrientation::Horizontal);

        // 4 panes -> three Horizontal dividers, panes in tile order, ids gapless.
        let four = build_boot_topology(4).expect("four panes dock");
        assert_eq!(four.leaf_count(), 4);
        assert_eq!(four.split_count(), 3);
        assert_eq!(
            four.panel_ids(),
            (0..4).map(panel_id).collect::<Vec<_>>(),
            "panes stay in left-to-right tile order"
        );
        assert_eq!(
            four.split_ids(),
            (0..3).map(boot_split_id).collect::<Vec<_>>(),
            "boot dividers are ids 0..n-1 in pre-order"
        );
    }

    #[test]
    fn use_split_ratio_is_per_id_memoised_and_seeds_default() {
        let owner = Owner::new();
        owner.run(|| {
            let a = use_split_ratio(boot_split_id(0), SPLIT_RATIO_DEFAULT);
            let b = use_split_ratio(boot_split_id(0), SPLIT_RATIO_DEFAULT);
            assert!(Rc::ptr_eq(&a, &b), "memoised by Split id");
            assert!((a.get() - SPLIT_RATIO_DEFAULT).abs() < f32::EPSILON);
            a.set(0.7);
            assert!(
                (use_split_ratio(boot_split_id(0), SPLIT_RATIO_DEFAULT).get() - 0.7).abs()
                    < f32::EPSILON,
                "drag re-weights; the seed does not reset an existing slot"
            );
            assert!(
                (use_split_ratio(boot_split_id(1), SPLIT_RATIO_DEFAULT).get()
                    - SPLIT_RATIO_DEFAULT)
                    .abs()
                    < f32::EPSILON,
                "a different divider is independent"
            );
        });
    }

    #[test]
    fn float_collapses_and_dock_back_restores_index_order() {
        let owner = Owner::new();
        owner.run(|| {
            // Boots with the default 2 docked panes (no SPRAG_GUI_PANES in test env).
            let topo = use_dock_topology();
            assert_eq!(
                topo.get().expect("boots docked").panel_ids(),
                vec![panel_id(0), panel_id(1)]
            );

            // Float pane 0 -> the sibling (pane 1) promotes; pane 0 leaves the tree.
            float_pane(0);
            let after = topo.get().expect("one pane still docked");
            assert_eq!(after.panel_ids(), vec![panel_id(1)]);
            assert_eq!(after.split_count(), 0, "the split collapsed with the leaf");

            // Dock pane 0 back -> index order restored (pane 0 before pane 1).
            dock_pane(0);
            assert_eq!(
                topo.get().expect("docked").panel_ids(),
                vec![panel_id(0), panel_id(1)],
                "dock-back keeps left-to-right index order"
            );

            // Float every pane -> the tree empties to None (no docked panes).
            float_pane(0);
            float_pane(1);
            assert!(
                topo.get().is_none(),
                "floating the last docked pane empties the tree"
            );

            // Dock one back from empty -> a single leaf, no split.
            dock_pane(1);
            let single = topo.get().expect("one pane docked again");
            assert_eq!(single.panel_ids(), vec![panel_id(1)]);
            assert_eq!(single.split_count(), 0);
        });
    }
}
