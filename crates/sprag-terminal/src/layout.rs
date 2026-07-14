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
//! ## v1 bound: this is a SEED, not yet the authority (read this before trusting it)
//!
//! Only the READ half is built (host → wire → client projection). There is no
//! client→host write path: no mux action or `intervene` slot mutates the arrangement, and
//! [`LayoutTree::append_pane`] / [`remove_pane`](LayoutTree::remove_pane) are reached
//! only from [`reconcile`](LayoutTree::reconcile). So today this tree carries NO
//! information the pane list does not already have — `dir` is always
//! `SplitDir::Horizontal` and `ratio` always `RATIO_DEFAULT`, because nothing writes
//! them. The client seeds its dock tree from this ONCE and then forks: every subsequent
//! split / drag / reorganize lives only in the client, and a reattach would re-derive the
//! default even row rather than restore the user's layout. Until the write path lands,
//! the CLIENT is the arrangement authority and this is its boot seed.
//!
//! ## Membership is the Workspace's, arrangement is ours (reconcile, don't co-mutate)
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
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutNode {
    /// A pane occupies this cell, addressed by its registry-global [`PaneId`].
    Leaf(PaneId),
    /// A division of two sub-trees at `ratio` (the `first` child's share, `0.0..=1.0`).
    ///
    /// `ratio` is the share a client OPENS the divider at.
    ///
    /// **v1 bound:** it is not yet durable. A live drag lives in the client's own
    /// per-split signal, and no write-back path exists, so a dragged ratio does NOT
    /// survive a detach today — the arrangement served here is still whatever
    /// [`LayoutTree::reconcile`] derives from the pane set (always `RATIO_DEFAULT`).
    /// Making this the durable share is what the client→host write path buys.
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

/// A window's logical layout tree: how its panes are arranged, and nothing about pixels.
///
/// Empty (`root == None`) means the window has no panes — the honest zero-pane state,
/// not an error.
#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
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

        let json = serde_json::to_string(&tree).expect("the layout serializes");
        let back: LayoutTree = serde_json::from_str(&json).expect("the layout round-trips");
        assert_eq!(back, tree, "serde is lossless for the whole arrangement");
        assert_eq!(back.panes(), ids(3));

        // A restored tree must keep minting FRESH split ids (next_split survived), or a
        // reattached client's next split would collide with an existing divider.
        let mut back = back;
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

    #[test]
    fn reconcile_is_idempotent() {
        let mut tree = LayoutTree::new();
        tree.reconcile(&ids(3));
        let once = tree.clone();
        tree.reconcile(&ids(3));
        assert_eq!(tree, once, "reconciling an unchanged set changes nothing");
    }
}
