//! A window's LOGICAL layout — which panes are arranged how, pinion-free.
//!
//! ## Why this lives in the producer (superseding the Round 7 note)
//!
//! [`workspace`](crate::workspace)'s Round 7 design note said there is no split tree
//! here because a tree "only has meaning relative to a display surface to divide, so it
//! is a rendering concern". That conflated two different things:
//!
//! * **Pixel geometry** (what rect each pane occupies at this client's size) — genuinely
//!   a rendering concern, and it stays in the display client. Nothing here has a rect.
//! * **Logical arrangement** (which panes are split, in what order, at what proportion)
//!   — genuinely SESSION state. tmux keeps it server-side precisely so a client can
//!   detach and reattach — possibly at a different size, from a different machine — and
//!   get its layout back. If it lived only in the client, detaching would destroy it.
//!
//! The detach/reattach arc needs the second one to outlive any client, so the logical
//! arrangement belongs here and the Round 7 note is superseded for it (never for pixels).
//! This type is deliberately pinion-free: the display client PROJECTS it into pinion's
//! `DockTopology` / Scene, which keeps "pixels are pinion's" intact.
//!
//! ## The client→host write path (what makes this the authority)
//!
//! A display client PROJECTS this tree onto its own surface, resolves a gesture there (a
//! divider drag, a drag-to-dock reorganize), and writes the SETTLED arrangement back
//! through [`LayoutTree::set_from_wire`]. That write is what puts the user's intent into
//! session state, so it outlives the client that expressed it. Without it this tree would
//! carry no information the pane list does not already have (`dir` always
//! [`SplitDir::Horizontal`], `ratio` always the even default, because only
//! [`reconcile`](LayoutTree::reconcile) would ever build it) and a reattach would
//! re-derive a default even row instead of restoring the user's layout.
//!
//! **The host is the ID AUTHORITY.** A client resolves a gesture on its own surface and
//! may mint a divider the host has never seen; it sends that one as `id: None`
//! ([`LayoutNodeWire`]) and the host stamps a fresh [`SplitId`] on it. The client then
//! re-reads the canonical tree, so every divider it keys per-split state on has a durable
//! identity — and there is exactly ONE minting site, here. A write is VALIDATED
//! ([`LayoutError`]): a client cannot install a tree with a duplicate divider, the same
//! pane in two places, or a nonsense ratio.
//!
//! ## Membership is the Workspace's, float is the Window's, arrangement is ours
//!
//! This tree arranges the window's DOCKED panes. A pane a client has floated out into its
//! own OS window is not tiled, so it holds no leaf here: [`Window`](crate::Window) owns
//! that set and reconciles this tree over `panes − floating`. The seam is the same one the
//! module rests on — WHICH panes are tiled is logical (session state, so a reattaching
//! client floats them back out), WHERE a floating window sits on screen is pixels (the
//! client's, and never here).
//!
//! ## Reconcile, don't co-mutate
//!
//! Pane lifecycle runs through [`Workspace`](crate::Workspace) directly — the control
//! and plugin surfaces hold `Arc<Mutex<Workspace>>` and spawn/close on it without ever
//! seeing a [`Window`](crate::Window). So this tree must never be the membership
//! authority: it would silently drift the moment a plugin spawned a pane. Instead it is
//! an ARRANGEMENT that self-heals against the pane set via [`LayoutTree::reconcile`] —
//! a new pane appears, a closed pane's leaf collapses into its sibling. That keeps one
//! membership SSOT (the workspace) and makes this layer robust to every spawn path,
//! present and future.
//!
//! [`reconcile`](LayoutTree::reconcile) is PURE (it takes the pane list, holds no lock),
//! so a caller resolves the workspace's panes first and then reconciles — never holding
//! the registry lock across the workspace lock.

use std::collections::HashSet;

use crate::workspace::PaneId;

/// Which way a [`LayoutNode::Split`] divides its two children.
///
/// `Horizontal` lays `first` LEFT and `second` RIGHT; `Vertical` lays `first` TOP and
/// `second` BOTTOM (pinion `DockNode::Split`'s convention, so the client's projection is
/// a direct mapping).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

/// A split's stable identity, minted per [`LayoutTree`] and never reused.
///
/// Stable across mutations so a client can key its per-split state (the live drag ratio)
/// on it rather than on traversal order — the same reason pinion's `DockNode::Split`
/// carries a stable id. The client formats this into its own id string; the number is
/// the durable part.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct SplitId(pub u64);

/// The even divider share a freshly-minted split opens at (matching the client's
/// historical even boot tiling, so lifting the layout here changed no visible default).
const RATIO_DEFAULT: f32 = 0.5;

/// One node of a window's logical layout: either a pane, or a division of two sub-trees.
///
/// Not the wire type — [`LayoutNodeWire`] is (this one's `id` is always present, which is
/// exactly the invariant a write establishes).
#[derive(Clone, PartialEq, Debug)]
pub enum LayoutNode {
    /// A pane occupies this cell, addressed by its registry-global [`PaneId`].
    Leaf(PaneId),
    /// A division of two sub-trees at `ratio` (the `first` child's share, `0.0..=1.0`).
    ///
    /// The DURABLE share: a client seeds its divider from this, and writes the settled
    /// value back when the user finishes dragging ([`LayoutTree::set_from_wire`]), so the
    /// boundary the user chose survives a detach. A freshly-minted divider opens at an
    /// even share. The LIVE value mid-drag lives in the client's own per-split
    /// state — a drag is a gesture, not session state, until it settles.
    Split {
        id: SplitId,
        dir: SplitDir,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// This sub-tree's panes, left-to-right / top-to-bottom (paint order).
    fn panes_into(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(pane) => out.push(*pane),
            Self::Split { first, second, .. } => {
                first.panes_into(out);
                second.panes_into(out);
            }
        }
    }

    /// Append `pane` at the RIGHTMOST position, preserving the right-nested row shape
    /// (divider `k` separates pane `k` from everything to its right) the client's boot
    /// tree has always had — so an appended pane lands where a terminal user expects.
    fn append(self, pane: PaneId, mint: &mut impl FnMut() -> SplitId) -> Self {
        match self {
            leaf @ Self::Leaf(_) => Self::Split {
                id: mint(),
                dir: SplitDir::Horizontal,
                ratio: RATIO_DEFAULT,
                first: Box::new(leaf),
                second: Box::new(Self::Leaf(pane)),
            },
            Self::Split {
                id,
                dir,
                ratio,
                first,
                second,
            } => Self::Split {
                id,
                dir,
                ratio,
                first,
                // Descend the right spine so the new leaf lands at the far right.
                second: Box::new(second.append(pane, mint)),
            },
        }
    }

    /// Drop `pane`'s leaf, COLLAPSING its parent split into the surviving sibling (the
    /// sibling reclaims the space — the terminal-multiplexer fill). `None` if this whole
    /// sub-tree was just that pane.
    fn remove(self, pane: PaneId) -> Option<Self> {
        match self {
            Self::Leaf(p) if p == pane => None,
            leaf @ Self::Leaf(_) => Some(leaf),
            Self::Split {
                id,
                dir,
                ratio,
                first,
                second,
            } => match (first.remove(pane), second.remove(pane)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    id,
                    dir,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                // One side went away: the split has nothing left to divide, so the
                // survivor takes its place (this split's id + ratio retire with it).
                (Some(alone), None) | (None, Some(alone)) => Some(alone),
                (None, None) => None,
            },
        }
    }
}

/// A window's logical layout tree: how its DOCKED panes are arranged, and nothing about
/// pixels.
///
/// Empty (`root == None`) means the window tiles no panes — the honest zero-pane state,
/// not an error (every pane floated is the other way to reach it).
///
/// Deliberately NOT serde-derived: its split-id minting counter is internal state, and putting it on the wire would invite a client to drive this type's identity
/// allocation. [`LayoutWire`] is the wire form, and it carries no counter — a read DERIVES
/// it, a write RECOMPUTES it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LayoutTree {
    root: Option<LayoutNode>,
    next_split: u64,
}

impl LayoutTree {
    /// An empty layout (no panes).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The arrangement's root, or `None` when the window holds no panes.
    #[must_use]
    pub fn root(&self) -> Option<&LayoutNode> {
        self.root.as_ref()
    }

    /// The arranged panes in paint order (left-to-right / top-to-bottom).
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.panes_into(&mut out);
        }
        out
    }

    /// Mint the next never-reused split id.
    fn mint(next: &mut u64) -> SplitId {
        let id = SplitId(*next);
        *next += 1;
        id
    }

    /// Arrange `pane` at the rightmost position. A no-op if it is already arranged.
    pub fn append_pane(&mut self, pane: PaneId) {
        if self.panes().contains(&pane) {
            return;
        }
        let next = &mut self.next_split;
        self.root = Some(match self.root.take() {
            None => LayoutNode::Leaf(pane),
            Some(root) => root.append(pane, &mut || Self::mint(next)),
        });
    }

    /// Drop `pane`'s leaf; its sibling reclaims the space. A no-op if it is not arranged.
    pub fn remove_pane(&mut self, pane: PaneId) {
        if let Some(root) = self.root.take() {
            self.root = root.remove(pane);
        }
    }

    /// Self-heal this arrangement against the window's live pane set: drop the leaves of
    /// panes that are gone (siblings reclaim), then arrange any pane not yet placed, in
    /// `panes` order. Panes already arranged keep their exact position + ratios.
    ///
    /// This is how the tree stays true without being the membership authority — the
    /// workspace is (see the module docs). PURE: it holds no lock, so a caller reads the
    /// pane ids first and reconciles after, never nesting the registry lock inside the
    /// workspace lock.
    pub fn reconcile(&mut self, panes: &[PaneId]) {
        let live: HashSet<PaneId> = panes.iter().copied().collect();
        for gone in self.panes().into_iter().filter(|p| !live.contains(p)) {
            self.remove_pane(gone);
        }
        let arranged: HashSet<PaneId> = self.panes().into_iter().collect();
        for pane in panes.iter().filter(|p| !arranged.contains(p)) {
            self.append_pane(*pane);
        }
    }

    /// Replace this arrangement with a client's `wire` one, stamping a fresh [`SplitId`]
    /// on every divider the client minted itself — the WRITE half of the arc (module docs).
    ///
    /// Ids the client did supply are honored, so a divider keeps its identity — and with
    /// it the client's per-split state — across the round trip. Minting resumes above BOTH
    /// this tree's own high-water mark and every id in the incoming tree, so a stamped id
    /// can collide neither with a divider still on screen nor with one just retired.
    ///
    /// VALIDATED, not trusted: this is the only place structure a client authored enters
    /// this type. On an error the tree is left EXACTLY as it was — a rejected write cannot
    /// half-apply, so a client that sends nonsense keeps the arrangement it had rather than
    /// corrupting the session's.
    ///
    /// # Errors
    ///
    /// [`LayoutError`] if the arrangement is not well-formed.
    pub fn set_from_wire(&mut self, wire: LayoutWire) -> Result<(), LayoutError> {
        let Some(root) = wire.root else {
            self.root = None; // the honest zero-pane write (every pane closed or floated)
            return Ok(());
        };
        let mut panes = HashSet::new();
        let mut ids = HashSet::new();
        validate(&root, &mut panes, &mut ids)?;
        // Mint above every id the client kept AS WELL AS our own mark: the client may have
        // dropped dividers (freeing ids we must still never reissue) and kept others.
        let mut next = self
            .next_split
            .max(ids.iter().map(|id| id.0 + 1).max().unwrap_or(0));
        self.root = Some(adopt(root, &mut next));
        self.next_split = next;
        Ok(())
    }
}

/// Why a client's arrangement write was REJECTED ([`LayoutTree::set_from_wire`]).
///
/// Each variant names an invariant [`LayoutTree`] holds that a client could break: a pane
/// is in exactly one place, a divider's identity is its own, and a ratio is a real share.
/// A client that broke one has a bug the host should not absorb silently — a wrong-but-
/// plausible arrangement stored as session state would outlive the client that authored it.
#[derive(Clone, PartialEq, Debug)]
pub enum LayoutError {
    /// The same pane holds two leaves — it cannot be in two places at once, and the
    /// duplicate would make [`reconcile`](LayoutTree::reconcile) unstable.
    DuplicatePane(PaneId),
    /// Two dividers claim one id. Ids key the client's per-split state, so a duplicate
    /// would silently weld two boundaries' drags together.
    DuplicateSplitId(SplitId),
    /// A divider's ratio is not a share in `0.0..=1.0` (out of range, infinite, or NaN).
    /// A NaN would also make the tree's `PartialEq` lie about its own equality.
    InvalidRatio(f32),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicatePane(pane) => write!(f, "pane {pane} is arranged twice"),
            Self::DuplicateSplitId(id) => write!(f, "split id {} is claimed twice", id.0),
            Self::InvalidRatio(ratio) => write!(f, "split ratio {ratio} is not a 0..=1 share"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Check `node` is a well-formed arrangement, accumulating the panes and divider ids seen
/// so far so the checks are whole-tree (a duplicate is only visible across sub-trees).
///
/// Recursion is bounded by the caller: the wire form reaches the host through `serde_json`,
/// whose parser rejects nesting past its own recursion limit long before this could
/// overflow a stack.
fn validate(
    node: &LayoutNodeWire,
    panes: &mut HashSet<PaneId>,
    ids: &mut HashSet<SplitId>,
) -> Result<(), LayoutError> {
    match node {
        LayoutNodeWire::Leaf(pane) => {
            if panes.insert(*pane) {
                Ok(())
            } else {
                Err(LayoutError::DuplicatePane(*pane))
            }
        }
        LayoutNodeWire::Split {
            id,
            ratio,
            first,
            second,
            ..
        } => {
            if let Some(id) = id
                && !ids.insert(*id)
            {
                return Err(LayoutError::DuplicateSplitId(*id));
            }
            if !ratio.is_finite() || !(0.0..=1.0).contains(ratio) {
                return Err(LayoutError::InvalidRatio(*ratio));
            }
            validate(first, panes, ids)?;
            validate(second, panes, ids)
        }
    }
}

/// Adopt a VALIDATED `node` into the tree's own form, stamping an id minted from `next` on
/// each divider the client did not name. Split out from [`validate`] so the tree is only
/// touched once the whole arrangement is known good (the all-or-nothing write).
fn adopt(node: LayoutNodeWire, next: &mut u64) -> LayoutNode {
    match node {
        LayoutNodeWire::Leaf(pane) => LayoutNode::Leaf(pane),
        LayoutNodeWire::Split {
            id,
            dir,
            ratio,
            first,
            second,
        } => LayoutNode::Split {
            id: id.unwrap_or_else(|| LayoutTree::mint(next)),
            dir,
            ratio,
            first: Box::new(adopt(*first, next)),
            second: Box::new(adopt(*second, next)),
        },
    }
}

/// The wire form of a window's arrangement — the ONE shape that crosses the socket, in
/// BOTH directions: a client reads it to project, and writes it back to record a gesture.
///
/// A DTO rather than [`LayoutTree`] itself, for two reasons the read half alone never
/// exposed. The tree's split-id minting counter is internal state a client must not drive
/// (and a `next_split` on the wire would invite exactly that). And a client legitimately
/// arrives holding a divider the host has never seen — see [`LayoutNodeWire::Split::id`].
/// Same convention as `sprag-host`'s `PaneScrollFacts`: one serde-derived definition, so
/// the two ends cannot drift on a field name.
#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LayoutWire {
    /// The arrangement's root, or `None` when the window tiles no panes.
    pub root: Option<LayoutNodeWire>,
}

/// One node of an arrangement in transit — the wire twin of [`LayoutNode`] (see
/// [`LayoutWire`]).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutNodeWire {
    /// A pane occupies this cell, by its registry-global [`PaneId`].
    Leaf(PaneId),
    /// A division of two sub-trees at `ratio` (the `first` child's share).
    Split {
        /// The divider's durable identity — or `None` for one the CLIENT minted while
        /// resolving a gesture on its own surface, which the host has never seen.
        ///
        /// This is why the wire form is not [`LayoutNode`], whose id is never absent. A
        /// client resolves a drag locally (that is what makes it feel instant) and can
        /// invent a divider doing so; but an id must be unique and never reused across the
        /// whole tree's life, which only its owner can promise. So the client says "a new
        /// divider, here" and [`LayoutTree::set_from_wire`] names it.
        #[serde(default)]
        id: Option<SplitId>,
        dir: SplitDir,
        ratio: f32,
        first: Box<LayoutNodeWire>,
        second: Box<LayoutNodeWire>,
    },
}

impl From<&LayoutTree> for LayoutWire {
    fn from(tree: &LayoutTree) -> Self {
        Self {
            root: tree.root().map(LayoutNodeWire::from),
        }
    }
}

impl From<&LayoutNode> for LayoutNodeWire {
    fn from(node: &LayoutNode) -> Self {
        match node {
            LayoutNode::Leaf(pane) => Self::Leaf(*pane),
            LayoutNode::Split {
                id,
                dir,
                ratio,
                first,
                second,
            } => Self::Split {
                // A tree the host owns has named every divider, so a READ never yields
                // `None` — that variant exists strictly for what a client sends back.
                id: Some(*id),
                dir: *dir,
                ratio: *ratio,
                first: Box::new(Self::from(&**first)),
                second: Box::new(Self::from(&**second)),
            },
        }
    }
}

/// A window's arrangement AND the revision it is at — what the host serves for the `layout`
/// read, and what it returns from a write.
///
/// The revision is what lets a client hold a PROJECTION rather than a fork: it re-reads
/// exactly when the number changes, so a host-side change (another client's gesture, a pane
/// spawned by a plugin, a float) reaches it without polling the whole tree every frame.
/// Returning it from a write closes the loop in one round trip: the writer learns the
/// canonical tree — with its client-minted dividers now named — without a second read.
#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LayoutSnapshot {
    /// The revision this arrangement is at (see [`crate::Window::layout_revision`]).
    pub revision: u64,
    /// The arrangement itself.
    pub tree: LayoutWire,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<PaneId> {
        (0..n).map(PaneId).collect()
    }

    /// Every split id in `node`, depth-first.
    fn split_ids(node: &LayoutNode) -> Vec<SplitId> {
        match node {
            LayoutNode::Leaf(_) => Vec::new(),
            LayoutNode::Split {
                id, first, second, ..
            } => {
                let mut out = vec![*id];
                out.extend(split_ids(first));
                out.extend(split_ids(second));
                out
            }
        }
    }

    #[test]
    fn an_empty_layout_has_no_panes() {
        let tree = LayoutTree::new();
        assert!(tree.root().is_none());
        assert!(tree.panes().is_empty());
    }

    #[test]
    fn append_builds_a_right_nested_row_in_order() {
        // The client's historical boot shape: divider k separates pane k from everything
        // to its right, so panes read left-to-right in spawn order.
        let mut tree = LayoutTree::new();
        for pane in ids(3) {
            tree.append_pane(pane);
        }
        assert_eq!(tree.panes(), ids(3));
        match tree.root().unwrap() {
            LayoutNode::Split { first, second, .. } => {
                assert_eq!(**first, LayoutNode::Leaf(PaneId(0)), "leftmost is pane 0");
                assert!(
                    matches!(**second, LayoutNode::Split { .. }),
                    "the row nests to the RIGHT",
                );
            }
            other => panic!("a 3-pane row roots at a Split, got {other:?}"),
        }
    }

    #[test]
    fn a_removed_panes_sibling_reclaims_the_space() {
        let mut tree = LayoutTree::new();
        for pane in ids(2) {
            tree.append_pane(pane);
        }
        tree.remove_pane(PaneId(0));
        // The split collapses: the survivor becomes the root, not a half-empty split.
        assert_eq!(tree.root(), Some(&LayoutNode::Leaf(PaneId(1))));
        tree.remove_pane(PaneId(1));
        assert!(
            tree.root().is_none(),
            "removing the last pane empties the tree"
        );
    }

    #[test]
    fn removing_the_middle_pane_keeps_the_others_ordered() {
        let mut tree = LayoutTree::new();
        for pane in ids(3) {
            tree.append_pane(pane);
        }
        tree.remove_pane(PaneId(1));
        assert_eq!(tree.panes(), vec![PaneId(0), PaneId(2)]);
    }

    #[test]
    fn split_ids_are_unique_and_never_reused() {
        // A client keys its live drag ratio on the split id, so a reused id would
        // silently re-bind a divider's drag state to a different boundary.
        let mut tree = LayoutTree::new();
        for pane in ids(3) {
            tree.append_pane(pane);
        }
        let seen = split_ids(tree.root().unwrap());
        let unique: HashSet<SplitId> = seen.iter().copied().collect();
        assert_eq!(unique.len(), seen.len(), "split ids are unique: {seen:?}");

        // Collapsing a split retires its id; the next append must not reissue it.
        let before = tree.next_split;
        tree.remove_pane(PaneId(2));
        tree.append_pane(PaneId(9));
        assert!(tree.next_split > before, "a fresh split minted a NEW id");
    }

    #[test]
    fn reconcile_adds_new_panes_and_drops_gone_ones() {
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(3));
        assert_eq!(tree.panes(), ids(3), "unarranged panes get placed");

        // Pane 1 closed (e.g. a plugin reaped it straight off the Workspace) and pane 3
        // appeared: the arrangement self-heals against the live set.
        tree.reconcile(&[PaneId(0), PaneId(2), PaneId(3)]);
        assert_eq!(tree.panes(), vec![PaneId(0), PaneId(2), PaneId(3)]);

        tree.reconcile(&[]);
        assert!(
            tree.root().is_none(),
            "an emptied workspace empties the layout"
        );
    }

    #[test]
    fn reconcile_preserves_the_position_and_ratio_of_surviving_panes() {
        // The load-bearing property for detach/reattach: reconciling around a set change
        // must not reshuffle or re-even the panes the user already arranged.
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(2));
        // The user dragged the divider off-centre.
        if let Some(LayoutNode::Split { ratio, .. }) = tree.root.as_mut() {
            *ratio = 0.8;
        }
        let split_id = match tree.root().unwrap() {
            LayoutNode::Split { id, .. } => *id,
            other => panic!("expected a split, got {other:?}"),
        };

        tree.reconcile(&ids(3)); // a third pane appears

        match tree.root().unwrap() {
            LayoutNode::Split { id, ratio, .. } => {
                assert_eq!(*id, split_id, "the existing divider kept its identity");
                assert!(
                    (*ratio - 0.8).abs() < f32::EPSILON,
                    "the user's dragged ratio survived a set change, got {ratio}",
                );
            }
            other => panic!("expected a split, got {other:?}"),
        }
        assert_eq!(tree.panes(), ids(3));
    }

    #[test]
    fn the_layout_round_trips_the_wire_losslessly() {
        // The arc's load-bearing wire claim: a detached session's arrangement reaches a
        // reattaching client EXACTLY — every pane, split identity, direction, and the
        // user's dragged ratio. If serde dropped a field the client would silently
        // restore a DIFFERENT layout, which is the whole thing detach/reattach sells.
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(3));
        if let Some(LayoutNode::Split { ratio, dir, .. }) = tree.root.as_mut() {
            *ratio = 0.73;
            *dir = SplitDir::Vertical;
        }

        let json = serde_json::to_string(&LayoutWire::from(&tree)).expect("the layout serializes");
        let wire: LayoutWire = serde_json::from_str(&json).expect("the layout round-trips");
        let mut back = LayoutTree::new();
        back.set_from_wire(wire).expect("a served layout is valid");
        assert_eq!(
            back.root, tree.root,
            "serde is lossless for the arrangement"
        );
        assert_eq!(back.panes(), ids(3));

        // A restored tree must keep minting FRESH split ids, or a reattached client's next
        // split would collide with a divider already on screen. The counter is NOT on the
        // wire — it is recomputed from the ids that arrived.
        let before: Vec<_> = split_ids(back.root().unwrap());
        back.append_pane(PaneId(9));
        let after = split_ids(back.root().unwrap());
        let fresh: Vec<_> = after.iter().filter(|id| !before.contains(id)).collect();
        assert_eq!(fresh.len(), 1, "exactly one new divider");
        assert!(
            !before.contains(fresh[0]),
            "a restored tree does not reissue a split id",
        );
    }

    /// The write half's core promise: the host NAMES the dividers a client minted itself,
    /// and honors the identity of the ones it already knew.
    #[test]
    fn a_write_stamps_client_minted_dividers_and_keeps_known_ones() {
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(2)); // one divider, id 0

        // The client dragged pane 2 in, minting a divider it cannot name (`id: None`),
        // and kept the divider it already knew (id 0) at a ratio the user dragged.
        tree.set_from_wire(LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: Some(SplitId(0)),
                dir: SplitDir::Horizontal,
                ratio: 0.8,
                first: Box::new(LayoutNodeWire::Leaf(PaneId(0))),
                second: Box::new(LayoutNodeWire::Split {
                    id: None, // the client's own, awaiting a name
                    dir: SplitDir::Vertical,
                    ratio: 0.25,
                    first: Box::new(LayoutNodeWire::Leaf(PaneId(1))),
                    second: Box::new(LayoutNodeWire::Leaf(PaneId(2))),
                }),
            }),
        })
        .expect("a well-formed arrangement installs");

        let seen = split_ids(tree.root().unwrap());
        assert_eq!(seen[0], SplitId(0), "a divider the host knew kept its id");
        assert_ne!(seen[1], SplitId(0), "the client's divider got a FRESH id");
        assert_eq!(seen.len(), 2);
        assert_eq!(tree.panes(), ids(3));

        // The user's intent — direction and dragged ratio — is now session state.
        let LayoutNode::Split { ratio, second, .. } = tree.root().unwrap() else {
            panic!("the root is a split");
        };
        assert!(
            (*ratio - 0.8).abs() < f32::EPSILON,
            "the dragged ratio stuck"
        );
        let LayoutNode::Split { dir, ratio, .. } = &**second else {
            panic!("the client's divider is a split");
        };
        assert_eq!(
            *dir,
            SplitDir::Vertical,
            "a Vertical split is now reachable"
        );
        assert!((*ratio - 0.25).abs() < f32::EPSILON);

        // The next mint clears BOTH high-water marks (the tree's own and what arrived).
        let before = split_ids(tree.root().unwrap());
        tree.append_pane(PaneId(3));
        let fresh: Vec<_> = split_ids(tree.root().unwrap())
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        assert_eq!(fresh.len(), 1);
        assert!(!before.contains(&fresh[0]), "no id is ever reissued");
    }

    /// A stamped id must never collide with one already on screen — including when the
    /// client keeps an id ABOVE this tree's own minting mark (which it can, having read a
    /// tree we later shrank).
    #[test]
    fn a_stamped_id_never_collides_with_one_the_client_kept() {
        let mut tree = LayoutTree::new();
        tree.set_from_wire(LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: Some(SplitId(41)), // far above a fresh tree's mark of 0
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNodeWire::Leaf(PaneId(0))),
                second: Box::new(LayoutNodeWire::Split {
                    id: None,
                    dir: SplitDir::Horizontal,
                    ratio: 0.5,
                    first: Box::new(LayoutNodeWire::Leaf(PaneId(1))),
                    second: Box::new(LayoutNodeWire::Leaf(PaneId(2))),
                }),
            }),
        })
        .expect("a well-formed arrangement installs");

        let seen = split_ids(tree.root().unwrap());
        let unique: HashSet<SplitId> = seen.iter().copied().collect();
        assert_eq!(unique.len(), seen.len(), "ids stay unique: {seen:?}");
        assert!(
            seen[1].0 > 41,
            "minting resumed above the id that arrived, got {:?}",
            seen[1],
        );
    }

    /// A write is the one place a client authors structure, so it is validated — and a
    /// REJECTED write leaves the session's arrangement exactly as it was.
    #[test]
    fn a_malformed_write_is_rejected_whole() {
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(2));
        let good = tree.clone();

        let split = |ratio: f32, first, second| LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: None,
                dir: SplitDir::Horizontal,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
        };

        // The same pane in two places.
        assert_eq!(
            tree.set_from_wire(split(
                0.5,
                LayoutNodeWire::Leaf(PaneId(0)),
                LayoutNodeWire::Leaf(PaneId(0)),
            )),
            Err(LayoutError::DuplicatePane(PaneId(0))),
        );
        // Ratios that are not a share.
        for bad in [1.5, -0.1, f32::NAN, f32::INFINITY] {
            assert!(
                matches!(
                    tree.set_from_wire(split(
                        bad,
                        LayoutNodeWire::Leaf(PaneId(0)),
                        LayoutNodeWire::Leaf(PaneId(1)),
                    )),
                    Err(LayoutError::InvalidRatio(_)),
                ),
                "ratio {bad} is not a 0..=1 share",
            );
        }
        // Two dividers claiming one id (which would weld their drags together).
        assert_eq!(
            tree.set_from_wire(LayoutWire {
                root: Some(LayoutNodeWire::Split {
                    id: Some(SplitId(7)),
                    dir: SplitDir::Horizontal,
                    ratio: 0.5,
                    first: Box::new(LayoutNodeWire::Leaf(PaneId(0))),
                    second: Box::new(LayoutNodeWire::Split {
                        id: Some(SplitId(7)),
                        dir: SplitDir::Horizontal,
                        ratio: 0.5,
                        first: Box::new(LayoutNodeWire::Leaf(PaneId(1))),
                        second: Box::new(LayoutNodeWire::Leaf(PaneId(2))),
                    }),
                }),
            }),
            Err(LayoutError::DuplicateSplitId(SplitId(7))),
        );

        assert_eq!(tree, good, "every rejected write left the tree untouched");
    }

    /// The zero-pane write (the last pane closed, or every pane floated out) is a legal
    /// arrangement, not an error — and it must not reset the minting counter, or the next
    /// divider would reissue a retired id.
    #[test]
    fn an_empty_write_is_legal_and_keeps_the_minting_mark() {
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(3));
        let mark = tree.next_split;
        assert!(mark > 0);

        tree.set_from_wire(LayoutWire { root: None })
            .expect("an empty arrangement is legal");
        assert!(tree.root().is_none());
        assert_eq!(tree.next_split, mark, "a retired id is never reissued");
    }

    #[test]
    fn reconcile_is_idempotent() {
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(3));
        let once = tree.clone();
        tree.reconcile(&ids(3));
        assert_eq!(tree, once, "reconciling an unchanged set changes nothing");
    }
}
