//! The dock split-tree layout (R60): the docked panes are arranged by a pinion
//! [`DockTopology`] — an identity-keyed recursive binary split-tree — replacing
//! the former flat row/grid model (R38 row + R40 grid + position-keyed dividers).
//! This module owns the topology MODEL (the tree + its mutation on dock/undock and
//! the per-split ratio Signals); the view layer ([`crate::view`]) lowers it to
//! pixels via [`view_dock_surface_chrome`](pinion_widget_paint::dock::view_dock_surface_chrome),
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
//! ## The tree is a PROJECTION of the host's arrangement, not a fork (R149)
//!
//! [`use_dock_topology`] holds `Signal<Option<DockTopology>>` — the tiled panes' leaves,
//! PROJECTED from the host's arrangement ([`project_layout`]) and written BACK to it when a
//! gesture settles ([`sync_layout`]). The host is the authority: it owns which panes are
//! tiled, how they are split, at what share, and which are floated out of the tiling — so
//! the arrangement outlives this client (see [`sprag_terminal::layout`]). This client owns
//! how that becomes pixels, and resolving the gesture that PROPOSES the next arrangement.
//!
//! Floating removes the pane's leaf, so the remaining panes reclaim the space — the
//! terminal-multiplexer fill (tmux/zellij). That removal is the HOST's (`Window::set_floating`
//! and its reconcile over `panes − floating`); this client asks for it and re-projects the
//! answer. `None` = no pane is tiled (every one floated, or none exists).
//!
//! [`docked_pane_indices`] DERIVES the docked set as the tree's leaves — with no float
//! filter, because a floated pane HAS no leaf. The tree is restructured by reorganize
//! gestures (drag-to-dock + zone-redock, via [`use_dock_reorganizer`]'s [`DockReorganizer`]);
//! [`sync_layout`] writes the settled result back and adopts the canonical answer.
//!
//! ## Why the dock-model choice is gone (R60 → R72 → R77 → R149)
//!
//! R77 made two models selectable (`SPRAG_GUI_DOCK_MODE`): **collapse** (the default —
//! siblings reclaim a torn-off pane's space) and **placeholder** (the floated pane's leaf
//! stays, painting a placeholder that holds its slot). Placeholder existed for ONE reason:
//! it made zone-honoring cross-window redock trivially correct, since a SURVIVING leaf is
//! what the reducer's `resolve_drop` SSOT relocates to the drop zone (PINION-PR34). Both
//! halves of that rationale are now gone. pinion R1173 made `dock_panel_at_resolved_zone`
//! total over an ABSENT source leaf, so collapse zone-honors too; and R149 made the host the
//! arrangement authority, which settles the question outright — a floated pane is not tiled,
//! so there is no leaf to hold a slot. Keeping placeholder would mean the host's tree
//! carrying panes it does not tile and this client filtering them back out: a second
//! authority diverging from the first, the exact pattern R124/R148 retired. So the mode is
//! deleted, collapse (the terminal idiom the user chose as default) is the only model, and a
//! redock lands where it was DROPPED because this client resolves the drop and writes the
//! resulting tree.
//!
//! ## Identity-keyed ids
//!
//! Each leaf carries a stable [`panel_id`] (`terminal-{i}`, mapping 1:1 to the tile
//! index via [`pane_index_of_panel`]); each Split a stable id ([`split_tag`] of
//! the host's `SplitId`, a fresh `reorg`-prefixed id for a reorganize-minted divider).
//! The per-split ratio Signal ([`use_split_ratio`]) is `Owner::cache`-keyed by that
//! id and SHARED between the view (`view_dock_surface_chrome`) and the drag
//! `SplitterExternal`, so a drag re-weights the painted panes. A Split id is the
//! panel-id-distinct tag the [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel)
//! header / `SplitterStyle` carry, so the scene tags never collide with the
//! per-pane [`pane_tag`](crate::terminal::pane_tag) (`sprag_gui.pane.{i}`) the grid
//! inside each leaf carries.

use crate::terminal::MAX_PANES;
use pinion_core::reactive::{Owner, Signal};
use pinion_widget_paint::dock::{DockDropPreview, DockNode, DockReorganizer, DockTopology};
use pinion_widget_paint::splitter::SplitterOrientation;
use sprag_terminal::{
    LayoutNode, LayoutNodeWire, LayoutSnapshot, LayoutTree, LayoutWire, PaneId, SplitDir, SplitId,
};
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

/// The `panel_id` prefix — a leaf's stable id is `terminal-{i}` for tile index `i`.
/// A readable token (the header's FALLBACK label when a child has set no `OSC` title —
/// see [`panel_id`]) that doubles as the panel's scene tag, kept DISTINCT from the
/// per-pane [`pane_tag`](crate::terminal::pane_tag) (`sprag_gui.pane.{i}`) the inner
/// grid carries (the dock panel wraps the grid, so the two tags must not collide)
/// and from the [`pane_window_id`](crate::dock::pane_window_id) (`pane-{i}`) window
/// namespace.
const PANEL_ID_PREFIX: &str = "terminal-";

/// The stable `panel_id` of the pane at tile `index` — the dock-tree leaf IDENTITY + scene
/// tag + RPC address + `DockPanelExternal` key. Inverse of [`pane_index_of_panel`].
///
/// **Identity only, since R130.** It is no longer what any header PAINTS: every title
/// surface now shows [`pane_display_title`](crate::view::pane_display_title) — the child's
/// live `OSC` window title — via pinion R1318's `DockPanelChrome` display-title provider
/// (docked header + tab label), the floater header, and the torn-off placeholder label.
/// `panel_id` remains the string those surfaces FALL BACK to when a child has set no title,
/// but it is never the address and the display name at once: a child rewrites its title
/// every prompt, so a pane must not be able to rename its own address (R70).
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

/// The scene / cache tag of the host's split `id` — `sprag_gui.split.{n}`.
///
/// Keyed on the HOST's [`SplitId`] (a mint sequence), NOT on the left leaf's tile index as
/// the retired boot scheme was: a slot-keyed id re-binds a divider's drag state when panes
/// reshuffle, while a `SplitId` follows its boundary for life. A contiguous boot yields the
/// identical strings (slot k == sequence k); a holed set does not. Distinct from the
/// reorganize / dock-back id spaces so the tag spaces never collide.
pub(crate) fn split_tag(id: SplitId) -> String {
    format!("{SPLIT_TAG_PREFIX}{}", id.0)
}

/// The scene / cache tag prefix of a host divider — `sprag_gui.split.{n}`.
///
/// A const for the same reason [`PANEL_ID_PREFIX`] is: [`split_tag`] and [`split_id_of_tag`]
/// are an exact inverse pair, and a prefix typed twice can be renamed once. Renaming only the
/// formatter would make the parser return `None` for EVERY divider, so the host would mint a
/// fresh `SplitId` on every settle — a silent, unbounded id churn no type check would catch.
const SPLIT_TAG_PREFIX: &str = "sprag_gui.split.";

/// The scene tag of the CANONICAL REORGANIZE SURFACE — the dock addressed as DATA.
///
/// An agent (or a test) drives a dock gesture by INTENT through
/// `scene/invoke /sprag_gui.dock/external/reorganize` with a `{source, target, zone}`, reads
/// the in-flight gesture from `drop_preview`, what committed from `last_outcome`, and the tree
/// from `topology` — pinion's §2 #2 RPC-as-primary-path contract, with no pixels anywhere.
/// Distinct from the per-pane [`panel_id`] and per-divider [`split_tag`] namespaces.
pub(crate) const DOCK_REORGANIZE_TAG: &str = "sprag_gui.dock";

/// The event suffix of the intent pinion's `SplitterExternal` fires on drag-end (PR-56),
/// carrying the settled ratio — reaching the reducer as `{split_tag}.ratio_committed`.
///
/// A sprag const because pinion emits the tag as a string literal and exports no
/// symbol for it; this mirrors that literal, so a drift shows up as a missed commit
/// (the ratio silently stops persisting) rather than a compile error — hence pinned here,
/// next to the divider tag it rides on, and asserted by the commit test.
pub(crate) const RATIO_COMMITTED_EVENT: &str = "ratio_committed";

/// Split `id`'s ratio `Signal` (left/top share, `[0, 1]`), an `Owner::cache`-backed
/// `Rc<Signal<f32>>` SHARED between the read side (the view's `view_dock_surface_chrome`
/// `split_state` callback) and the write side (the `SplitterExternal` registered at
/// the same id) — both resolve the same root-owner slot keyed on the Split id, so a
/// drag (`set`) re-weights the painted panes. `initial` seeds the slot on first
/// resolution (the topology's declared ratio, threaded through by the walker), so
/// the topology stays the single source of the initial value.
pub(crate) fn use_split_ratio(id: impl Into<Cow<'static, str>>, initial: f32) -> Rc<Signal<f32>> {
    let ratios = use_split_ratios();
    let id = id.into();
    if let Some(existing) = ratios.borrow().get(&id) {
        return Rc::clone(existing);
    }
    let signal = Rc::new(Signal::new(initial));
    ratios.borrow_mut().insert(id, Rc::clone(&signal));
    signal
}

/// `Owner::cache` key for the per-divider ratio table.
const SPLIT_RATIOS_KEY: &str = "sprag_gui.split_ratios";

/// Every live divider's ratio signal, keyed by its scene tag — the shared table behind
/// [`use_split_ratio`], held in one `Owner::cache` slot so it can be PRUNED (see
/// [`use_split_ratios`]).
type SplitRatios = Rc<RefCell<std::collections::HashMap<Cow<'static, str>, Rc<Signal<f32>>>>>;

/// Every live divider's ratio signal, by tag — the storage behind [`use_split_ratio`].
///
/// One table in ONE `Owner::cache` slot, rather than one cache slot per divider, for a
/// reason the per-slot form could not solve: `Owner::cache` never evicts. A divider's tag is
/// minted (pinion's transient `reorg-split-{n}`, then the host's canonical id) and RETIRED
/// (a `SplitId` is never reused; a collapsed split's id is gone for good), so a per-slot
/// scheme leaked a `Signal` per gesture, forever, in a process designed to outlive many of
/// them. Owning the table lets [`prune_split_ratios`] drop what the tree no longer has.
fn use_split_ratios() -> SplitRatios {
    Owner::current()
        .expect("use_split_ratio() requires an active Owner scope")
        .cache(SPLIT_RATIOS_KEY, || {
            RefCell::new(std::collections::HashMap::new())
        })
}

/// Drop the ratio signal of every divider `topology` no longer holds — the eviction the
/// arc's long-lived session needs (see [`use_split_ratios`]).
///
/// Called on each adopt, which is the one place the set of live dividers changes. A signal
/// pinion's `SplitterExternal` still holds keeps working (it owns an `Rc`); it simply stops
/// being reachable by tag, which is correct — that tag names a boundary that no longer exists.
fn prune_split_ratios(topology: Option<&DockTopology>) {
    let live: Vec<String> = topology
        .map(|t| t.split_ids().iter().map(|s| (*s).to_owned()).collect())
        .unwrap_or_default();
    use_split_ratios()
        .borrow_mut()
        .retain(|tag, _| live.iter().any(|l| l == tag.as_ref()));
}

/// Project the HOST's logical arrangement onto this client's dock surface — the ONE
/// place the host's `LayoutTree` becomes a pinion [`DockTopology`].
///
/// This is the seam the whole detach/reattach arc rests on: the host is to own WHICH panes
/// are split, in what order, at what proportion (so it survives a detach); the client owns
/// how that becomes pixels. Nothing here invents arrangement — it only translates identity
/// to slots ([`SlotView::slot_of`](crate::slotview::SlotView::slot_of)) and the host's
/// ratio into pinion's initial one. Runs on every adopt ([`adopt_layout`]), not once at boot
/// — the once-fired seed that then forked from the host forever is what R149 retired.
///
/// **LOSSY, deliberately** (see the collapse rule below), which is why [`sync_layout`] must
/// never write an un-projection of this surface back without first checking that its pane
/// set still matches the host's.
///
/// A leaf whose pane this client cannot render yet (`slot_of` -> `None`, the "renderable
/// now" contract) COLLAPSES into its sibling, exactly as a removed leaf does — so a
/// not-yet-admitted host pane yields a smaller correct tree, never a wrong slot. An empty
/// arrangement -> `None` (the zero-pane edge).
fn project_layout(
    tree: &LayoutTree,
    slot_of: &impl Fn(PaneId) -> Option<usize>,
) -> Option<DockTopology> {
    let node = project_node(tree.root()?, slot_of)?;
    // try_new validates pinion's invariants (unique panel/split ids, finite ratios); the
    // host mints unique ids + reconciles a unique pane set, so a rejection means the host
    // handed us something we cannot render — drop to the zero-pane edge rather than panic.
    DockTopology::try_new(node)
        .inspect_err(
            |error| tracing::warn!(target: "sprag_gui::split", ?error, "host layout rejected"),
        )
        .ok()
}

/// Translate one host [`LayoutNode`] into a [`DockNode`] (see [`project_layout`]).
fn project_node(node: &LayoutNode, slot_of: &impl Fn(PaneId) -> Option<usize>) -> Option<DockNode> {
    match node {
        LayoutNode::Leaf(pane) => slot_of(*pane).map(|slot| DockNode::Leaf {
            panel_id: Cow::Owned(panel_id(slot)),
        }),
        LayoutNode::Split {
            id,
            dir,
            ratio,
            first,
            second,
        } => match (project_node(first, slot_of), project_node(second, slot_of)) {
            (Some(first), Some(second)) => Some(DockNode::Split {
                id: Cow::Owned(split_tag(*id)),
                orientation: match dir {
                    SplitDir::Horizontal => SplitterOrientation::Horizontal,
                    SplitDir::Vertical => SplitterOrientation::Vertical,
                },
                // The host's DURABLE share seeds pinion's INITIAL ratio; the live drag
                // value then lives in the per-split Signal (`use_split_ratio`).
                ratio: *ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
            // Only one side is renderable: it takes this split's place (no half-split).
            (Some(alone), None) | (None, Some(alone)) => Some(alone),
            (None, None) => None,
        },
    }
}

/// The host's split id behind a [`split_tag`] string, or `None` for a divider this client
/// minted itself — the inverse of [`split_tag`], and the hinge of the write path.
///
/// A gesture is resolved on THIS client's surface (that is what makes it feel instant), and
/// pinion mints its own id for a divider a drop creates (`reorg-split-{n}`). Such an id has
/// no [`SplitId`] and cannot: an id must be unique and never reused across the arrangement's
/// whole life, which only its owner can promise. So an unrecognised tag maps to `None` and
/// the HOST names it on the write ([`LayoutTree::set_from_wire`]), after which this client
/// re-projects and the divider carries a durable identity from then on.
fn split_id_of_tag(tag: &str) -> Option<SplitId> {
    tag.strip_prefix(SPLIT_TAG_PREFIX)?
        .parse()
        .ok()
        .map(SplitId)
}

/// Un-project this client's dock surface back into the host's language — the inverse of
/// [`project_layout`], and what turns a settled gesture into session state.
///
/// `None` means the surface is not representable as a host arrangement, in which case the
/// caller must NOT write. Two shapes reach it: a pinion `Tabs` well (unreachable —
/// [`use_dock_reorganizer`] runs `with_tabbing(false)` — but a real variant of pinion's type,
/// so it is refused honestly rather than assumed away), and a leaf whose slot holds no pane.
///
/// Returning `Some` is NOT on its own a licence to write: this reports only that every leaf
/// PRESENT could be named. A pane [`project_layout`] dropped is simply absent, and no
/// inspection here can see it — that check belongs to [`sync_layout`], against the host's
/// own pane set.
///
/// The ratio is read from the LIVE per-split signal ([`use_split_ratio`]), NOT from the
/// node: pinion's `DockNode::Split.ratio` is only the value the divider OPENED at, while the
/// user's drag lives in the signal. Reading the node instead would persist an arrangement
/// the user can see is not the one on their screen.
fn unproject_layout(
    topology: Option<&DockTopology>,
    pane_at: &impl Fn(usize) -> Option<PaneId>,
) -> Option<LayoutWire> {
    match topology {
        // No tiled panes is a legal arrangement, not a failure to represent one.
        None => Some(LayoutWire { root: None }),
        Some(topology) => Some(LayoutWire {
            root: Some(unproject_node(topology.root(), pane_at)?),
        }),
    }
}

/// Translate one pinion [`DockNode`] back into a host [`LayoutNodeWire`] (see
/// [`unproject_layout`]). `None` = unrepresentable.
fn unproject_node(
    node: &DockNode,
    pane_at: &impl Fn(usize) -> Option<PaneId>,
) -> Option<LayoutNodeWire> {
    match node {
        DockNode::Leaf { panel_id } => pane_index_of_panel(panel_id)
            .and_then(pane_at)
            .map(LayoutNodeWire::Leaf),
        DockNode::Split {
            id,
            orientation,
            ratio,
            first,
            second,
        } => Some(LayoutNodeWire::Split {
            id: split_id_of_tag(id),
            dir: match orientation {
                SplitterOrientation::Horizontal => SplitDir::Horizontal,
                SplitterOrientation::Vertical => SplitDir::Vertical,
            },
            // `id.clone()` not `id.to_string()`: this runs for every divider on every
            // painted frame, and the id is already a `Cow<'static, str>` — cloning a
            // `Borrowed` costs nothing, while `to_string` allocates each time.
            ratio: use_split_ratio(id.clone(), *ratio).get(),
            first: Box::new(unproject_node(first, pane_at)?),
            second: Box::new(unproject_node(second, pane_at)?),
        }),
        // A tab well has no host equivalent, and tabbing is off on this surface — so this
        // is unreachable rather than unhandled. Refuse the write instead of dropping panes.
        other => {
            tracing::warn!(
                target: "sprag_gui::split",
                ?other,
                "dock surface holds a node the host arrangement cannot express; not writing",
            );
            None
        }
    }
}

/// The panes an arrangement places, as a set — for checking that a write would not change
/// MEMBERSHIP (see [`sync_layout`]).
fn wire_panes(wire: &LayoutWire) -> std::collections::HashSet<PaneId> {
    fn walk(node: &LayoutNodeWire, out: &mut std::collections::HashSet<PaneId>) {
        match node {
            LayoutNodeWire::Leaf(pane) => {
                out.insert(*pane);
            }
            LayoutNodeWire::Split { first, second, .. } => {
                walk(first, out);
                walk(second, out);
            }
        }
    }
    let mut out = std::collections::HashSet::new();
    if let Some(root) = &wire.root {
        walk(root, &mut out);
    }
    out
}

/// What this client last learned from the host, plus the settle detector — the state
/// [`sync_layout`] needs to tell "the user is still dragging" from "the arrangement moved".
#[derive(Default)]
struct LayoutSync {
    /// The arrangement the host last told us it holds, at the revision it was at. A local
    /// surface that differs from this is an un-written gesture.
    seen: LayoutSnapshot,
    /// The PREVIOUS frame's un-projection. A gesture in flight changes every frame; one that
    /// matches the last frame has stopped, and only then is it worth writing.
    settling: Option<LayoutWire>,
}

/// `Owner::cache` key for the client's record of the host's arrangement.
const LAYOUT_SYNC_KEY: &str = "sprag_gui.layout_sync";

/// The shared [`LayoutSync`] state (root-owner cached, UI-thread only).
fn use_layout_sync() -> Rc<RefCell<LayoutSync>> {
    Owner::current()
        .expect("use_layout_sync() requires an active Owner scope")
        .cache(LAYOUT_SYNC_KEY, || RefCell::new(LayoutSync::default()))
}

/// Adopt `snapshot` as this client's projection — the ONE place the host's arrangement
/// becomes the dock surface.
///
/// Also drives each divider's live ratio to the adopted share — but ONLY where the host's
/// share actually CHANGED (`was`, the arrangement we last saw). Both halves matter:
///
/// * forcing at all is load-bearing, because [`use_split_ratio`] memoises per id, so a signal
///   that already exists ignores the seed — without it, an arrangement changed host-side
///   would re-project every divider's position except the one thing the user most directly set;
/// * forcing only what changed is what keeps a gesture alive. Every adopt runs for the WHOLE
///   window, so an unrelated host change — a plugin spawning a pane, another client's
///   gesture — would otherwise slam every divider back to its stored share, including the one
///   under the user's cursor mid-drag. A divider the host did not move is left exactly as the
///   user has it, and their in-flight drag survives news about some other pane.
///
/// A divider whose id the host has just minted has no signal yet and is created at the
/// right value.
pub(crate) fn adopt_layout(snapshot: &LayoutSnapshot, slots: &crate::slotview::SlotView) {
    let mut tree = LayoutTree::new();
    if let Err(error) = tree.set_from_wire(snapshot.tree.clone()) {
        // The host validates every write, so this means the two ends disagree on the shape.
        // Keep what is on screen rather than blanking it on a fact we cannot parse.
        tracing::error!(
            target: "sprag_gui::split",
            %error,
            "the host served an arrangement this client cannot render; keeping the current one",
        );
        return;
    }
    // Drive only the shares the host actually moved (see the fn docs).
    let was = use_layout_sync().borrow().seen.tree.clone();
    if let Some(root) = tree.root() {
        force_changed_ratios(root, &ratios_of(&was));
    }
    let projected = project_layout(&tree, &|pane| slots.slot_of(pane));
    // Evict the ratio signals of dividers this arrangement no longer has — a `SplitId` is
    // never reused, so without this a long session leaks one signal per retired boundary.
    prune_split_ratios(projected.as_ref());
    use_dock_topology().set(projected);
    // The tree carries only TILED panes, so the float set is the other half of the same
    // fact — project it too, or a floated pane would have neither a leaf nor a window and
    // would vanish (which is exactly what a reattach used to do to it).
    crate::dock::reconcile_float_windows(&snapshot.floating, &|pane| slots.slot_of(pane));
    let sync = use_layout_sync();
    let mut sync = sync.borrow_mut();
    sync.seen = snapshot.clone();
    sync.settling = None;
}

/// Each divider's share in `wire`, by id — what [`adopt_layout`] diffs the incoming
/// arrangement against so it can drive only the ones the host moved.
fn ratios_of(wire: &LayoutWire) -> std::collections::HashMap<SplitId, f32> {
    fn walk(node: &LayoutNodeWire, out: &mut std::collections::HashMap<SplitId, f32>) {
        if let LayoutNodeWire::Split {
            id,
            ratio,
            first,
            second,
            ..
        } = node
        {
            if let Some(id) = id {
                out.insert(*id, *ratio);
            }
            walk(first, out);
            walk(second, out);
        }
    }
    let mut out = std::collections::HashMap::new();
    if let Some(root) = &wire.root {
        walk(root, &mut out);
    }
    out
}

/// Drive the live ratio signal of every divider whose share the host MOVED since `was` — a
/// divider it left alone keeps whatever the user has done to it (see [`adopt_layout`]).
fn force_changed_ratios(node: &LayoutNode, was: &std::collections::HashMap<SplitId, f32>) {
    if let LayoutNode::Split {
        id,
        ratio,
        first,
        second,
        ..
    } = node
    {
        // A divider we have not seen before (freshly minted) has no signal yet; one whose
        // share is unchanged must not be touched, or an unrelated adopt yanks a live drag.
        if was
            .get(id)
            .is_none_or(|before| before.to_bits() != ratio.to_bits())
        {
            use_split_ratio(split_tag(*id), *ratio).set(*ratio);
        }
        force_changed_ratios(first, was);
        force_changed_ratios(second, was);
    }
}

/// Keep this client's dock surface and the host's arrangement in step — the write half of
/// the arc, run once per frame from the pre-view reconcile hook.
///
/// Two directions, and never both at once:
///
/// * the HOST moved (another attached client's gesture, a plugin's spawn, a float): its
///   revision differs from what we last saw, so we re-project. The host wins outright — it
///   is the authority, and a client that argued would fork the tree, which is precisely what
///   R148 found and this retires.
/// * WE moved (the user dragged a divider or re-docked a pane): the surface no longer
///   matches what the host holds, so we write it — but only once it has STOPPED changing.
///   A drag moves every frame; writing each one would put a synchronous socket round trip on
///   the UI thread sixty times a second to record intermediate states nobody asked to keep.
///   Waiting for one still frame costs a frame of latency on a gesture that has already
///   ended, and the user cannot perceive it.
///
/// The write's ANSWER is adopted directly (it carries the canonical ids), so a gesture costs
/// exactly one round trip and converges: re-projecting it yields the same surface, whose
/// un-projection then equals what the host holds, so nothing is written again.
pub(crate) fn sync_layout(slots: &crate::slotview::SlotView) {
    let host = slots.layout();
    if host.revision != use_layout_sync().borrow().seen.revision {
        adopt_layout(&host, slots);
        return;
    }
    match pending_write(slots) {
        // Nothing legitimate to write (unrepresentable, a foreign pane set, or already the
        // host's): no gesture is in flight, so forget any half-tracked settle.
        None => use_layout_sync().borrow_mut().settling = None,
        Some((current, expected)) => {
            // A REORGANIZE (drag-to-dock) has no explicit commit edge — pinion only mutates
            // the topology signal as the drag moves — so settle it: write once the surface
            // has stopped changing (this frame's un-projection == last frame's), else a drag
            // would put a synchronous socket round trip on the UI thread every frame. A drag
            // that has already ended costs one still frame of latency the user cannot perceive.
            // (A RATIO drag does not come through here — it commits via [`commit_layout`] on
            // the `ratio_committed` intent, which is why removing the idle-repaint livelock did
            // not strand it on a frame that never arrives.)
            if use_layout_sync().borrow().settling.as_ref() != Some(&current) {
                use_layout_sync().borrow_mut().settling = Some(current);
                return;
            }
            adopt_layout(&slots.set_layout(current, expected), slots);
            use_layout_sync().borrow_mut().settling = None;
        }
    }
}

/// Write the settled arrangement to the host NOW, without the [`sync_layout`] settle gate —
/// the RATIO-drag commit path.
///
/// pinion (PR-56) fires a `ratio_committed` intent on splitter drag-end, and the final ratio
/// is already in the divider's [`use_split_ratio`] signal (the live drag set it), so this
/// un-projects that value and writes immediately. It is the explicit commit edge the
/// reorganize path lacks: a ratio release re-sets no signal, so no repaint follows it, so the
/// settle detector's "one still frame" would never arrive once the idle-repaint livelock was
/// fixed. The intent reaches the reducer regardless of painting, so committing here is frame-
/// independent by construction.
///
/// Shares [`pending_write`] with [`sync_layout`] — same membership guard, same
/// compare-and-set — so a ratio commit cannot write a lossy or foreign arrangement any more
/// than a settled reorganize can.
pub(crate) fn commit_layout(slots: &crate::slotview::SlotView) {
    let host = slots.layout();
    if host.revision != use_layout_sync().borrow().seen.revision {
        adopt_layout(&host, slots); // the host moved under the drag; it wins, the drag is dropped
        return;
    }
    if let Some((current, expected)) = pending_write(slots) {
        adopt_layout(&slots.set_layout(current, expected), slots);
    }
}

/// Un-project this client's surface and, when it is a well-formed re-arrangement of the SAME
/// panes the host holds and it differs from the host's, the `(topology, expected_revision)` to
/// write. `None` when there is nothing legitimate to write — the shared decision behind both
/// [`sync_layout`] (which then settle-gates) and [`commit_layout`] (which writes at once).
///
/// `None` covers three cases, none of which a client may promote to session state:
/// * UNREPRESENTABLE — [`unproject_layout`] cannot form a tree (e.g. a tab well).
/// * a FOREIGN pane SET — the surface tiles different panes than the host. A gesture
///   RE-ARRANGES panes; it never adds or removes one. [`project_layout`] is deliberately lossy
///   (a pane past the slot cap or not yet admitted COLLAPSES out), so writing the un-projection
///   would DELETE that pane from the session; the host would re-append it with a fresh divider,
///   which the client re-projects and writes again — a permanent loop that also relocates the
///   pane. Membership is the Workspace's and never the client's to edit, so we render what we
///   can and write nothing until we can represent the whole set.
/// * ALREADY the host's — the surface equals what the host holds, so there is nothing to record.
fn pending_write(slots: &crate::slotview::SlotView) -> Option<(LayoutWire, u64)> {
    let current = unproject_layout(use_dock_topology().get().as_ref(), &|slot| {
        slots.pane_at(slot)
    })?;
    let sync = use_layout_sync();
    if wire_panes(&current) != wire_panes(&sync.borrow().seen.tree) {
        tracing::debug!(
            target: "sprag_gui::split",
            "this client cannot render every tiled pane; not writing (its arrangement is the host's)",
        );
        return None;
    }
    if current == sync.borrow().seen.tree {
        return None;
    }
    let expected = sync.borrow().seen.revision;
    Some((current, expected))
}

/// `Owner::cache` key for the held dock-tree topology Signal.
const TOPOLOGY_KEY: &str = "sprag_gui.dock_topology";

/// The dock-tree topology Signal — the tiled panes' leaves (`None` = nothing tiled).
/// Cached in the root owner, so the view fn, the splitter-External registration, and the
/// reorganizer all resolve the same shared slot. The shell's `view` subscribes it (a `set`
/// repaints) and `reconcile_externals` re-walks it to register a reorganize-minted Split's
/// drag External (pinion R689).
///
/// Starts EMPTY and is filled by [`adopt_layout`] on the first [`sync_layout`], which runs
/// in the pre-view hook — so the first paint already has the host's arrangement, and there
/// is exactly one place the tree is populated. (It was once seeded here, in a factory that
/// fired once and then forked from the host forever: the R148 finding this retires.)
///
/// A gesture mutates it directly — that is what makes the drag feel instant — and
/// [`sync_layout`] writes the settled result back to the host, which owns it.
pub(crate) fn use_dock_topology() -> Rc<Signal<Option<DockTopology>>> {
    Owner::current()
        .expect("use_dock_topology() requires an active Owner scope")
        .cache(TOPOLOGY_KEY, || Signal::new(None))
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

/// The tile indices of the panes currently DOCKED — the dock-tree's leaves, in
/// left-to-right paint order, or empty when nothing is tiled. The SINGLE authority for
/// "which panes does the main window show" — read by BOTH the paint
/// ([`crate::view::view_for_window`]) and the per-window a11y projection
/// ([`crate::a11y::access_nodes_for_window`]), so they can never disagree.
///
/// No float filter: the tree projects the host's arrangement, which tiles only the panes
/// that are not floated, so a floated pane HAS no leaf here. (The filter existed for the
/// retired placeholder model, where the tree held every pane's leaf and membership had to be
/// derived by subtracting the floaters — see the module docs.)
pub(crate) fn docked_pane_indices() -> Vec<usize> {
    use_dock_topology()
        .get()
        .map(|t| {
            t.panel_ids()
                .iter()
                .filter_map(|p| pane_index_of_panel(p))
                .collect()
        })
        .unwrap_or_default()
}

/// TEST FIXTURE: the dock tree `slots` (as pane ids, identity-mapped) arrange into.
///
/// Production no longer builds a boot tree at all — the HOST owns the arrangement and
/// [`use_dock_topology`] PROJECTS it ([`project_layout`]). This mirror exists only so the
/// tree-shaped tests below have a fixture, and it is built by running the REAL host
/// reconcile + projection, so there is exactly ONE tree-shape authority (a second
/// hand-rolled builder is what R124 retired).
#[cfg(test)]
fn boot_tree(slots: &[usize]) -> Option<DockTopology> {
    let mut tree = LayoutTree::new();
    tree.reconcile(&slots.iter().map(|&s| PaneId(s as u64)).collect::<Vec<_>>());
    project_layout(&tree, &|pane| Some(pane.0 as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;
    use pinion_widget_paint::dock::DockNode; // used only by the boot-shape assertion

    #[test]
    fn panel_id_round_trips_through_pane_index_of_panel() {
        for i in 0..MAX_PANES {
            assert_eq!(pane_index_of_panel(&panel_id(i)), Some(i));
            assert_eq!(panel_id(i), format!("terminal-{i}"));
        }
        // Non-panel ids (the splitter tag, a window id, garbage) are not panes.
        assert_eq!(pane_index_of_panel(&split_tag(SplitId(0))), None);
        assert_eq!(pane_index_of_panel("pane-0"), None); // window-id namespace
        assert_eq!(pane_index_of_panel("terminal-"), None); // no index
        assert_eq!(pane_index_of_panel("terminal-x"), None); // non-numeric
        assert_eq!(pane_index_of_panel(&panel_id(MAX_PANES)), None); // out of range
    }

    /// The host's live panes become leaves, and a genuinely empty arrangement stays empty.
    ///
    /// (R151: this was named `..._falls_back_to_a_local_arrangement` and documented a
    /// `WireHost`-returns-nothing fallback that R149 DELETED — `WireHost::layout` now serves
    /// the last-known mirror and boot fails hard instead. The body never exercised a fallback
    /// even then, so the name guarded nothing. Renamed to what it actually proves.)
    #[test]
    fn project_layout_maps_live_panes_to_leaves_and_keeps_the_zero_pane_edge() {
        let mut local = LayoutTree::new();
        local.reconcile(&[PaneId(0), PaneId(1)]);
        let projected =
            project_layout(&local, &|pane| Some(pane.0 as usize)).expect("live panes arrange");
        assert_eq!(projected.leaf_count(), 2, "both live panes are rendered");

        // ...whereas a genuinely empty host layout over NO panes stays empty.
        assert!(
            project_layout(&LayoutTree::new(), &|pane: PaneId| Some(pane.0 as usize)).is_none()
        );
    }

    #[test]
    fn boot_topology_is_a_right_nested_horizontal_row() {
        // 0 panes -> no topology.
        assert!(boot_tree(&[]).is_none());

        // 1 pane -> a single leaf, no split.
        let one = boot_tree(&[0]).expect("one pane docks");
        assert_eq!(one.leaf_count(), 1);
        assert_eq!(one.split_count(), 0);
        assert_eq!(one.panel_ids(), vec![panel_id(0)]);

        // 2 panes -> one Horizontal split (== the old row default at 2 panes).
        let two = boot_tree(&[0, 1]).expect("two panes dock");
        assert_eq!(two.leaf_count(), 2);
        assert_eq!(two.split_count(), 1);
        assert_eq!(two.split_ids(), vec![split_tag(SplitId(0))]);
        assert_eq!(two.panel_ids(), vec![panel_id(0), panel_id(1)]);
        let DockNode::Split { orientation, .. } = two.root() else {
            panic!("two-pane root is a Split");
        };
        assert_eq!(*orientation, SplitterOrientation::Horizontal);

        // 4 panes -> three Horizontal dividers, panes in tile order, ids gapless.
        let four = boot_tree(&[0, 1, 2, 3]).expect("four panes dock");
        assert_eq!(four.leaf_count(), 4);
        assert_eq!(four.split_count(), 3);
        assert_eq!(
            four.panel_ids(),
            (0..4).map(panel_id).collect::<Vec<_>>(),
            "panes stay in left-to-right tile order"
        );
        assert_eq!(
            four.split_ids(),
            (0..3).map(|k| split_tag(SplitId(k))).collect::<Vec<_>>(),
            "boot dividers are ids 0..n-1 in pre-order"
        );

        // A HOLE (slot 1 empty — an attach-race pane that failed its first fetch, R121):
        // leaves exist for EXACTLY the renderable panes (0, 2, 3), never orphaning the
        // pane at slot 3 nor painting a blank leaf at slot 1 (the F6 silent-orphan fix,
        // now upheld by `project_layout` collapsing an unmappable leaf into its sibling).
        let holed = boot_tree(&[0, 2, 3]).expect("three occupied slots dock");
        assert_eq!(
            holed.panel_ids(),
            vec![panel_id(0), panel_id(2), panel_id(3)],
            "leaves are exactly the occupied slots; the hole at slot 1 has none"
        );
        assert_eq!(holed.leaf_count(), 3);
        // Divider ids are the HOST's split sequence (0, 1) — NOT the left leaf's slot
        // (which would be 0, 2). The scheme changed with the arrangement moving host-side,
        // and sequence ids are the stronger identity: a slot-keyed id re-binds a divider's
        // drag state when panes reshuffle, while the host's SplitId follows its boundary
        // for life. Contiguous boots are unaffected (slot k == sequence k), so only a
        // holed set like this one observes the difference.
        assert_eq!(
            holed.split_ids(),
            vec![split_tag(SplitId(0)), split_tag(SplitId(1))],
        );
    }

    /// The even share a divider opens at — the host's `RATIO_DEFAULT`, restated here only
    /// as a test expectation (production reads the share off the host's arrangement).
    const RATIO_EVEN: f32 = 0.5;

    /// A tree the host served, projected and un-projected, must come back IDENTICAL — the
    /// write path's core promise. If the round trip drifted, every settled gesture would
    /// write a subtly different arrangement than the one on screen, and (worse) the drift
    /// would differ from the host's, so `sync_layout` would write on every frame forever.
    #[test]
    fn unprojecting_a_projected_arrangement_returns_it_unchanged() {
        let owner = Owner::new();
        owner.run(|| {
            let mut tree = LayoutTree::new();
            tree.reconcile(&[PaneId(0), PaneId(1), PaneId(2)]);
            let wire = LayoutWire::from(&tree);

            let topo = project_layout(&tree, &|p| Some(p.0 as usize)).expect("panes arrange");
            let back = unproject_layout(Some(&topo), &|slot| Some(PaneId(slot as u64)))
                .expect("a projected surface is representable");

            assert_eq!(back, wire, "project -> unproject is the identity");
        });
    }

    /// The hinge of the write path: a divider PINION minted while resolving a drop has no
    /// `SplitId`, so it must un-project as `id: None` and let the host name it. Reading a
    /// number out of `reorg-split-0` (or dropping the divider) would either collide with a
    /// real id or silently lose the user's split.
    #[test]
    fn a_client_minted_divider_unprojects_unnamed_for_the_host_to_name() {
        let owner = Owner::new();
        owner.run(|| {
            let topo = DockTopology::new(DockNode::Split {
                id: Cow::Borrowed("reorg-split-0"), // pinion's own, not the host's
                orientation: SplitterOrientation::Vertical,
                ratio: 0.5,
                first: Box::new(DockNode::leaf(panel_id(0))),
                second: Box::new(DockNode::leaf(panel_id(1))),
            });
            let wire = unproject_layout(Some(&topo), &|slot| Some(PaneId(slot as u64)))
                .expect("representable");
            let Some(LayoutNodeWire::Split { id, dir, .. }) = wire.root else {
                panic!("the root is a split, got {:?}", wire.root);
            };
            assert_eq!(id, None, "a client-minted divider goes to the host UNNAMED");
            assert_eq!(
                dir,
                SplitDir::Vertical,
                "its direction is the user's intent"
            );

            // ...whereas one the host DID name keeps its identity across the round trip.
            assert_eq!(split_id_of_tag(&split_tag(SplitId(7))), Some(SplitId(7)));
            assert_eq!(split_id_of_tag("reorg-split-0"), None);
            assert_eq!(split_id_of_tag("sprag_gui.split.x"), None);
        });
    }

    /// What gets written is the ratio the user DRAGGED, not the one the divider opened at.
    ///
    /// pinion's `DockNode::Split.ratio` is only the initial value — a live drag lives in the
    /// per-split signal — so un-projecting the node would persist the old boundary and throw
    /// away the exact thing the gesture expressed.
    #[test]
    fn unproject_writes_the_dragged_ratio_not_the_one_the_divider_opened_at() {
        let owner = Owner::new();
        owner.run(|| {
            let mut tree = LayoutTree::new();
            tree.reconcile(&[PaneId(0), PaneId(1)]);
            let topo = project_layout(&tree, &|p| Some(p.0 as usize)).expect("panes arrange");

            // The user drags the divider: the SIGNAL moves, the node still says 0.5.
            use_split_ratio(split_tag(SplitId(0)), RATIO_EVEN).set(0.82);

            let wire = unproject_layout(Some(&topo), &|slot| Some(PaneId(slot as u64)))
                .expect("representable");
            let Some(LayoutNodeWire::Split { ratio, .. }) = wire.root else {
                panic!("the root is a split");
            };
            assert!(
                (ratio - 0.82).abs() < f32::EPSILON,
                "the DRAGGED share is what reaches the host, got {ratio}",
            );
        });
    }

    /// A surface the host's arrangement cannot express is REFUSED, not written lossily.
    ///
    /// A pinion tab well is the only such shape, and it is unreachable (this surface runs
    /// `with_tabbing(false)`) — but it is a real variant of pinion's type, so an honest
    /// `None` beats an assumption. Writing the panes minus their well would silently destroy
    /// the arrangement the user is looking at.
    #[test]
    fn an_unrepresentable_surface_refuses_the_write() {
        let owner = Owner::new();
        owner.run(|| {
            let tabs = DockTopology::new(DockNode::tabs(
                "well-0",
                vec![Cow::Owned(panel_id(0)), Cow::Owned(panel_id(1))],
                0,
            ));
            assert_eq!(
                unproject_layout(Some(&tabs), &|slot| Some(PaneId(slot as u64))),
                None,
                "a tab well is refused rather than written lossily",
            );
            // The zero-pane surface, by contrast, is a legal arrangement.
            assert_eq!(
                unproject_layout(None, &|slot| Some(PaneId(slot as u64))),
                Some(LayoutWire { root: None }),
            );
        });
    }

    /// A pane this client cannot RENDER must never be deleted from the SESSION's
    /// arrangement — the authority inversion this whole arc exists to prevent.
    ///
    /// [`project_layout`] is lossy on purpose: a pane with no slot (past `MAX_PANES`, or not
    /// yet admitted) collapses out of the painted surface. Feeding that surface back through
    /// [`unproject_layout`] yields a tree MISSING the pane — and R150's `sync_layout` would
    /// have written it, whereupon the host re-appends the pane at the end with a fresh
    /// divider, this client re-projects, drops it again, and writes again: a permanent write
    /// loop on the UI thread that also relocates the user's pane for good, durably. Found by
    /// the R151 3-lens review; missed here because the first round-trip test used a TOTAL
    /// `slot_of`, so it could not see the case that matters.
    #[test]
    fn a_surface_missing_a_pane_the_client_cannot_render_is_never_written() {
        let owner = Owner::new();
        owner.run(|| {
            let mut host = LayoutTree::new();
            host.reconcile(&[PaneId(0), PaneId(1), PaneId(2)]);
            let served = LayoutWire::from(&host);

            // Pane 2 is real and tiled host-side, but this client has no slot for it.
            let topo = project_layout(&host, &|p| (p.0 < 2).then_some(p.0 as usize))
                .expect("the renderable panes still paint");
            assert_eq!(
                topo.leaf_count(),
                2,
                "the surface honestly shows what it can"
            );

            let would_write = unproject_layout(Some(&topo), &|slot| Some(PaneId(slot as u64)))
                .expect("representable");
            assert_ne!(
                wire_panes(&would_write),
                wire_panes(&served),
                "the un-projected surface is MISSING pane 2 (this is the trap)",
            );
            // ...so the guard must refuse it: membership is the Workspace's, never ours.
            assert!(
                wire_panes(&would_write) != wire_panes(&served),
                "sync_layout compares exactly this and declines to write",
            );

            // And the honest case still writes: same panes, different arrangement.
            let full = project_layout(&host, &|p| Some(p.0 as usize)).expect("all render");
            let rearranged = unproject_layout(Some(&full), &|slot| Some(PaneId(slot as u64)))
                .expect("representable");
            assert_eq!(
                wire_panes(&rearranged),
                wire_panes(&served),
                "a real gesture re-arranges the SAME panes, so it passes the guard",
            );
        });
    }

    /// A retired divider's ratio signal is EVICTED — the long session this arc exists for
    /// must not leak one per gesture.
    ///
    /// `SplitId`s are never reused and pinion mints a transient `reorg-split-{n}` per drop,
    /// so every reorganize used to strand two entries in a cache that never evicts.
    #[test]
    fn a_retired_dividers_ratio_is_evicted_on_adopt() {
        let owner = Owner::new();
        owner.run(|| {
            let live = |tag: &str| use_split_ratios().borrow().contains_key(tag);

            // A divider the host knows, and one pinion minted mid-gesture.
            use_split_ratio(split_tag(SplitId(0)), 0.5).set(0.7);
            use_split_ratio(Cow::Borrowed("reorg-split-0"), 0.5);
            assert!(live(&split_tag(SplitId(0))) && live("reorg-split-0"));

            // The adopted arrangement holds ONLY divider 0 — the transient is gone for good.
            let mut tree = LayoutTree::new();
            tree.reconcile(&[PaneId(0), PaneId(1)]);
            let topo = project_layout(&tree, &|p| Some(p.0 as usize));
            prune_split_ratios(topo.as_ref());

            assert!(
                live(&split_tag(SplitId(0))),
                "a live divider keeps its ratio"
            );
            assert!(!live("reorg-split-0"), "the transient divider is evicted");
            assert!(
                (use_split_ratio(split_tag(SplitId(0)), 0.5).get() - 0.7).abs() < f32::EPSILON,
                "and the surviving divider kept the user's dragged share",
            );

            // Every divider retired (the split collapsed): nothing is left behind.
            prune_split_ratios(None);
            assert!(use_split_ratios().borrow().is_empty());
        });
    }

    #[test]
    fn use_split_ratio_is_per_id_memoised_and_seeds_default() {
        let owner = Owner::new();
        owner.run(|| {
            let a = use_split_ratio(split_tag(SplitId(0)), RATIO_EVEN);
            let b = use_split_ratio(split_tag(SplitId(0)), RATIO_EVEN);
            assert!(Rc::ptr_eq(&a, &b), "memoised by Split id");
            assert!((a.get() - RATIO_EVEN).abs() < f32::EPSILON);
            a.set(0.7);
            assert!(
                (use_split_ratio(split_tag(SplitId(0)), RATIO_EVEN).get() - 0.7).abs()
                    < f32::EPSILON,
                "drag re-weights; the seed does not reset an existing slot"
            );
            assert!(
                (use_split_ratio(split_tag(SplitId(1)), RATIO_EVEN).get() - RATIO_EVEN).abs()
                    < f32::EPSILON,
                "a different divider is independent"
            );
        });
    }

    /// (R136, consuming PINION-PR54 / pinion R1338) In a 2-PANE dock, an outer
    /// full-span dock is REDUNDANT at every edge: removing either pane leaves a single
    /// leaf, so the outer band is `Split[dragged | lone]` — structurally identical to an
    /// INNER split of that lone pane (only the thin `OUTER_DOCK_NEW_FRAC` ratio differs).
    /// pinion now reports it redundant, so `resolve_drop` snaps back instead of docking a
    /// worse-ratio full-span strip, and the user's top/bottom EDGE drop yields the inner
    /// 50/50 split (the "왜 절반이 아니라 끝에 붙나" complaint). At >=3 panes the outer band
    /// spans multiple REMAINING panes an inner split cannot reproduce, so it stays offered.
    ///
    /// sprag owns this integration claim (its `panel_id` scheme + `tabbing=false`
    /// reorganizer over the boot topology); pinion owns the redundancy logic + the pointer
    /// drag that consumes it. sprag has NO code for the gesture (a same-window docked drag
    /// is resolved inside pinion's `DockPanelExternal`), so this pins the behaviour sprag
    /// gets for free and guards against a pinion regression reaching it.
    #[test]
    fn two_pane_outer_dock_is_redundant_but_three_pane_stays_offered() {
        use pinion_widget_paint::dock::DockDropZone;

        // Two panes: outer dock at EVERY edge duplicates a plain split -> redundant.
        let owner = Owner::new();
        owner.run(|| {
            use_dock_topology().set(boot_tree(&[0, 1]));
            let reorg = use_dock_reorganizer();
            for zone in [
                DockDropZone::Top,
                DockDropZone::Bottom,
                DockDropZone::Left,
                DockDropZone::Right,
            ] {
                assert!(
                    reorg.outer_dock_is_redundant(&panel_id(1), zone),
                    "2-pane outer {zone:?} duplicates a plain split -> must snap back (PR-54)",
                );
            }
        });

        // Three panes: an outer TOP puts the dragged pane above the two REMAINING panes at
        // once -> a full-span row no single inner split reaches -> genuinely offered.
        let owner = Owner::new();
        owner.run(|| {
            use_dock_topology().set(boot_tree(&[0, 1, 2]));
            let reorg = use_dock_reorganizer();
            assert!(
                !reorg.outer_dock_is_redundant(&panel_id(2), DockDropZone::Top),
                "3-pane outer Top spans the other two panes -> a distinct full-span row",
            );
        });
    }
}
