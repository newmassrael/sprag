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
//! ## Two selectable dock models (R77): collapse (default) | placeholder
//!
//! [`use_dock_topology`] holds `Signal<Option<DockTopology>>` — the tree of the panes'
//! leaves, built at boot ([`build_boot_topology`]) over ALL panes. How a FLOAT affects
//! this tree is chosen by [`DockMode`](crate::dock::DockMode) (`SPRAG_GUI_DOCK_MODE`):
//!
//!  - **`Collapse` (DEFAULT, R60):** floating REMOVES the pane's leaf ([`float_pane`]) so
//!    the remaining panes reclaim the space — the terminal-multiplexer fill (tmux/zellij).
//!    Docking back re-inserts it index-relative ([`dock_pane`]). The windows-signal and
//!    this tree CO-MUTATE (the tree tracks float state); `None` = all panes floated.
//!  - **`Placeholder` (opt-in, R72):** the floated pane's leaf STAYS; the view paints a
//!    [`view_floating_placeholder`](pinion_widget_paint::dock::view_floating_placeholder)
//!    holding its slot. The windows-signal ([`crate::dock::use_windows_topology`]) is then
//!    the SOLE float authority, ORTHOGONAL to this tree (never co-mutated by float/dock).
//!    Mirrors pinion's `hello-dock-panels-editor` (one DockTopology, leaves never removed).
//!
//! [`docked_pane_indices`] works for BOTH: it DERIVES the docked set as the tree's leaves
//! filtered by `!`[`is_pane_floating`](crate::dock::is_pane_floating). In placeholder mode
//! the filter is load-bearing (it drops the still-present floated leaves — R61's deferred
//! "membership is derived, not a third stored copy" cleanup); in collapse mode the floated
//! panes aren't leaves anyway, so the filter is redundant-but-correct. The tree is also
//! restructured by reorganize gestures (drag-to-dock + zone-redock, via
//! [`use_dock_reorganizer`]'s [`DockReorganizer`]) in both modes.
//!
//! ## Why both models exist (R60 → R72 → R77 — keep the rationale of each)
//!
//! R60 chose **collapse** for the TWM fill (siblings reclaim a torn-off pane's space).
//! R72 reversed to **placeholder** (the slot is held) because it makes zone-honoring
//! cross-window redock trivially correct — the surviving leaf is what the reducer's
//! `resolve_drop` SSOT relocates to the drop zone (mooting PINION-PR34) —
//! and mirrors pinion's IDE-dock reference (VS Code dragging a panel out). Live-testing,
//! the held slot felt un-terminal-like (the user expected the fill), so **R77 made both
//! selectable and restored collapse as the DEFAULT.** The trade is inherent: collapse =
//! siblings reclaim, but cross-window redock lands index-relative (the leaf is gone, can't
//! be zone-placed); placeholder = zone-honoring redock, but the slot is held. Two
//! reasonable desktop semantics; the user picks per run.
//! ## Identity-keyed ids
//!
//! Each leaf carries a stable [`panel_id`] (`terminal-{i}`, mapping 1:1 to the tile
//! index via [`pane_index_of_panel`]); each Split a stable id ([`boot_split_id`]
//! for the boot tree, a fresh `reorg`-prefixed id for a reorganize-minted divider).
//! The per-split ratio Signal ([`use_split_ratio`]) is `Owner::cache`-keyed by that
//! id and SHARED between the view (`view_dock_surface`) and the drag
//! `SplitterExternal`, so a drag re-weights the painted panes. A Split id is the
//! panel-id-distinct tag the [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel)
//! header / `SplitterStyle` carry, so the scene tags never collide with the
//! per-pane [`pane_tag`](crate::terminal::pane_tag) (`sprag_gui.pane.{i}`) the grid
//! inside each leaf carries.

use crate::terminal::{MAX_PANES, use_terminal};
use pinion_core::reactive::{Owner, Signal};
use pinion_widget_paint::dock::{
    DockDropPreview, DockNode, DockReorganizer, DockSplitPosition, DockTopology, TopologyError,
};
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
/// the boot tree ([`build_boot_topology`] over the host's live pane count — its truth,
/// which in attach mode is the adopted set, not the env request). Float/dock
/// do NOT mutate it (placeholder model — see the module docs); only a reorganize
/// gesture ([`use_dock_reorganizer`]) restructures it. The shell's `view` subscribes
/// it (a `set` repaints) and `reconcile_externals` re-walks it to register a
/// reorganize-minted Split's drag External (pinion R689).
pub(crate) fn use_dock_topology() -> Rc<Signal<Option<DockTopology>>> {
    let owner = Owner::current().expect("use_dock_topology() requires an active Owner scope");
    // The boot tree spans the HOST's pane count (its truth): in attach mode the GUI
    // ADOPTS the host's live set, so the tree must match that count, not the env
    // `SPRAG_GUI_PANES` request (they coincide in spawn mode). Resolve `use_terminal`
    // (idempotent; `create_extra_externals` boots it first) BEFORE the cache factory —
    // an `Owner::cache` factory must not nest another cache resolution, and
    // `use_terminal` resolves the SESSION_KEY slot.
    let count = use_terminal().host.pane_count();
    owner.cache(TOPOLOGY_KEY, move || {
        Signal::new(build_boot_topology(count))
    })
}

/// `Owner::cache` key for the shared drag-to-dock reorganize coordinator.
const REORGANIZER_KEY: &str = "sprag_gui.dock_reorganizer";

/// The ONE shared drag-to-dock reorganize coordinator (pinion R1081/PR-29, P2). Holds
/// the dock-tree topology Signal — PR-29.1 (pinion R1084) made [`DockReorganizer`] total
/// over `Option<DockTopology>`, so sprag's `Option<DockTopology>`
/// ([`use_dock_topology`]) is accepted DIRECTLY (no second signal): a reorganize on an
/// empty (`None`, the zero-pane edge) or single-leaf dock is a no-op, which is the only
/// correct result (no source/target to move). Cached (`Owner::cache`) so every per-panel
/// [`DockPanelExternal`](pinion_widget_paint::dock::DockPanelExternal) drag source shares
/// ONE coordinator → a pointer drop mints split ids from one `split_seq` counter. The
/// topology dep is resolved BEFORE the cache factory (an `Owner::cache` factory must not
/// nest another `cache` resolution).
///
/// `with_tabbing(false)` (pinion R1111/R1112 / PINION-PR37): a terminal multiplexer docks
/// by SPLIT only — "stack two terminals as tabs in one slot" (the centre-zone `Tabify`) is
/// not a terminal idiom. Tabbing-off makes pinion's drop classification route a centre drop
/// to the nearest edge (split) instead of a tab well, on EVERY path (drag, RPC, cross-window
/// redock — R1112 lifted the flag to this one surface SSOT). So no `Center`/tab drop zone
/// appears for the user.
pub(crate) fn use_dock_reorganizer() -> Rc<DockReorganizer> {
    let topology = use_dock_topology();
    Owner::current()
        .expect("use_dock_reorganizer() requires an active Owner scope")
        .cache(REORGANIZER_KEY, move || {
            DockReorganizer::new(topology).with_tabbing(false)
        })
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

// ── Collapse-model primitives (R60, restored R77) ───────────────────────────────
// These mutate the topology and are called by [`crate::dock`] ONLY in
// [`DockMode::Collapse`](crate::dock::DockMode) (the default). In
// [`DockMode::Placeholder`] the topology is static under float/dock (the leaf stays,
// painting a placeholder) and these are never called — see the module docs.

/// `Owner::cache` key for the dock-back Split-id sequence counter.
const SPLIT_SEQ_KEY: &str = "sprag_gui.dock_split_seq";

/// Mint a fresh, never-reused Split id for a collapse-mode dock-back insert
/// (`sprag_gui.split.dock.{seq}`). A monotonic `Owner::cache`-backed counter, so a
/// re-inserted divider gets a unique id (its ratio starts at the default — a
/// dock/undock cycle does not remember the old boundary's ratio, a v1 bound). Distinct
/// from the boot ids ([`boot_split_id`]) so the two spaces never collide.
fn next_dock_split_id() -> String {
    let seq = Owner::current()
        .expect("next_dock_split_id() requires an active Owner scope")
        .cache(SPLIT_SEQ_KEY, || Signal::new(0_u64));
    let n = seq.get();
    seq.set(n + 1);
    format!("sprag_gui.split.dock.{n}")
}

/// Collapse-mode undock: remove pane `index`'s leaf from the dock tree so the remaining
/// panes reclaim the space (the sibling sub-tree promotes into the parent Split's place).
/// Floating the LAST docked pane empties the tree (`None`); floating a pane already
/// absent is a no-op. Called by `crate::dock::push_float` in `DockMode::Collapse`.
pub(crate) fn float_pane(index: usize) {
    let topo = use_dock_topology();
    let Some(current) = topo.get() else {
        return; // nothing docked
    };
    match current.remove_leaf(&panel_id(index)) {
        Ok(next) => topo.set(Some(next)),
        // RootRemoval = pane `index` was the sole docked leaf -> no docked panes left.
        Err(TopologyError::RootRemoval) => topo.set(None),
        // PanelNotFound (already floated) — leave the tree unchanged.
        Err(_) => {}
    }
}

/// Collapse-mode dock-back: re-insert pane `index`'s leaf, preserving left-to-right pane
/// INDEX order — split the smallest-indexed docked pane whose index exceeds `index` (the
/// new leaf takes the `First`/left slot), or append after the last docked pane
/// (`Second`/right) when `index` is the largest. An empty tree (`None`) becomes the
/// single leaf. The new divider gets a fresh id ([`next_dock_split_id`]) at the default
/// ratio. Called by [`crate::dock::redock_pane`] in `DockMode::Collapse`.
///
/// **v1 bound:** index-relative, not drop-zone-relative — a collapse-mode cross-window
/// redock lands at the pane's index home, not where it was dropped (zone-honoring redock
/// needs `DockMode::Placeholder`, where the surviving leaf is relocated by the reducer's
/// `resolve_drop` SSOT). See the module docs + PINION-PR34.
pub(crate) fn dock_pane(index: usize) {
    let topo = use_dock_topology();
    let next = match topo.get() {
        None => DockTopology::single(panel_id(index)),
        Some(current) => {
            let mut docked: Vec<usize> = current
                .panel_ids()
                .iter()
                .filter_map(|p| pane_index_of_panel(p))
                .collect();
            // Index order is emergent (not enforced) — sort so the "first index >
            // `index`" insertion point is unconditionally correct.
            docked.sort_unstable();
            let split_id = next_dock_split_id();
            let inserted = match docked.iter().find(|&&j| j > index) {
                Some(&j) => current.split_leaf_into(
                    &panel_id(j),
                    panel_id(index),
                    split_id,
                    SplitterOrientation::Horizontal,
                    SPLIT_RATIO_DEFAULT,
                    DockSplitPosition::First,
                ),
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

    /// PLACEHOLDER mode: floating leaves a pane's leaf in the tree (the windows-signal is
    /// the sole float authority), and `docked_pane_indices` derives membership by FILTERING
    /// the leaves. Modeled by pushing the window directly (which is exactly what
    /// placeholder-mode `push_float` does — window-only, leaf untouched).
    #[test]
    fn docked_membership_filters_floating_panes_while_the_leaf_survives() {
        use pinion_shell::{SizeStrategy, WindowSpec};
        let owner = Owner::new();
        owner.run(|| {
            crate::dock::set_dock_mode(crate::dock::DockMode::Placeholder);
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

    /// COLLAPSE mode (the default): floating REMOVES a pane's leaf so the siblings reclaim
    /// the space ([`float_pane`]), and docking back re-inserts it index-relative
    /// ([`dock_pane`]). Exercises the restored R60 primitives directly (no windows-signal /
    /// `use_terminal` needed — float_pane/dock_pane are pure topology mutations).
    #[test]
    fn collapse_float_removes_the_leaf_and_dock_restores_index_order() {
        let owner = Owner::new();
        owner.run(|| {
            use_dock_topology().set(build_boot_topology(4));
            assert_eq!(docked_pane_indices(), vec![0, 1, 2, 3]);

            // Float a MIDDLE pane (1) -> its leaf is REMOVED, siblings reclaim: [0, 2, 3].
            float_pane(1);
            assert_eq!(docked_pane_indices(), vec![0, 2, 3]);
            assert_eq!(
                use_dock_topology().get().expect("still docked").panel_ids(),
                vec![panel_id(0), panel_id(2), panel_id(3)],
                "collapse: the floated pane's leaf is gone from the tree"
            );

            // Dock pane 1 back -> re-inserted in INDEX order (before pane 2): [0, 1, 2, 3].
            dock_pane(1);
            assert_eq!(
                docked_pane_indices(),
                vec![0, 1, 2, 3],
                "collapse dock-back restores left-to-right index order"
            );

            // Float every pane -> the tree empties to `None` (no docked panes).
            for i in 0..4 {
                float_pane(i);
            }
            assert!(
                use_dock_topology().get().is_none(),
                "collapse: floating the last docked pane empties the tree to None"
            );
        });
    }
}
