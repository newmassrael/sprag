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
//! ## The topology is static under float/dock (placeholder model, R72)
//!
//! [`use_dock_topology`] holds `Signal<Option<DockTopology>>` — the tree of ALL panes'
//! leaves, built once at boot ([`build_boot_topology`]) and NEVER mutated by floating or
//! docking a pane. A pane floats by gaining a `pane-{i}` OS window in the windows-signal
//! ([`crate::dock::use_windows_topology`]) — the SOLE floating authority; its leaf STAYS
//! in this tree as a placeholder (the view paints a [`view_floating_placeholder`](pinion_widget_paint::dock::view_floating_placeholder)
//! for it — see [`crate::view`]) holding its slot. This mirrors pinion's `hello-dock-panels-editor`
//! reference consumer (one DockTopology, leaves never removed; float = a WindowSpec).
//!
//! So the two authorities are ORTHOGONAL: the windows-signal owns "which panes float";
//! this topology owns "the dock layout shape + ratios". They are never co-mutated.
//! [`docked_pane_indices`] DERIVES the docked set from the windows-signal (filtering the
//! tree's leaves by [`crate::dock::is_pane_floating`]) — this lands R61's deferred cleanup
//! (membership from one authority; the tree is held shape state). The tree is restructured
//! ONLY by reorganize gestures (drag-to-dock + zone-redock, via [`use_dock_reorganizer`]'s
//! [`DockReorganizer`]), never by a plain float/dock. `None` = the zero-pane edge only.
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
use pinion_widget_paint::dock::{DockDropPreview, DockNode, DockReorganizer, DockTopology};
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
/// its right). Stable for the life of the boot tree; a reorganize gesture
/// ([`use_dock_reorganizer`]) mints fresh `reorg`-prefixed ids instead, so the two id
/// spaces never collide.
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

/// The dock-tree topology Signal — the layout of ALL panes' leaves (`None` = the
/// zero-pane edge only). Cached in the root owner (the view fn, the splitter-External
/// registration, and the reorganizer resolve the same shared slot), seeded once with
/// the boot tree ([`build_boot_topology`] over all [`pane_count`] panes). Float/dock
/// do NOT mutate it (placeholder model — see the module docs); only a reorganize
/// gesture ([`use_dock_reorganizer`]) restructures it. The shell's `view` subscribes
/// it (a `set` repaints) and `reconcile_externals` re-walks it to register a
/// reorganize-minted Split's drag External (pinion R689).
pub(crate) fn use_dock_topology() -> Rc<Signal<Option<DockTopology>>> {
    Owner::current()
        .expect("use_dock_topology() requires an active Owner scope")
        .cache(TOPOLOGY_KEY, || {
            Signal::new(build_boot_topology(pane_count()))
        })
}

/// `Owner::cache` key for the shared drag-to-dock reorganize coordinator.
const REORGANIZER_KEY: &str = "sprag_gui.dock_reorganizer";

/// The ONE shared drag-to-dock reorganize coordinator (pinion R1081/PR-29, P2). Holds
/// the dock-tree topology Signal — PR-29.1 (pinion R1084) made [`DockReorganizer`] total
/// over `Option<DockTopology>`, so sprag's collapse-to-`None` topology
/// ([`use_dock_topology`]) is accepted DIRECTLY (no second signal): a reorganize on an
/// empty (`None`) or single-leaf dock is a no-op, which is the only correct result (no
/// source/target to move). Cached (`Owner::cache`) so every per-panel
/// [`DockPanelExternal`](pinion_widget_paint::dock::DockPanelExternal) drag source shares
/// ONE coordinator → a pointer drop mints split ids from one `split_seq` counter. The
/// topology dep is resolved BEFORE the cache factory (an `Owner::cache` factory must not
/// nest another `cache` resolution).
pub(crate) fn use_dock_reorganizer() -> Rc<DockReorganizer> {
    let topology = use_dock_topology();
    Owner::current()
        .expect("use_dock_reorganizer() requires an active Owner scope")
        .cache(REORGANIZER_KEY, move || DockReorganizer::new(topology))
}

/// `Owner::cache` key for the shared live drag-to-dock drop-preview.
const DROP_PREVIEW_KEY: &str = "sprag_gui.dock_drop_preview";

/// The ONE shared live drop-preview (pinion R1082/PR-29, P2): the dragged panel's
/// [`DockPanelExternal::drag_to`](pinion_widget_paint::dock::DockPanelExternal) writes
/// it on every cursor move (the target panel + its
/// [`DockDropZone`](pinion_widget_paint::dock::DockDropZone) under the cursor),
/// and [`view_main`](crate::view)'s `drop_zone` callback reads it to paint the target
/// panel's zone affordance. `None` between drags. Cached (`Owner::cache`) so every panel
/// external + the view fn reach the SAME Signal — a `drag_to` `set` repaints the
/// highlight reactively.
pub(crate) fn use_drop_preview() -> Rc<Signal<Option<DockDropPreview>>> {
    Owner::current()
        .expect("use_drop_preview() requires an active Owner scope")
        .cache(DROP_PREVIEW_KEY, || Signal::new(None))
}

/// The tile indices of the panes currently DOCKED — the dock-tree's leaves (in
/// left-to-right paint order) MINUS those whose pane is floating
/// ([`crate::dock::is_pane_floating`]), or empty at the zero-pane edge. The SINGLE
/// authority for "which panes does the main window show" — read by BOTH the paint
/// ([`crate::view::view_for_window`]) and the per-window a11y projection
/// ([`crate::a11y::access_nodes_for_window`]), so they can never disagree.
///
/// In the placeholder model (R72) the tree holds EVERY pane's leaf always; a floated
/// pane's leaf stays (painting a placeholder), so docked membership is DERIVED here by
/// filtering the leaf set by the windows-signal (the sole floating authority). This is
/// R61's deferred "membership is a projection of one authority" cleanup: the tree is no
/// longer co-mutated to track float state — it is filtered.
pub(crate) fn docked_pane_indices() -> Vec<usize> {
    use_dock_topology()
        .get()
        .map(|t| {
            t.panel_ids()
                .iter()
                .filter_map(|p| pane_index_of_panel(p))
                .filter(|&i| !crate::dock::is_pane_floating(i))
                .collect()
        })
        .unwrap_or_default()
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

// Float/dock no longer mutate the topology (R72 placeholder model): a pane floats by
// gaining a `pane-{i}` window in the windows-signal ([`crate::dock`]), and its leaf
// STAYS in this tree painting a placeholder. The former `float_pane` (remove_leaf) and
// `dock_pane` (index-relative re-insert) are gone; the tree is restructured only by a
// reorganize gesture ([`use_dock_reorganizer`]), and dock-back placement is now
// zone-driven (the reorganizer's `apply_zone_redock`), not index-relative.

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;
    use pinion_widget_paint::splitter::SplitterOrientation;

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
    fn docked_membership_filters_floating_panes_while_the_leaf_survives() {
        use pinion_shell::{SizeStrategy, WindowSpec};
        let owner = Owner::new();
        owner.run(|| {
            // Seed a 4-pane tree directly (independent of SPRAG_GUI_PANES). No floating
            // windows yet (the windows-signal seeds with just the main window), so all
            // four panes read as docked.
            use_dock_topology().set(build_boot_topology(4));
            assert_eq!(docked_pane_indices(), vec![0, 1, 2, 3]);

            // Float a MIDDLE pane (1) by pushing its `pane-1` window — the windows-signal
            // is the SOLE floating authority. The topology is NOT touched (placeholder
            // model): the leaf stays so the main window can paint its placeholder.
            let windows = crate::dock::use_windows_topology();
            let mut w = windows.get();
            w.push(WindowSpec::new(
                Cow::Owned(crate::dock::pane_window_id(1)),
                "floating pane 1",
                SizeStrategy::Fixed {
                    width: 100,
                    height: 100,
                },
            ));
            windows.set(w);

            // Membership DROPS pane 1 (filtered by is_pane_floating) ...
            assert_eq!(
                docked_pane_indices(),
                vec![0, 2, 3],
                "a floating pane is filtered from the docked set"
            );
            // ... but the topology STILL holds all four leaves (no collapse).
            assert_eq!(
                use_dock_topology()
                    .get()
                    .expect("topology intact")
                    .panel_ids(),
                (0..4).map(panel_id).collect::<Vec<_>>(),
                "the floated pane's leaf survives in the tree (placeholder model)"
            );

            // De-float pane 1 (drop its window) -> membership restores, leaf unchanged.
            let mut w = windows.get();
            w.retain(|s| s.id != crate::dock::pane_window_id(1));
            windows.set(w);
            assert_eq!(docked_pane_indices(), vec![0, 1, 2, 3]);
        });
    }
}
