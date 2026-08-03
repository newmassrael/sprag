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

use std::collections::{HashMap, HashSet};

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

impl SplitDir {
    /// The OTHER axis — what a move along this one is measured across.
    fn across(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

/// Which side of its parent [`LayoutNode::Split`] a leaf occupies (`First` = left / top,
/// matching [`SplitDir`]'s convention).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitSide {
    First,
    Second,
}

/// One of the four directions a pane can have a NEIGHBOUR in — tmux `select-pane -L/-R/-U/-D`.
///
/// Not a fifth vocabulary: a direction IS a pair this module already speaks — an axis
/// ([`SplitDir`]) and a side of it ([`SplitSide`]). `Left` is the `First` side of a `Horizontal`
/// division, `Down` the `Second` side of a `Vertical` one. Stating it that way is what lets
/// [`LayoutTree::neighbor`] be a walk over the tree's own shape instead of a second geometry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneDir {
    Left,
    Right,
    Up,
    Down,
}

impl PaneDir {
    /// Every direction, in the order tmux's own flags are conventionally listed (`-L -R -U -D`) —
    /// what a caller asking for a pane's whole neighbourhood iterates.
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];

    /// The axis this direction moves along: left/right divide a ROW (`Horizontal`), up/down a
    /// COLUMN (`Vertical`).
    #[must_use]
    pub fn axis(self) -> SplitDir {
        match self {
            Self::Left | Self::Right => SplitDir::Horizontal,
            Self::Up | Self::Down => SplitDir::Vertical,
        }
    }

    /// Which side of a division on [`axis`](Self::axis) this direction points AT — so a pane has a
    /// neighbour to its left exactly when some ancestor split on the horizontal axis was entered
    /// from its `Second` side.
    #[must_use]
    pub fn side(self) -> SplitSide {
        match self {
            Self::Left | Self::Up => SplitSide::First,
            Self::Right | Self::Down => SplitSide::Second,
        }
    }

    /// Read a direction off the wire (`"left"` / `"right"` / `"up"` / `"down"`), `None` for
    /// anything else.
    ///
    /// The ONE definition of this vocabulary, the way `sprag_detect`'s `AgentState::from_wire` is
    /// for agent states: the wire action, the CLI flags and this walk read the same four words, so
    /// a fifth spelling cannot appear in one of them alone.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        match word {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    /// This direction's wire word — the inverse of [`from_wire`](Self::from_wire), and the key a
    /// neighbourhood is published under.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// A leaf's place in the tiling, stated relative to a SIBLING: which pane it sits beside,
/// on which side, on which axis, at which share.
///
/// Two authors state one, for two different reasons, and they meet at the SAME insertion —
/// so there is one place a leaf is positioned, not one per author:
///
/// * a FLOAT **captures** one before the leaf collapses ([`LayoutTree::leaf_home`]), so docking
///   back returns the pane where it was — the answer to "a floated pane docks back WHERE?";
/// * a SPLIT **authors** one ([`LeafHome::beside`]), which is what makes "put a new pane below
///   pane 3" expressible at all: a direction is meaningless without the pane it is relative to.
///
/// Without one, re-tiling can only [`append`](LayoutTree::append_pane) and a pane loses its
/// place durably: float the middle of `0|1|2`, dock back, and it is `0|2|1` for good. That is
/// session state quietly discarded by the authority that owns it, so the home is stated
/// here rather than left to whichever client happens to be attached.
///
/// The home names a SIBLING rather than an index because an index means nothing once the
/// tiling reflows around the gap — the neighbour is the only fact that survives the user
/// re-arranging what is left. It is a memo, never an authority: an unhonorable home
/// (the sibling is gone, or floated out itself) degrades to an append, never an error.
#[derive(Clone, PartialEq, Debug)]
pub struct LeafHome {
    /// The sibling pane this leaf sits next to IN PAINT ORDER — see [`leaf_home_rec`] for why
    /// that is a different end of the sub-tree on each side.
    ///
    /// EXACT when the sibling was a bare leaf. When it was a SUB-TREE the pane returns
    /// adjacent to this representative rather than wrapping the whole sub-tree, and the
    /// honest bound is narrower than it looks:
    ///
    /// * the pane SEQUENCE is restored — that is what picking the adjacent end buys, and it
    ///   is the headline claim;
    /// * no pane is lost;
    /// * but the SHARES permute, and panes the user never touched are resized. See
    ///   [`LeafHome::ratio`].
    ///
    /// pinion's `DockTopology::leaf_anchor` documents a sub-tree bound too, but claims only
    /// *"no panel lost"* — do not read this as sourced from it beyond that word, and do not
    /// read pinion as promising the rest. pinion also calls a fully sub-tree-faithful wrap
    /// "a deferred follow-up gated on a consumer that needs pixel-exact nesting"; sprag IS
    /// that consumer, so this bound is sprag's to retire, not an inherited excuse.
    sibling: PaneId,
    /// Which side of the parent split the leaf held.
    side: SplitSide,
    /// The parent split's axis.
    dir: SplitDir,
    /// The parent split's share.
    ///
    /// **Restores the dragged boundary only when the sibling was a BARE LEAF.** Against a
    /// sub-tree sibling it is re-applied to the split the leaf is re-inserted at, which is a
    /// DIFFERENT boundary from the one it was captured off — so the order comes home and the
    /// sizes do not. `append` builds a right-nested spine, so in a row of N panes only the
    /// last two have a bare-leaf sibling: **the permuting case is the majority, not the
    /// corner**. Measured: `0|(1|2)` at even shares, float pane 0, dock back → order
    /// `[0,1,2]` restored, areas `.50/.25/.25` → `.25/.25/.50`.
    ratio: f32,
}

impl LeafHome {
    /// State the home of a leaf placed BESIDE `sibling`, on `side`, dividing it on `dir` — the
    /// SPLIT author's constructor. (A float's author is [`LayoutTree::leaf_home`], which READS
    /// one off the tree; this one AUTHORS a place that was never occupied.)
    ///
    /// The share is the even default a freshly-minted divider opens at, because a split creates
    /// a boundary nobody has dragged yet. The share the user later chooses is the tree's
    /// ([`LayoutNode::Split::ratio`]) — this type carries a share only so a CAPTURED home can
    /// bring one back.
    #[must_use]
    pub fn beside(sibling: PaneId, side: SplitSide, dir: SplitDir) -> Self {
        Self {
            sibling,
            side,
            dir,
            ratio: RATIO_DEFAULT,
        }
    }
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

    /// Exchange the leaves holding `a` and `b`, leaving every division — id, direction and
    /// ratio — exactly where it was.
    ///
    /// In place, and rewriting only the two leaves, because that is what makes the shape survive
    /// BY CONSTRUCTION: nothing is removed, so no split can collapse, and nothing has to be
    /// restored afterwards. A swap expressed as two removals plus two insertions would have to
    /// put back the very ratios the user dragged, and would have to do it in an order that works
    /// when the two panes are each other's sibling.
    ///
    /// A pane this sub-tree does not hold is simply not found; the caller checks membership.
    fn swap_ids(&mut self, a: PaneId, b: PaneId) {
        match self {
            Self::Leaf(pane) if *pane == a => *pane = b,
            Self::Leaf(pane) if *pane == b => *pane = a,
            Self::Leaf(_) => {}
            Self::Split { first, second, .. } => {
                first.swap_ids(a, b);
                second.swap_ids(a, b);
            }
        }
    }

    /// This sub-tree's panes, left-to-right / top-to-bottom (paint order).
    fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.panes_into(&mut out);
        out
    }

    /// This sub-tree's first pane in paint order.
    ///
    /// Total, not optional: a node is either a leaf or a division of two nodes, so every
    /// one bottoms out in at least one pane. Only [`LayoutTree::root`] can be empty.
    fn first_pane(&self) -> PaneId {
        match self {
            Self::Leaf(pane) => *pane,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    /// This sub-tree's last pane in paint order. Total, for the same reason as
    /// [`Self::first_pane`].
    fn last_pane(&self) -> PaneId {
        match self {
            Self::Leaf(pane) => *pane,
            Self::Split { second, .. } => second.last_pane(),
        }
    }

    /// Put `pane` back beside `home.sibling`, re-splitting that sibling's leaf on the side
    /// `home` recorded. Unchanged if this sub-tree does not hold the sibling.
    ///
    /// The split is minted a FRESH id rather than the retired one the leaf left with —
    /// see [`LayoutTree::insert_at_home`] for why sprag diverges from pinion here.
    fn insert_beside(
        self,
        pane: PaneId,
        home: &LeafHome,
        mint: &mut impl FnMut() -> SplitId,
    ) -> Self {
        match self {
            Self::Leaf(p) if p == home.sibling => {
                let (first, second) = match home.side {
                    SplitSide::First => (Self::Leaf(pane), Self::Leaf(p)),
                    SplitSide::Second => (Self::Leaf(p), Self::Leaf(pane)),
                };
                Self::Split {
                    id: mint(),
                    dir: home.dir,
                    ratio: home.ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }
            }
            leaf @ Self::Leaf(_) => leaf,
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
                // A pane holds at most one leaf (the tree's invariant), so at most one of
                // these two descents matches — the other rebuilds itself unchanged.
                first: Box::new(first.insert_beside(pane, home, mint)),
                second: Box::new(second.insert_beside(pane, home, mint)),
            },
        }
    }
}

/// Capture the home of `target`'s leaf: the parent split's axis / share, which side the leaf
/// held, and a representative pane of the sibling sub-tree.
///
/// **The representative is the sibling's pane ADJACENT IN PAINT ORDER, which is a different
/// end of the sub-tree on each side** — `second.first_pane()` for a `First` leaf,
/// `first.last_pane()` for a `Second` one. Re-splitting that leaf returns the pane to its old
/// slot in the pane SEQUENCE; re-splitting the far end inserts it into the middle instead.
///
/// Geometry cannot decide this and it is a mistake to reach for it: against a sub-tree sibling
/// there is often no single neighbouring pane — in `(0 over 2) | 1` BOTH 0 and 2 border pane
/// 1 — so paint order, which is total, is the fact that picks. Taking the far end for a
/// `Second` leaf made `(0 over 2) | 1` dock back as `(0|1) over 2`: the pane landed inside its
/// neighbour's quadrant at half its area while an untouched pane doubled — strictly WORSE than
/// the plain append this replaced. Every home test before that one used a right-nested boot
/// row, where the floated leaf is always `First` and the asymmetry cannot show.
///
/// `None` when `target` holds no leaf here, or holds the ROOT leaf — a sole tiled pane has no
/// parent split, hence no neighbour to come home to.
fn leaf_home_rec(node: &LayoutNode, target: PaneId) -> Option<LeafHome> {
    let LayoutNode::Split {
        dir,
        ratio,
        first,
        second,
        ..
    } = node
    else {
        return None;
    };
    let is_target = |n: &LayoutNode| matches!(n, LayoutNode::Leaf(p) if *p == target);
    if is_target(first) {
        return Some(LeafHome {
            sibling: second.first_pane(),
            side: SplitSide::First,
            dir: *dir,
            ratio: *ratio,
        });
    }
    if is_target(second) {
        return Some(LeafHome {
            // The sibling's LAST pane: the one this leaf follows in paint order.
            sibling: first.last_pane(),
            side: SplitSide::Second,
            dir: *dir,
            ratio: *ratio,
        });
    }
    leaf_home_rec(first, target).or_else(|| leaf_home_rec(second, target))
}

/// A leaf's extent on ONE axis, as a fraction of the window (`start` and `len` in `0.0..=1.0`).
///
/// Unit space, never cells. "What is left of pane 3" is a question about the ARRANGEMENT, so its
/// answer must not depend on the size of whichever client happens to be attached — the rival this
/// was derived against answers it from the last COMPOSED FRAME's rectangles, which is why theirs
/// moves with a sidebar, a tab bar and `u16` rounding.
///
/// It is used ONLY to RANK candidates the tree walk has already established exist. That is what
/// makes a float honest here: a rounding difference can pick a different one of two equally
/// overlapping neighbours, and can never invent or destroy one.
#[derive(Clone, Copy, Debug)]
struct Span {
    start: f64,
    len: f64,
}

impl Span {
    /// The whole window on this axis — where every walk starts.
    const FULL: Self = Self {
        start: 0.0,
        len: 1.0,
    };

    /// This span divided at `ratio`, keeping `side`.
    fn divide(self, ratio: f32, side: SplitSide) -> Self {
        // A tree written by a client carries the ratio it settled on; clamping here keeps a
        // malformed share (already refused at `set_from_wire`) from producing a negative length
        // that would order candidates nonsensically rather than merely oddly.
        let ratio = f64::from(ratio).clamp(0.0, 1.0);
        match side {
            SplitSide::First => Self {
                start: self.start,
                len: self.len * ratio,
            },
            SplitSide::Second => Self {
                start: self.start + self.len * ratio,
                len: self.len * (1.0 - ratio),
            },
        }
    }

    /// How much of this span `other` covers — `0.0` when they merely touch or miss.
    fn overlap(self, other: Self) -> f64 {
        let start = self.start.max(other.start);
        let end = (self.start + self.len).min(other.start + other.len);
        (end - start).max(0.0)
    }
}

/// What deriving adjacency needs of an arrangement's node, and ALL it needs: whether the node holds
/// a pane, or divides two sub-trees at a ratio.
///
/// A trait rather than a walk over [`LayoutNode`] for one reason. The arrangement the host WORKS in
/// and the arrangement it PUBLISHES ([`LayoutNodeWire`]) are two spellings of one shape, and a
/// client holding the published one has to be able to ask the question the host can answer — else
/// every reader that needs "which pane is to the left of this one" writes its own walk, and a second
/// derivation of adjacency is a second thing that can come to disagree with `select-pane -L`. There
/// is one derivation ([`neighbor_in`]); both forms are its input.
///
/// A node's split IDENTITY is deliberately absent: it is the one field the two forms genuinely
/// differ on (the wire's is optional, for a divider a client minted), and adjacency does not depend
/// on it. A trait that exposed it could not be implemented for both.
trait Arranged: Sized {
    /// This node as the adjacency walk reads it.
    fn shape(&self) -> Shape<'_, Self>;
}

/// One node of an arrangement as [`Arranged`] exposes it — the shape both the owned tree and its
/// wire twin have in common.
enum Shape<'a, N> {
    /// A pane occupies this cell.
    Leaf(PaneId),
    /// A division of two sub-trees at `ratio`, the `first` child's share.
    Split {
        dir: SplitDir,
        ratio: f32,
        first: &'a N,
        second: &'a N,
    },
}

impl Arranged for LayoutNode {
    fn shape(&self) -> Shape<'_, Self> {
        match self {
            Self::Leaf(pane) => Shape::Leaf(*pane),
            Self::Split {
                dir,
                ratio,
                first,
                second,
                ..
            } => Shape::Split {
                dir: *dir,
                ratio: *ratio,
                first,
                second,
            },
        }
    }
}

impl Arranged for LayoutNodeWire {
    fn shape(&self) -> Shape<'_, Self> {
        match self {
            Self::Leaf(pane) => Shape::Leaf(*pane),
            Self::Split {
                dir,
                ratio,
                first,
                second,
                ..
            } => Shape::Split {
                dir: *dir,
                ratio: *ratio,
                first,
                second,
            },
        }
    }
}

/// The ancestors of `pane`'s leaf, outermost first: each split and the side the descent took.
///
/// `false` — with `out` left exactly as it was found — when this sub-tree holds no such leaf, so a
/// caller can try siblings without clearing up after it.
fn leaf_path<'a, N: Arranged>(
    node: &'a N,
    pane: PaneId,
    out: &mut Vec<(&'a N, SplitSide)>,
) -> bool {
    match node.shape() {
        Shape::Leaf(held) => held == pane,
        Shape::Split { first, second, .. } => {
            out.push((node, SplitSide::First));
            if leaf_path(first, pane, out) {
                return true;
            }
            out.pop();
            out.push((node, SplitSide::Second));
            if leaf_path(second, pane, out) {
                return true;
            }
            out.pop();
            false
        }
    }
}

/// The leaves of `node` that FACE a pane lying on the other side of a division on `dir`'s axis,
/// each with its span across that axis — in paint order, which is what breaks a ranking tie.
///
/// Two rules, and they are the whole geometry:
///
/// * a division ACROSS the move puts both halves against the source, so both descend;
/// * a division ALONG it puts only the NEARER half against the source — looking left, the
///   sibling's own right-hand child — so the far half cannot be anyone's neighbour.
fn facing_leaves<N: Arranged>(node: &N, dir: PaneDir, span: Span, out: &mut Vec<(PaneId, Span)>) {
    match node.shape() {
        Shape::Leaf(pane) => out.push((pane, span)),
        Shape::Split {
            dir: node_dir,
            ratio,
            first,
            second,
        } => {
            // Derived here rather than passed in: the axis a span is measured across is a function
            // of `dir`, and a parameter carrying it would be a second copy of one fact that a
            // caller could contradict.
            if node_dir == dir.axis().across() {
                facing_leaves(first, dir, span.divide(ratio, SplitSide::First), out);
                facing_leaves(second, dir, span.divide(ratio, SplitSide::Second), out);
            } else {
                let near = match dir.side() {
                    SplitSide::First => second,
                    SplitSide::Second => first,
                };
                facing_leaves(near, dir, span, out);
            }
        }
    }
}

/// The pane adjacent to `pane` in `dir` within the arrangement rooted at `root`, or `None` when
/// there is none — the ONE derivation of adjacency in this project.
///
/// Its statement, its guarantees and the reason it is structural rather than geometric are on
/// [`LayoutTree::neighbor`], which is one of its two callers; the other is [`LayoutWire::neighbor`],
/// so a client reading a published arrangement gets the daemon's own answer rather than its own.
fn neighbor_in<N: Arranged>(root: &N, pane: PaneId, dir: PaneDir) -> Option<PaneId> {
    let mut path = Vec::new();
    if !leaf_path(root, pane, &mut path) {
        return None;
    }
    // The span of each ancestor ACROSS the move, and — after the loop — of the pane's own
    // leaf. Recorded before the node's own division is applied, which is what makes
    // `spans[depth]` the span of the sub-tree that hangs off `path[depth]`.
    let across = dir.axis().across();
    let mut span = Span::FULL;
    let mut spans = Vec::with_capacity(path.len());
    for (node, side) in &path {
        spans.push(span);
        if let Shape::Split {
            dir: node_dir,
            ratio,
            ..
        } = node.shape()
            && node_dir == across
        {
            span = span.divide(ratio, *side);
        }
    }
    let source = span;
    // INNERMOST first: the nearest division on this axis is the one whose other half is
    // actually adjacent. An outer one is only reached when every inner division runs the
    // other way, which is precisely when the whole inner sub-tree faces that boundary.
    for (depth, (node, side)) in path.iter().enumerate().rev() {
        let Shape::Split {
            dir: node_dir,
            first,
            second,
            ..
        } = node.shape()
        else {
            continue;
        };
        if node_dir != dir.axis() || *side == dir.side() {
            continue;
        }
        let sibling = match dir.side() {
            SplitSide::First => first,
            SplitSide::Second => second,
        };
        let mut candidates = Vec::new();
        facing_leaves(sibling, dir, spans[depth], &mut candidates);
        return candidates
            .into_iter()
            .reduce(|best, next| {
                // Strictly greater, so a tie keeps the earlier one in paint order.
                if next.1.overlap(source) > best.1.overlap(source) {
                    next
                } else {
                    best
                }
            })
            .map(|(pane, _)| pane);
    }
    None
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

    /// The arranged panes in paint order (left-to-right / top-to-bottom). Empty when the
    /// window tiles nothing.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        self.root
            .as_ref()
            .map(LayoutNode::panes)
            .unwrap_or_default()
    }

    /// The pane ADJACENT to `pane` in `dir`, or `None` when there is none — which is exactly the
    /// statement "`pane` is at that edge of the window".
    ///
    /// Answers herdr's `pane.neighbor` AND its `pane.edges` with one derivation, deliberately:
    /// their two methods reach the same fact by two different routes (a rect-vs-area comparison and
    /// a walk over the other panes' rects) and nothing makes them agree. Here the edge IS the
    /// absent neighbour, so the two halves cannot disagree — a shape that is unrepresentable rather
    /// than merely forbidden.
    ///
    /// **Structural, not geometric.** Whether a neighbour EXISTS is decided by the tree's shape
    /// alone: walk up to the first division on `dir`'s axis that this pane sits on the far side of.
    /// Only the CHOICE between several candidates (a column of panes on the other side of one
    /// divider) consults the ratios, as fractions of the window, and picks the greatest overlap across the
    /// other axis with paint order as the tie-break. So the answer is the same for every client, at
    /// every size, and with no client attached at all.
    ///
    /// `None` for a pane this tree holds no leaf for — one that exited, or that a client has
    /// FLOATED out of the tiling. A floating pane is still a pane and can still be the active one;
    /// it is simply not in the arrangement adjacency is a property of.
    ///
    /// The derivation is shared with [`LayoutWire::neighbor`], so a client reading the arrangement
    /// this host PUBLISHES gets THIS answer rather than one of its own: the two are one walk over
    /// one shape, not two implementations that agree today.
    #[must_use]
    pub fn neighbor(&self, pane: PaneId, dir: PaneDir) -> Option<PaneId> {
        neighbor_in(self.root.as_ref()?, pane, dir)
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

    /// Divide `target`'s cell and put `pane` in the half on `side`, along `dir`. Returns whether
    /// `target` was there to divide.
    ///
    /// **Two callers, one operation**, which is why the name says PLACE rather than split: the
    /// pane being placed is a freshly spawned one for a directional split (tmux `split-window -h` /
    /// `-v`) and an already-tiled one for a move (tmux `move-pane`), and the tree cannot tell the
    /// two apart — nor should it, since the position asked for is the same position. It was called
    /// `place_beside` while the split was its only caller, which hid the move inside it for as long
    /// as nobody looked.
    ///
    /// This is the operation [`append_pane`](Self::append_pane) is the direction-less form of:
    /// an append states WHERE only by convention (the rightmost spine), while a placement states it
    /// relative to a pane the caller named. Both end at the same insertion, so the tree has one
    /// positioning path regardless of which one asked.
    ///
    /// `false` — and the tree UNCHANGED — when `target` holds no leaf here (it exited, it is
    /// floating, or it is another window's), or when it IS `pane`. The caller refuses rather
    /// than falling back to an append: a direction the user spelled is a request, and silently
    /// appending instead would be the same lie as accepting `-h` and ignoring it.
    ///
    /// `pane` is MOVED if it already holds a leaf, never duplicated — the property the move caller
    /// rests on entirely, and which the split caller needs for a different reason: a freshly spawned
    /// pane can be [`reconcile`](Self::reconcile)d into place by another client's read before its
    /// split lands, and the split must still put it where it was asked to go.
    pub fn place_beside(
        &mut self,
        pane: PaneId,
        target: PaneId,
        side: SplitSide,
        dir: SplitDir,
    ) -> bool {
        if pane == target || !self.panes().contains(&target) {
            return false;
        }
        // Remove FIRST so an already-arranged pane moves instead of appearing twice. `target`
        // survives it (it is a different pane, and a pane holds at most one leaf), so the home
        // built next is still honorable — which is why this cannot half-apply.
        self.remove_pane(pane);
        self.insert_at_home(pane, &LeafHome::beside(target, side, dir))
    }

    /// Exchange the POSITIONS of two arranged panes — tmux `swap-pane`. Returns whether both
    /// were there to exchange; `false` leaves the tree untouched.
    ///
    /// The sibling of [`place_beside`](Self::place_beside), and deliberately NOT expressible as a
    /// pair of placements. A placement names where a pane goes; a swap names only that two panes
    /// trade, and the shapes they trade into are whatever each already had. Doing it as two
    /// placements would mean naming those shapes — reconstructing the ratio the user dragged, in
    /// an order that still works when the two panes are each other's sibling. Exchanging the two
    /// leaf ids where they sit keeps every division's id, direction and ratio by construction, so
    /// there is nothing to reconstruct and nothing to get wrong.
    ///
    /// Swapping a pane with ITSELF is `false`, not a panic and not a silent success: it changes
    /// nothing, and the caller reports "nothing moved" for it exactly as it does for a direction
    /// with no neighbour.
    ///
    /// Both panes must hold a leaf HERE. A cross-window swap is not this function — the two panes
    /// are in different trees and different pools, so it is the registry's
    /// ([`SessionRegistry::swap_panes`](crate::SessionRegistry::swap_panes)), built out of
    /// [`leaf_home`](Self::leaf_home) instead.
    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> bool {
        if a == b {
            return false;
        }
        let panes = self.panes();
        if !panes.contains(&a) || !panes.contains(&b) {
            return false;
        }
        if let Some(root) = self.root.as_mut() {
            root.swap_ids(a, b);
        }
        true
    }

    /// Drop `pane`'s leaf; its sibling reclaims the space. A no-op if it is not arranged.
    pub fn remove_pane(&mut self, pane: PaneId) {
        if let Some(root) = self.root.take() {
            self.root = root.remove(pane);
        }
    }

    /// Capture where `pane`'s leaf sits, so a later re-tile can put it back
    /// ([`LeafHome`]). `None` if it holds no leaf here, or holds the sole one.
    ///
    /// Read it BEFORE the leaf collapses: once the tiling reflows over the gap, the fact is
    /// gone and nothing can reconstruct it.
    #[must_use]
    pub fn leaf_home(&self, pane: PaneId) -> Option<LeafHome> {
        leaf_home_rec(self.root.as_ref()?, pane)
    }

    /// Re-tile `pane` at the `home` it left, returning whether the home could be honored.
    ///
    /// `false` — and the tree UNCHANGED — when the home's sibling is not currently tiled:
    /// it exited, or the user floated it out too. The caller falls back to
    /// [`append_pane`](Self::append_pane), which is where a pane with no home to return to
    /// has always gone. Note the test is "is the sibling in the TREE", not "is it alive":
    /// a home whose sibling is itself floating is just as unhonorable as one whose sibling
    /// exited, and asking the tree answers both without enumerating the cases.
    ///
    /// **The restored split is minted a FRESH [`SplitId`]** — deliberately unlike pinion's
    /// `insert_leaf_at_anchor`, which reuses the retired one so a binding's per-split state
    /// re-binds. sprag cannot: ids here are never reused
    /// (see [`SplitId`]), and a client keys its live drag ratio on them, so reissuing a
    /// retired id would re-bind a divider's drag state to a different boundary. The share
    /// the user chose comes home in [`LeafHome::ratio`] instead — carried by the tree,
    /// which is the durable authority for it anyway ([`LayoutNode::Split::ratio`]).
    fn insert_at_home(&mut self, pane: PaneId, home: &LeafHome) -> bool {
        let Some(root) = self.root.take() else {
            return false;
        };
        if !root.panes().contains(&home.sibling) {
            self.root = Some(root);
            return false;
        }
        let next = &mut self.next_split;
        self.root = Some(root.insert_beside(pane, home, &mut || Self::mint(next)));
        true
    }

    /// Self-heal this arrangement against the window's live pane set: drop the leaves of
    /// panes that are gone (siblings reclaim), then place every pane not yet arranged — at
    /// its [`LeafHome`] if `homes` has an honorable one, else appended in `panes` order.
    /// Panes already arranged keep their exact position + ratios.
    ///
    /// **Placing a pane SPENDS its home**, honored or not: once it is tiled it has a real
    /// position, and a stale memo of an older one could only fight it later. So `homes` is
    /// drained here — this is the one place a leaf moves, and therefore the one place a home
    /// can be consumed without the two coming apart.
    ///
    /// Homes are restored to a FIXPOINT rather than in `panes` order, because a home can
    /// name a pane that is itself docking back in this same reconcile (float 1 beside 2,
    /// float 2 beside 0, dock both back). In `panes` order pane 1 would find its sibling
    /// not yet tiled and append — losing a home that was about to become honorable. Each
    /// pass restores whoever can, until a pass restores nobody; then the rest append.
    /// The loop terminates: every pass either places a pane (there are finitely many) or
    /// breaks.
    pub fn reconcile(&mut self, panes: &[PaneId], homes: &mut HashMap<PaneId, LeafHome>) {
        let live: HashSet<PaneId> = panes.iter().copied().collect();
        for gone in self.panes().into_iter().filter(|p| !live.contains(p)) {
            self.remove_pane(gone);
        }
        loop {
            let arranged: HashSet<PaneId> = self.panes().into_iter().collect();
            let mut placed_one = false;
            for pane in panes.iter().filter(|p| !arranged.contains(p)) {
                let Some(home) = homes.get(pane).cloned() else {
                    continue;
                };
                if self.insert_at_home(*pane, &home) {
                    homes.remove(pane);
                    placed_one = true;
                }
            }
            if !placed_one {
                break;
            }
        }
        let arranged: HashSet<PaneId> = self.panes().into_iter().collect();
        for pane in panes.iter().filter(|p| !arranged.contains(p)) {
            homes.remove(pane);
            self.append_pane(*pane);
        }
        // A home is spent when its pane IS TILED — not only when this call placed it. A pane
        // can already hold a leaf and a home at once (floated and un-floated between two
        // reconciles, so the leaf never collapsed), and the loops above skip exactly that
        // pane because it is already arranged. Draining on the FACT ("it is tiled, so it has
        // a real position") rather than on the EVENT ("I just placed it") is what makes the
        // claim above true by construction, instead of true only while every caller happens
        // to reconcile between one float and the next.
        let tiled: HashSet<PaneId> = self.panes().into_iter().collect();
        homes.retain(|pane, _| !tiled.contains(pane));
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
    /// The write was authored against an arrangement that is no longer in force — someone
    /// else changed it first. Not malformed: simply answering a question that has moved on.
    Stale { expected: u64, actual: u64 },
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicatePane(pane) => write!(f, "pane {pane} is arranged twice"),
            Self::DuplicateSplitId(id) => write!(f, "split id {} is claimed twice", id.0),
            Self::InvalidRatio(ratio) => write!(f, "split ratio {ratio} is not a 0..=1 share"),
            Self::Stale { expected, actual } => write!(
                f,
                "arrangement was authored against revision {expected}, but {actual} is in force"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Check `node` is a well-formed arrangement, accumulating the panes and divider ids seen
/// so far so the checks are whole-tree (a duplicate is only visible across sub-trees).
///
/// Recursion is bounded by [`MAX_LAYOUT_DEPTH`], enforced where the wire form is BUILT
/// ([`LayoutWire`]'s deserialiser) rather than where its text is parsed. It used to read
/// "bounded by `serde_json`'s recursion limit", which was true of the socket path and false
/// of the in-process one (`from_value` has no such limit) — and was the same accident that
/// capped a session at 62 panes.
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
/// Same convention as `sprag-host`'s `PaneScrollFacts`: one definition of the field set, so
/// the two ends cannot drift on a field name.
///
/// ## Its serialised shape is FLAT, and that is load-bearing
///
/// This type nests in memory but serialises to an ARENA — a list of nodes that name their
/// children by index (see [`MAX_LAYOUT_DEPTH`] for the whole story). The nested spelling was
/// the obvious one and it made a window's JSON depth track its PANE COUNT, which is how a
/// session of more than 62 panes became unattachable: a window's arrangement is a
/// right-nested chain, so the last pane sat `2N + 2` levels down and every deserializer in
/// the project stops at `serde_json`'s default recursion limit of 128. Flat, the depth is a
/// constant four whatever the pane count, so nothing a user can do to a window bounds what a
/// client can read.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LayoutWire {
    /// The arrangement's root, or `None` when the window tiles no panes.
    pub root: Option<LayoutNodeWire>,
}

impl LayoutWire {
    /// Every pane this arrangement TILES, in paint order (left-to-right, top-to-bottom) — the same
    /// order [`LayoutTree::panes`] and a [`Tiling`](crate::Tiling) report.
    ///
    /// A floating pane is absent, because it has no leaf here: the host removes one from the tree
    /// when it floats, so "tiled" is not a flag to filter on but the shape of this structure. That
    /// is what makes this the honest set for a caller asking which panes a WINDOW is divided
    /// between — a client's own pane list would answer with the floating ones too.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        fn walk(node: &LayoutNodeWire, out: &mut Vec<PaneId>) {
            match node {
                LayoutNodeWire::Leaf(pane) => out.push(*pane),
                LayoutNodeWire::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = self.root.as_ref() {
            walk(root, &mut out);
        }
        out
    }

    /// The pane ADJACENT to `pane` in `dir`, or `None` when there is none — [`LayoutTree::neighbor`]
    /// asked of a PUBLISHED arrangement, by the same walk over the same shape.
    ///
    /// This is what a client that draws nothing needs and could not have. Adjacency is a function of
    /// the arrangement, so every reader of a [`LayoutSnapshot`] could in principle re-derive it —
    /// and a re-derivation is a SECOND definition of "the pane to the left", which would answer
    /// differently from the `select_pane` a keybinding invokes on the very arrangements that make
    /// the question interesting (a column facing one divider, where the choice is by overlap). One
    /// derivation, two forms.
    ///
    /// `None` for a pane with no leaf here — one that exited, or that a client has FLOATED out of
    /// the tiling — exactly as on the owned tree.
    #[must_use]
    pub fn neighbor(&self, pane: PaneId, dir: PaneDir) -> Option<PaneId> {
        neighbor_in(self.root.as_ref()?, pane, dir)
    }
}

/// One node of an arrangement in transit — the wire twin of [`LayoutNode`] (see
/// [`LayoutWire`]).
///
/// Deliberately NOT serialisable on its own: a node that could serialise itself would emit
/// the nested shape whose depth tracks the pane count, and one such call is all it takes to
/// put the ceiling back. [`LayoutWire`] owns the only serialised form there is, and it is
/// flat. This is a structural bar rather than a convention — there is no derive to forget.
#[derive(Clone, PartialEq, Debug)]
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
        id: Option<SplitId>,
        dir: SplitDir,
        ratio: f32,
        first: Box<LayoutNodeWire>,
        second: Box<LayoutNodeWire>,
    },
}

/// The deepest arrangement a [`LayoutWire`] may carry, and the reason it needs saying.
///
/// Until the wire form went flat, this bound existed but nobody chose it: a nested
/// arrangement's JSON depth tracked its pane count, so `serde_json`'s own recursion limit
/// stopped a deep tree before this module's own `validate`/`adopt` recursion could walk one
/// deep enough to overflow a stack. That was an accident twice over — it capped a USER at 62
/// panes, and it
/// was not even universal, since `serde_json::from_value` (the host's in-process arm) has no
/// such limit. A flat arena removes it entirely: depth is now a property of the tree the
/// arena DENOTES, and nothing about the text says how deep that is. So the bound moves to
/// where it belongs — the type's own deserialiser, which is the one gate every wire value
/// passes through — and gets a number chosen on purpose.
///
/// 1024 is far above any arrangement a machine can host (each leaf is a live PTY and a
/// process, and the deepest possible tree is one leaf per level), and far below what the
/// recursive walks over it cost: `validate`, `adopt`, [`LayoutWire::panes`] and the nested
/// `Drop` each take a small frame, so the worst case is a few hundred KB of a stack that has
/// megabytes.
pub const MAX_LAYOUT_DEPTH: usize = 1024;

/// One entry of the arena [`LayoutWire`] serialises to — a node whose children are INDICES
/// into the same arena rather than boxes inside it. Flattening is the whole point: this shape
/// has a fixed depth, so a window's pane count cannot deepen it.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlatNode {
    /// A pane occupies this cell — [`LayoutNodeWire::Leaf`].
    Leaf(PaneId),
    /// A division of two sub-trees, each named by its index — [`LayoutNodeWire::Split`].
    Split {
        #[serde(default)]
        id: Option<SplitId>,
        dir: SplitDir,
        ratio: f32,
        first: usize,
        second: usize,
    },
}

/// The arrangement's LEGACY nested form, accepted on READ so that a snapshot written before
/// the wire went flat still restores the user's sessions instead of booting them away.
///
/// Read-only and one migration long: nothing serialises this. Its recursion is safe for the
/// same reason the flat form's is not — a nested value's depth IS its JSON depth, so the
/// parser that produced it already refused anything deeper than it can walk.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyNode {
    Leaf(PaneId),
    Split {
        #[serde(default)]
        id: Option<SplitId>,
        dir: SplitDir,
        ratio: f32,
        first: Box<LegacyNode>,
        second: Box<LegacyNode>,
    },
}

impl From<LegacyNode> for LayoutNodeWire {
    fn from(node: LegacyNode) -> Self {
        match node {
            LegacyNode::Leaf(pane) => Self::Leaf(pane),
            LegacyNode::Split {
                id,
                dir,
                ratio,
                first,
                second,
            } => Self::Split {
                id,
                dir,
                ratio,
                first: Box::new(Self::from(*first)),
                second: Box::new(Self::from(*second)),
            },
        }
    }
}

/// A root as it may arrive: an arena INDEX (the flat form) or a whole nested node (the
/// legacy one). The two are distinguishable with no ambiguity — one is a number, the other an
/// object — which is what makes accepting both safe rather than a guess.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RootWire {
    Index(usize),
    Legacy(Box<LegacyNode>),
}

/// [`LayoutWire`]'s serialised shape, both forms at once: `nodes` is present exactly when the
/// arena form is in use, and absent for a legacy value whose whole tree hangs off `root`.
#[derive(serde::Deserialize)]
struct LayoutWireRepr {
    #[serde(default)]
    nodes: Option<Vec<FlatNode>>,
    #[serde(default)]
    root: Option<RootWire>,
}

/// The arena a [`LayoutWire`] writes: what [`LayoutWireRepr`] reads back.
#[derive(serde::Serialize)]
struct FlatLayout<'a> {
    nodes: &'a [FlatNode],
    root: Option<usize>,
}

/// Append `node`'s sub-tree to `arena` and answer the index it landed at.
///
/// Recursive, and deliberately unbounded here: this direction walks a tree the HOST already
/// holds in memory, on the same terms as [`LayoutWire::panes`] and `From<&LayoutNode>`. The
/// bound belongs on the reading side, where the tree is a stranger's ([`MAX_LAYOUT_DEPTH`]).
fn flatten(node: &LayoutNodeWire, arena: &mut Vec<FlatNode>) -> usize {
    let flat = match node {
        LayoutNodeWire::Leaf(pane) => FlatNode::Leaf(*pane),
        LayoutNodeWire::Split {
            id,
            dir,
            ratio,
            first,
            second,
        } => {
            // Children first, so their indices exist by the time this node names them.
            let first = flatten(first, arena);
            let second = flatten(second, arena);
            FlatNode::Split {
                id: *id,
                dir: *dir,
                ratio: *ratio,
                first,
                second,
            }
        }
    };
    arena.push(flat);
    arena.len() - 1
}

impl serde::Serialize for LayoutWire {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut nodes = Vec::new();
        let root = self.root.as_ref().map(|root| flatten(root, &mut nodes));
        FlatLayout {
            nodes: &nodes,
            root,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for LayoutWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let repr = LayoutWireRepr::deserialize(deserializer)?;
        match (repr.nodes, repr.root) {
            // The empty arrangement — a window that tiles no panes. Its arena, when it has
            // one, must be empty too: nodes under no root are panes that would silently
            // vanish, which is the same failure `build_arena` refuses as unreachable, and
            // refusing it there but not here would be a hole in that argument.
            (None, None) => Ok(Self { root: None }),
            (Some(nodes), None) if nodes.is_empty() => Ok(Self { root: None }),
            (Some(_), None) => Err(D::Error::custom(
                "an arrangement roots at nothing but still carries nodes",
            )),
            (Some(nodes), Some(RootWire::Index(root))) => Ok(Self {
                root: Some(build_arena(&nodes, root).map_err(D::Error::custom)?),
            }),
            // A snapshot from before the wire went flat.
            (None, Some(RootWire::Legacy(root))) => Ok(Self {
                root: Some(LayoutNodeWire::from(*root)),
            }),
            (None, Some(RootWire::Index(_))) => Err(D::Error::custom(
                "an arrangement names its root by index but carries no `nodes` arena",
            )),
            (Some(_), Some(RootWire::Legacy(_))) => Err(D::Error::custom(
                "an arrangement carries a `nodes` arena but spells its root out in full",
            )),
        }
    }
}

/// Turn a validated arena into the nested form the rest of this crate works in.
///
/// The check is ITERATIVE and the build is recursive, which is the whole trick: a stranger's
/// arena can denote a tree of any depth, so nothing may descend it until its depth is known
/// to be within [`MAX_LAYOUT_DEPTH`]. Once the walk below has proved that, the build cannot
/// recurse further than the walk already did.
///
/// The flat form admits three malformed shapes the nested one made UNREPRESENTABLE, so each
/// is rejected by name: a child index that names no node, a node reached twice (a cycle, or
/// two parents sharing one sub-tree — `adopt` would loop forever on the first and silently
/// duplicate panes on the second), and a node no walk from the root reaches, which would
/// quietly drop whatever panes it holds. This is the cost of flattening, paid in full here.
fn build_arena(nodes: &[FlatNode], root: usize) -> Result<LayoutNodeWire, String> {
    let mut seen = vec![false; nodes.len()];
    let mut reached = 0usize;
    let mut stack = vec![(root, 1usize)];
    while let Some((index, depth)) = stack.pop() {
        let node = nodes
            .get(index)
            .ok_or_else(|| format!("node {index} is named but not in the arrangement"))?;
        if std::mem::replace(&mut seen[index], true) {
            return Err(format!("node {index} is reached twice"));
        }
        reached += 1;
        if depth > MAX_LAYOUT_DEPTH {
            return Err(format!(
                "the arrangement nests deeper than the {MAX_LAYOUT_DEPTH} this build will walk"
            ));
        }
        if let FlatNode::Split { first, second, .. } = node {
            stack.push((*first, depth + 1));
            stack.push((*second, depth + 1));
        }
    }
    if reached != nodes.len() {
        return Err(format!(
            "the arrangement carries {} node(s) nothing reaches",
            nodes.len() - reached
        ));
    }
    Ok(nest(nodes, root))
}

/// Build the nested node at `index` — safe to recurse, because [`build_arena`] has already
/// proved every index resolves and the tree is no deeper than [`MAX_LAYOUT_DEPTH`].
fn nest(nodes: &[FlatNode], index: usize) -> LayoutNodeWire {
    match &nodes[index] {
        FlatNode::Leaf(pane) => LayoutNodeWire::Leaf(*pane),
        FlatNode::Split {
            id,
            dir,
            ratio,
            first,
            second,
        } => LayoutNodeWire::Split {
            id: *id,
            dir: *dir,
            ratio: *ratio,
            first: Box::new(nest(nodes, *first)),
            second: Box::new(nest(nodes, *second)),
        },
    }
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

/// A window's WHOLE arrangement — how its panes are tiled, which are floated out, and the
/// revision it is all at. What the host serves for the `layout` read, and returns from a write.
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
    /// How the TILED panes are arranged.
    pub tree: LayoutWire,
    /// The panes floated OUT of the tiling — those with no leaf in `tree`.
    ///
    /// Load-bearing for reattach, and not derivable from `tree`: "absent from the tiling"
    /// is exactly what a floated pane and a pane this client cannot see look like, so a
    /// client that only read `tree` would silently DROP a floated pane — it would render
    /// neither a leaf (correct, it is not tiled) nor a window (wrong, the pane is alive).
    /// Serving it is what lets a reattaching client put the user's floats back.
    ///
    /// WHERE each one's window sits on screen is deliberately NOT here: that is pixels, and
    /// it belongs to whichever client is drawing (see [`crate::Window`]). A reattaching
    /// client therefore restores WHICH panes float, and lets its window manager place them.
    #[serde(default)]
    pub floating: Vec<PaneId>,
    /// The pane filling the window on its own, or `None` while none is — tmux's zoom.
    ///
    /// It travels HERE, in the same read as the arrangement it filters, and that is the whole
    /// reason it is a pane id rather than the boolean the rival stores
    /// (herdr `9a4ce5e1`, `src/workspace/tab.rs:48`, whose zoom target is derived at paint time
    /// from whichever pane is focused). herdr can afford that: its renderer and its state are one
    /// process under one lock, so the two facts cannot be read a moment apart. sprag PUBLISHES
    /// them, and which pane is active is a different slot — so a boolean here would have to be
    /// joined against a fact fetched at another instant, and a client that woke between the two
    /// writes would fill the window with the wrong pane. Naming the pane makes the join
    /// unnecessary instead of making it careful.
    ///
    /// **This is not what to DRAW** — see [`projection`](Self::projection). `tree` remains the
    /// ARRANGEMENT: what `set_layout` writes back, what `move_pane` acts on, and what a caller
    /// that draws nothing reads to know where things are. Serving only the filtered tree would be
    /// tidier for a renderer and would blind exactly the audience those verbs exist for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoomed: Option<PaneId>,
}

impl LayoutSnapshot {
    /// What a client showing this window DRAWS — the arrangement as the zoom leaves it.
    ///
    /// The ONE place the two fields are combined, so the daemon's PTY reflow and both frontends
    /// answer "what is on screen" with one function rather than three agreeing branches. See
    /// [`Projection`](crate::Projection).
    #[must_use]
    pub fn projection(&self) -> crate::Projection<'_> {
        crate::Projection::of(&self.tree, self.zoomed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `reconcile` with no homes to restore — what a caller with none must now write, and
    /// writing it is the point: there is no shorter overload that drops them silently.
    fn heal(tree: &mut LayoutTree, panes: &[PaneId]) {
        tree.reconcile(panes, &mut HashMap::new());
    }

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

    /// Every split's ratio in `node`, depth-first — [`split_ids`]' companion, so a test can assert
    /// that a rearrangement kept the shares the user dragged as well as the dividers' identities.
    fn ratios(node: &LayoutNode) -> Vec<f32> {
        match node {
            LayoutNode::Leaf(_) => Vec::new(),
            LayoutNode::Split {
                ratio,
                first,
                second,
                ..
            } => {
                let mut out = vec![*ratio];
                out.extend(ratios(first));
                out.extend(ratios(second));
                out
            }
        }
    }

    /// Drag the OUTERMOST divider of `tree` to `share` — a stand-in for the user having moved it,
    /// so a test asserting that a rearrangement preserved the ratios has a ratio that is not the
    /// default one every fresh split would coincidentally produce.
    fn drag_outer_divider(tree: &mut LayoutTree, share: f32) {
        if let Some(LayoutNode::Split { ratio, .. }) = tree.root.as_mut() {
            *ratio = share;
        }
    }

    /// A tree built by hand, so a test states the ARRANGEMENT it means rather than the sequence of
    /// splits that happens to produce it.
    fn split(dir: SplitDir, ratio: f32, first: LayoutNode, second: LayoutNode) -> LayoutNode {
        LayoutNode::Split {
            id: SplitId(0),
            dir,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn leaf(pane: u64) -> LayoutNode {
        LayoutNode::Leaf(PaneId(pane))
    }

    /// A [`LayoutTree`] over a hand-built root. The minting counter starts past any id the shape
    /// uses, so a later mutation cannot collide with one.
    fn tree_of(root: LayoutNode) -> LayoutTree {
        LayoutTree {
            root: Some(root),
            next_split: 100,
        }
    }

    /// `pane`'s four neighbours, keyed the way the wire publishes them.
    fn around(tree: &LayoutTree, pane: u64) -> Vec<(&'static str, Option<u64>)> {
        PaneDir::ALL
            .iter()
            .map(|dir| {
                (
                    dir.wire_str(),
                    tree.neighbor(PaneId(pane), *dir).map(|id| id.0),
                )
            })
            .collect()
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

    /// The home of a leaf is its NEIGHBOUR plus the split it shared with it — the facts a
    /// dock-back needs and the only ones that survive the tiling reflowing over the gap.
    #[test]
    fn a_leaf_home_captures_the_sibling_side_and_share() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        // The user drags the inner divider off its even default, so the captured share is a
        // value only the TREE can supply. Asserting RATIO_DEFAULT here would pin nothing:
        // `append_pane` writes that same const, so a `leaf_home` that hardcoded it instead of
        // reading `*ratio` would pass.
        if let Some(LayoutNode::Split { second, .. }) = tree.root.as_mut()
            && let LayoutNode::Split { ratio, .. } = second.as_mut()
        {
            *ratio = 0.8;
        }
        // Right-nested `0 | (1 | 2)`: pane 1 is FIRST under the inner split, beside 2.
        let home = tree.leaf_home(PaneId(1)).expect("pane 1 is tiled beside 2");
        assert_eq!(home.sibling, PaneId(2));
        assert_eq!(home.side, SplitSide::First);
        assert_eq!(home.dir, SplitDir::Horizontal);
        assert!((home.ratio - 0.8).abs() < f32::EPSILON, "read, not assumed");
    }

    /// `(0 over 2) | 1` — two ordinary drags from boot. Pane 1 is the `Second` side of the
    /// root, so its home names the sibling COLUMN's last pane, not its first.
    ///
    /// Every other home test uses a right-nested boot row, where the floated leaf is always
    /// `First` and `second.first_pane()` happens to be the adjacent end — so the asymmetry
    /// hides and 48 green tests prove nothing about it. Getting this wrong docked pane 1 back
    /// INSIDE the top-left quadrant at half its area, which the plain append it replaced got
    /// right.
    #[test]
    fn a_second_side_leaf_comes_home_beside_its_actual_neighbour() {
        let mut tree = LayoutTree::new();
        tree.root = Some(LayoutNode::Split {
            id: SplitId(0),
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Split {
                id: SplitId(1),
                dir: SplitDir::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(0))),
                second: Box::new(LayoutNode::Leaf(PaneId(2))),
            }),
            second: Box::new(LayoutNode::Leaf(PaneId(1))),
        });
        tree.next_split = 2;
        let before = tree.panes();
        assert_eq!(before, vec![PaneId(0), PaneId(2), PaneId(1)]);

        let home = tree.leaf_home(PaneId(1)).expect("pane 1 is tiled");
        assert_eq!(
            home.sibling,
            PaneId(2),
            "pane 1 FOLLOWS the column's last pane (2) in paint order; 0 is the far end",
        );

        let mut homes = HashMap::from([(PaneId(1), home)]);
        heal(&mut tree, &[PaneId(0), PaneId(2)]); // pane 1 floats out
        tree.reconcile(&before, &mut homes); // …and docks back
        assert_eq!(
            tree.panes(),
            before,
            "the pane came home to its own column, not into its neighbour's quadrant",
        );
    }

    /// THE HONEST BOUND, pinned so it cannot be forgotten or quietly overstated: against a
    /// SUB-TREE sibling the pane's ORDER comes home but the SHARES permute — and that is the
    /// MAJORITY of positions in a default row, not a corner case (`append` builds a
    /// right-nested spine, so only the last two panes have a bare-leaf sibling).
    ///
    /// pinion ships a guard for its equivalent bound; R156 copied the bound's PROSE and left
    /// the guard behind, so nothing modelled the reachable-majority branch. If a future round
    /// makes the restore sub-tree-faithful, this test SHOULD fail — that is its job.
    #[test]
    fn a_subtree_sibling_restores_the_order_but_permutes_the_shares() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3)); // `0 | (1 | 2)`, every share even
        let before = tree.panes();

        // Float the LEFTMOST pane — the most ordinary float there is, and its sibling is the
        // whole `(1|2)` sub-tree.
        let home = tree.leaf_home(PaneId(0)).expect("pane 0 is tiled");
        let mut homes = HashMap::from([(PaneId(0), home)]);
        heal(&mut tree, &[PaneId(1), PaneId(2)]);
        tree.reconcile(&before, &mut homes);

        assert_eq!(tree.panes(), before, "the ORDER comes home");
        // …but pane 0 held half the window and now holds a quarter: the captured ratio was
        // re-applied to a boundary that is not the one it came off.
        let LayoutNode::Split { first, ratio, .. } = tree.root().unwrap() else {
            panic!("still a row")
        };
        assert!((*ratio - RATIO_DEFAULT).abs() < f32::EPSILON);
        assert!(
            matches!(first.as_ref(), LayoutNode::Split { .. }),
            "pane 0 came back NESTED beside pane 1, not spanning the left half: {:?}",
            tree.root(),
        );
    }

    /// The sole tiled pane has no parent split, hence no neighbour to come home to. `None`
    /// is the honest answer — not a home pointing at itself.
    #[test]
    fn the_sole_leaf_has_no_home_and_an_untiled_pane_has_none_either() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(1));
        assert!(tree.leaf_home(PaneId(0)).is_none(), "no parent split");
        assert!(tree.leaf_home(PaneId(7)).is_none(), "not tiled at all");
        assert!(LayoutTree::new().leaf_home(PaneId(0)).is_none(), "no root");
    }

    /// The payoff, at the algebra level: a pane re-tiled with its home lands back in its own
    /// place at its own share, not at the end.
    #[test]
    fn a_home_restores_the_pane_to_its_place_and_share() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        // The user drags the inner divider off its even default; that share is the one that
        // must come home.
        if let Some(LayoutNode::Split { second, .. }) = tree.root.as_mut()
            && let LayoutNode::Split { ratio, .. } = second.as_mut()
        {
            *ratio = 0.8;
        }

        let home = tree.leaf_home(PaneId(1)).unwrap();
        assert!((home.ratio - 0.8).abs() < f32::EPSILON, "captured the drag");
        let mut homes = HashMap::from([(PaneId(1), home)]);
        heal(&mut tree, &[PaneId(0), PaneId(2)]); // pane 1 floats out
        assert_eq!(tree.panes(), vec![PaneId(0), PaneId(2)]);

        tree.reconcile(&ids(3), &mut homes); // …and docks back
        assert_eq!(tree.panes(), ids(3), "home, not the end");
        assert!(homes.is_empty(), "an honored home is spent");
        let LayoutNode::Split { second, .. } = tree.root().unwrap() else {
            panic!("still a row")
        };
        let LayoutNode::Split { ratio, .. } = second.as_ref() else {
            panic!("re-split beside 2")
        };
        assert!(
            (*ratio - 0.8).abs() < f32::EPSILON,
            "the share the user chose came home, not the even default: {ratio}",
        );
    }

    /// A restored split is a NEW divider, not the retired one wearing its old name. sprag
    /// diverges from pinion's `insert_leaf_at_anchor` here on purpose — see
    /// `split_ids_are_unique_and_never_reused` for the reason a reissued id is unsafe.
    #[test]
    fn a_restored_home_mints_a_fresh_split_id_and_never_reissues_the_retired_one() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        let before: HashSet<SplitId> = split_ids(tree.root().unwrap()).into_iter().collect();

        let home = tree.leaf_home(PaneId(1)).unwrap();
        let mut homes = HashMap::from([(PaneId(1), home)]);
        heal(&mut tree, &[PaneId(0), PaneId(2)]);
        tree.reconcile(&ids(3), &mut homes);

        let after = split_ids(tree.root().unwrap());
        let unique: HashSet<SplitId> = after.iter().copied().collect();
        assert_eq!(unique.len(), after.len(), "ids stay unique: {after:?}");
        let fresh: Vec<_> = after.iter().filter(|id| !before.contains(id)).collect();
        assert_eq!(
            fresh.len(),
            1,
            "the re-split divider is a NEW id: {after:?}"
        );
    }

    /// A home is a memo, not a promise: a sibling that is not in the tiling cannot be
    /// re-split, so the pane appends — the behaviour a pane with no home has always had.
    ///
    /// This layer knows only "the sibling is absent"; WHY it is absent (it exited, or the
    /// user floated it out too) is the [`crate::Window`]'s distinction, and is tested there —
    /// the two are one case here, and writing them as two would assert the same thing twice.
    #[test]
    fn a_home_whose_sibling_is_not_tiled_degrades_to_an_append() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        let home = tree.leaf_home(PaneId(1)).unwrap();
        assert_eq!(home.sibling, PaneId(2));
        let mut homes = HashMap::from([(PaneId(1), home)]);

        // Pane 1 leaves the tiling, and so does its home sibling 2.
        heal(&mut tree, &[PaneId(0)]);
        tree.reconcile(&[PaneId(0), PaneId(1)], &mut homes);

        assert_eq!(
            tree.panes(),
            vec![PaneId(0), PaneId(1)],
            "no sibling to re-split, so the pane appends rather than failing",
        );
        assert!(
            homes.is_empty(),
            "a home is spent when the pane is tiled, honored or not — a stale memo of an \
             older place could only fight the real one later",
        );
    }

    /// Two panes docking back in ONE reconcile, where the first's home names the second.
    /// In `panes` order pane 1 would look for pane 2, not find it yet, and append — losing a
    /// home that was about to become honorable. The fixpoint restores whoever can, then asks
    /// again.
    #[test]
    fn homes_are_restored_to_a_fixpoint_not_in_pane_order() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        let home_1 = tree.leaf_home(PaneId(1)).unwrap();
        assert_eq!(home_1.sibling, PaneId(2), "1's home names 2");
        heal(&mut tree, &[PaneId(0), PaneId(2)]);
        let home_2 = tree.leaf_home(PaneId(2)).unwrap();
        assert_eq!(home_2.sibling, PaneId(0), "2's home names 0");
        heal(&mut tree, &[PaneId(0)]);

        // Both dock back at once. Pane 1 is unrestorable until pane 2 is back.
        let mut homes = HashMap::from([(PaneId(1), home_1), (PaneId(2), home_2)]);
        tree.reconcile(&ids(3), &mut homes);
        assert_eq!(
            tree.panes(),
            ids(3),
            "both reached their homes; neither fell to the end",
        );
        assert!(homes.is_empty());
    }

    #[test]
    fn reconcile_adds_new_panes_and_drops_gone_ones() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        assert_eq!(tree.panes(), ids(3), "unarranged panes get placed");

        // Pane 1 closed (e.g. a plugin reaped it straight off the Workspace) and pane 3
        // appeared: the arrangement self-heals against the live set.
        heal(&mut tree, &[PaneId(0), PaneId(2), PaneId(3)]);
        assert_eq!(tree.panes(), vec![PaneId(0), PaneId(2), PaneId(3)]);

        heal(&mut tree, &[]);
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
        heal(&mut tree, &ids(2));
        // The user dragged the divider off-centre.
        if let Some(LayoutNode::Split { ratio, .. }) = tree.root.as_mut() {
            *ratio = 0.8;
        }
        let split_id = match tree.root().unwrap() {
            LayoutNode::Split { id, .. } => *id,
            other => panic!("expected a split, got {other:?}"),
        };

        heal(&mut tree, &ids(3)); // a third pane appears

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
        heal(&mut tree, &ids(3));
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

    /// How many nesting levels `json` has, counted the way a parser counts them: every `{`
    /// or `[` is one deeper, and a brace inside a string is text.
    ///
    /// Measured on the TEXT rather than by walking a parsed `Value`, because the parse is
    /// the thing under test — a depth reported here is one a client's parser must descend
    /// before it can hand the reply to anything.
    fn json_depth(json: &str) -> usize {
        let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0usize, false, false);
        for byte in json.bytes() {
            if in_string {
                match byte {
                    _ if escaped => escaped = false,
                    b'\\' => escaped = true,
                    b'"' => in_string = false,
                    _ => {}
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        deepest
    }

    /// The exact text a client parses for a `layout` read: the host's answer inside the
    /// JSON-RPC envelope it is served in, for a window of `panes` panes in the shape the
    /// host actually builds — one `append_pane` per pane.
    fn served_layout_reply(panes: u64) -> String {
        let mut tree = LayoutTree::new();
        for pane in ids(panes) {
            tree.append_pane(pane);
        }
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": LayoutSnapshot {
                revision: 1,
                tree: LayoutWire::from(&tree),
                floating: Vec::new(),
                zoomed: None,
            },
        })
        .to_string()
    }

    /// An arrangement's wire depth must not grow with the pane count — the CAUSE behind a
    /// ceiling nobody designed.
    ///
    /// R263 found a session of more than 62 panes unattachable. The reason is here rather
    /// than in the transport: a window's arrangement is a right-nested chain, so a nested
    /// wire form buries the last pane `2N + 3` levels down, and every deserializer in the
    /// project stops at `serde_json`'s default recursion limit of 128. Depth that tracks a
    /// user's pane count is the defect; the ceiling is only where it first becomes visible.
    #[test]
    fn a_layouts_wire_depth_does_not_track_its_pane_count() {
        let shallow = json_depth(&served_layout_reply(2));
        for panes in [8_u64, 62, 63, 512] {
            assert_eq!(
                json_depth(&served_layout_reply(panes)),
                shallow,
                "a {panes}-pane arrangement must nest no deeper than a 2-pane one",
            );
        }
    }

    /// The SYMPTOM, pinned separately from its cause: a client can parse the arrangement it
    /// is served, whatever the pane count.
    ///
    /// This is `HostConn::call`'s own parse — `serde_json::from_str` over the reply line —
    /// so it reproduces an unattachable session with no socket, no daemon and no PTY. The
    /// same limit bites two more places the depth reaches: the host's parse of a client's
    /// layout WRITE, and `load_snapshot`, which answers `None` on a parse error and turns a
    /// saved session into a silent empty boot.
    #[test]
    fn a_client_parses_a_served_layout_at_any_pane_count() {
        for panes in [63_u64, 512] {
            let reply = served_layout_reply(panes);
            let parsed = serde_json::from_str::<serde_json::Value>(&reply);
            assert!(
                parsed.is_ok(),
                "a client must parse a {panes}-pane arrangement: {:?}",
                parsed.err(),
            );
        }
    }

    /// An arena denoting a right-nested chain of `splits` dividers (so a tree of depth
    /// `splits + 1`), as JSON text. Built by hand rather than by serialising a tree, because
    /// the point is to hand the deserialiser shapes no serialiser of ours would produce.
    fn chain_arena(splits: usize) -> serde_json::Value {
        let mut nodes = vec![serde_json::json!({ "leaf": 0 })];
        let mut root = 0;
        for k in 1..=splits {
            nodes.push(serde_json::json!({ "leaf": k }));
            let leaf = nodes.len() - 1;
            nodes.push(serde_json::json!({
                "split": { "id": null, "dir": "horizontal", "ratio": 0.5,
                           "first": leaf, "second": root },
            }));
            root = nodes.len() - 1;
        }
        serde_json::json!({ "nodes": nodes, "root": root })
    }

    /// The depth bound is where it CLAIMS to be — proved from both sides, because a bound
    /// only tested from the rejecting side is indistinguishable from one set far too low.
    ///
    /// The accepting half is the load-bearing one: it walks and then recursively builds a
    /// tree exactly [`MAX_LAYOUT_DEPTH`] deep on an ordinary test thread's stack, which is
    /// the evidence the constant was chosen with rather than asserted.
    #[test]
    fn an_arrangement_at_the_depth_bound_is_built_and_one_past_it_is_refused() {
        let deepest = chain_arena(MAX_LAYOUT_DEPTH - 1);
        let wire: LayoutWire =
            serde_json::from_value(deepest).expect("an arrangement at the bound is built");
        assert_eq!(
            wire.panes().len(),
            MAX_LAYOUT_DEPTH,
            "every pane of the deepest legal arrangement survives",
        );

        let deeper = serde_json::from_value::<LayoutWire>(chain_arena(MAX_LAYOUT_DEPTH));
        let error = deeper
            .expect_err("one level past the bound is refused")
            .to_string();
        assert!(
            error.contains(&MAX_LAYOUT_DEPTH.to_string()),
            "the refusal names the bound it enforced: {error}",
        );
    }

    /// The price of flattening, paid in full: an arena can spell out three arrangements the
    /// nested form made unrepresentable, and each is refused BY NAME rather than absorbed.
    ///
    /// None of these is hypothetical about what they would cost. A cycle makes `adopt` loop
    /// forever; a shared sub-tree duplicates every pane under it; an unreachable node holds
    /// panes that would simply vanish from the arrangement the user gets back.
    #[test]
    fn a_malformed_arena_is_refused_by_name() {
        let split = |first: usize, second: usize| {
            serde_json::json!({
                "split": { "id": null, "dir": "horizontal", "ratio": 0.5,
                           "first": first, "second": second },
            })
        };

        let cases = [
            (
                "a child index that names no node",
                serde_json::json!({ "nodes": [{ "leaf": 0 }, split(0, 7)], "root": 1 }),
                "not in the arrangement",
            ),
            (
                "a node reached twice — a cycle",
                serde_json::json!({ "nodes": [{ "leaf": 0 }, split(0, 1)], "root": 1 }),
                "reached twice",
            ),
            (
                "a node reached twice — two parents sharing one sub-tree",
                serde_json::json!({ "nodes": [{ "leaf": 0 }, split(0, 0)], "root": 1 }),
                "reached twice",
            ),
            (
                "a node no walk from the root reaches",
                serde_json::json!({
                    "nodes": [{ "leaf": 0 }, { "leaf": 1 }, split(0, 0)],
                    "root": 0,
                }),
                "nothing reaches",
            ),
            (
                "nodes under no root at all — the same vanishing, one step earlier",
                serde_json::json!({ "nodes": [{ "leaf": 0 }], "root": null }),
                "roots at nothing",
            ),
        ];

        for (what, value, expected) in cases {
            let error = serde_json::from_value::<LayoutWire>(value)
                .expect_err(&format!("{what} is refused"))
                .to_string();
            assert!(
                error.contains(expected),
                "{what} must be refused as {expected:?}, got {error:?}",
            );
        }
    }

    /// A snapshot written before the wire went flat still restores — the migration that keeps
    /// the fix from costing a user the sessions it was meant to protect.
    ///
    /// The legacy text is written out in full rather than generated, because a fixture built
    /// from today's types could not express the shape this is here to accept.
    #[test]
    fn the_legacy_nested_form_is_still_read() {
        let legacy = r#"{"root":{"split":{"id":0,"dir":"vertical","ratio":0.25,
            "first":{"leaf":7},
            "second":{"split":{"id":1,"dir":"horizontal","ratio":0.5,
                "first":{"leaf":8},"second":{"leaf":9}}}}}}"#;

        let wire: LayoutWire =
            serde_json::from_str(legacy).expect("a pre-flat arrangement still reads");
        assert_eq!(
            wire.panes(),
            vec![PaneId(7), PaneId(8), PaneId(9)],
            "every pane of the stored arrangement comes back, in order",
        );

        // And it comes back as a value the host will INSTALL, not merely one that parsed.
        let mut tree = LayoutTree::new();
        tree.set_from_wire(wire)
            .expect("a restored arrangement is well-formed");
        assert_eq!(tree.panes(), vec![PaneId(7), PaneId(8), PaneId(9)]);

        // Read is the only direction: what this build writes back is the flat form.
        let json = serde_json::to_string(&LayoutWire::from(&tree)).expect("it serialises");
        assert!(
            json.contains("\"nodes\":"),
            "a legacy arrangement is rewritten flat, not echoed nested: {json}",
        );
    }

    /// The migration runs ONE WAY, and this is what the other way costs — the diagnosis of a
    /// failure that went five rounds without one (R278's `sprag-tui` boot, "reproduction lost").
    ///
    /// This build reads a legacy arrangement (the test above). A build from BEFORE the flattening
    /// cannot read THIS one: its `root` was an externally tagged enum and ours is an arena INDEX,
    /// so `serde_json`'s `deserialize_enum` refuses the integer with a message about types — the
    /// exact sentence R278 chased. `LegacyNode` stands in for the deleted `LayoutNodeWire` because
    /// the two are the same externally tagged shape and the message comes from the DESERIALISER,
    /// not from the type's name.
    ///
    /// A single-pane window roots at index `0`, which is why the sentence said `integer 0` — a
    /// fresh client boot is exactly that window.
    ///
    /// It is pinned here, beside the migration it is the underside of, so that the reason
    /// [`sprag_rpc::WIRE_PROTOCOL`] exists cannot be read as caution about a hypothetical.
    #[test]
    fn an_older_build_cannot_read_this_ones_root_and_says_so_by_type() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(1));
        let flat = serde_json::to_value(LayoutWire::from(&tree)).expect("it serialises");
        assert_eq!(
            flat["root"],
            serde_json::json!(0),
            "a one-pane window roots at arena index 0 — the integer in the message",
        );

        let Err(refused) = serde_json::from_value::<LegacyNode>(flat["root"].clone()) else {
            panic!("an older build's root is an enum, and an index is not one");
        };
        assert_eq!(
            refused.to_string(),
            "invalid type: integer `0`, expected string or map",
            "the sentence R278 spent five hypotheses on is a version skew, spelled by serde",
        );
    }

    /// The write half's core promise: the host NAMES the dividers a client minted itself,
    /// and honors the identity of the ones it already knew.
    #[test]
    fn a_write_stamps_client_minted_dividers_and_keeps_known_ones() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(2)); // one divider, id 0

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
        heal(&mut tree, &ids(2));
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
        heal(&mut tree, &ids(3));
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
        heal(&mut tree, &ids(3));
        let once = tree.clone();
        heal(&mut tree, &ids(3));
        assert_eq!(tree, once, "reconciling an unchanged set changes nothing");
    }

    /// A split divides the pane it NAMES, on the axis it names — the fact `append_pane` cannot
    /// express and the whole reason this operation exists.
    ///
    /// Asserted through [`LayoutTree::leaf_home`], the RECIPROCAL reader: it reports where a leaf
    /// sits, so a home that reads back equal to the one the split authored is the tree agreeing
    /// with the request in the tree's own vocabulary, not a shape this test hand-copied.
    #[test]
    fn a_split_divides_the_pane_it_names_on_the_axis_it_names() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3)); // 0 | (1 | 2)

        assert!(tree.place_beside(PaneId(3), PaneId(1), SplitSide::Second, SplitDir::Vertical));

        assert_eq!(
            tree.leaf_home(PaneId(3)),
            Some(LeafHome::beside(
                PaneId(1),
                SplitSide::Second,
                SplitDir::Vertical
            )),
            "the new pane sits below pane 1, which is what was asked",
        );
        assert_eq!(
            tree.panes(),
            vec![PaneId(0), PaneId(1), PaneId(3), PaneId(2)],
            "and it lands INSIDE the row, not at the end an append would have chosen",
        );
    }

    /// `First` puts the new pane BEFORE its target (tmux `split-window -b`), on the same axis.
    #[test]
    fn a_split_on_the_first_side_puts_the_new_pane_before_its_target() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));

        assert!(tree.place_beside(PaneId(3), PaneId(1), SplitSide::First, SplitDir::Horizontal));

        assert_eq!(
            tree.leaf_home(PaneId(3)),
            Some(LeafHome::beside(
                PaneId(1),
                SplitSide::First,
                SplitDir::Horizontal
            )),
        );
        assert_eq!(
            tree.panes(),
            vec![PaneId(0), PaneId(3), PaneId(1), PaneId(2)],
        );
    }

    /// The RACE the operation is built to survive: a pane spawns, another client's read
    /// reconciles it to the END, and only then does the split land. It must MOVE the pane rather
    /// than plant a second leaf for it — otherwise the outcome depends on who ran in between.
    ///
    /// Revert-proof: drop `place_beside`'s `remove_pane` and `panes()` reports pane 3 TWICE.
    #[test]
    fn a_split_moves_a_pane_an_earlier_reconcile_already_appended() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(4)); // the interleaved reconcile: 0 | (1 | (2 | 3))

        assert!(tree.place_beside(PaneId(3), PaneId(1), SplitSide::Second, SplitDir::Vertical));

        assert_eq!(
            tree.panes(),
            vec![PaneId(0), PaneId(1), PaneId(3), PaneId(2)],
            "pane 3 moved beside its target and appears exactly once",
        );
    }

    /// A swap exchanges the two panes' POSITIONS and leaves every division exactly where it was —
    /// the property that makes it different from two placements, asserted on the divisions
    /// themselves rather than on the pane order alone.
    ///
    /// The control that makes it discriminate: the tree is given a NON-DEFAULT ratio first, so a
    /// swap implemented as remove-and-reinsert (which mints a fresh split at the even share) fails
    /// here while `panes()` alone would still read correct.
    ///
    /// Revert-proof: rewrite `swap_panes` as `remove_pane` + two `place_beside` calls and the split
    /// ids come back different and the 0.8 share comes back 0.5.
    #[test]
    fn a_swap_exchanges_positions_and_keeps_every_division() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3)); // 0 | (1 | 2)
        drag_outer_divider(&mut tree, 0.8);
        let before = tree.root().expect("three panes are arranged").clone();

        assert!(tree.swap_panes(PaneId(0), PaneId(2)));

        assert_eq!(
            tree.panes(),
            vec![PaneId(2), PaneId(1), PaneId(0)],
            "the two panes traded places and the untouched one stayed",
        );
        assert_eq!(
            split_ids(tree.root().expect("still arranged")),
            split_ids(&before),
            "every divider kept its id — nothing was retired and re-minted",
        );
        assert_eq!(
            ratios(tree.root().expect("still arranged")),
            ratios(&before),
            "and the share the user dragged is untouched",
        );
    }

    /// Two panes that are each other's SIBLING are the case a remove-and-reinsert swap cannot do
    /// in either order: removing the first collapses the very split the second would come home to.
    /// Exchanging the ids in place has no order to get wrong.
    #[test]
    fn a_swap_of_two_siblings_needs_no_order() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(2));
        let before = tree.root().expect("two panes are arranged").clone();

        assert!(tree.swap_panes(PaneId(0), PaneId(1)));

        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(0)]);
        assert_eq!(
            split_ids(tree.root().expect("still arranged")),
            split_ids(&before),
            "the divider between them is the same divider",
        );
    }

    /// A pane this tree does not hold, and a pane swapped with itself, both answer `false` with the
    /// arrangement untouched — the caller turns the first into a refusal and the second into
    /// "nothing moved", and neither can be told apart from a partial application here.
    #[test]
    fn a_swap_that_cannot_happen_changes_nothing() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        let before = tree.clone();

        assert!(!tree.swap_panes(PaneId(0), PaneId(9)), "9 is not arranged");
        assert!(!tree.swap_panes(PaneId(9), PaneId(0)), "in either position");
        assert!(!tree.swap_panes(PaneId(1), PaneId(1)), "nor with itself");
        assert_eq!(tree, before, "and the arrangement never moved");
    }

    /// A target that holds no leaf here — it exited, it is floating, or it is another window's —
    /// REFUSES, leaving the arrangement untouched. Silently appending would be the same lie as
    /// accepting `-h` and ignoring it.
    #[test]
    fn a_split_refuses_a_target_that_holds_no_leaf_and_changes_nothing() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        let before = tree.clone();

        assert!(!tree.place_beside(PaneId(9), PaneId(7), SplitSide::Second, SplitDir::Vertical));

        assert_eq!(tree, before, "a refused split does not half-apply");
    }

    /// A pane cannot be its own target: it would be removed to make room for itself and then
    /// find no sibling to sit beside.
    ///
    /// Revert-proof: drop the `pane == target` guard and this LOSES pane 1 from the tree
    /// entirely — the removal lands and the insertion cannot.
    #[test]
    fn a_split_refuses_its_own_pane_as_the_target() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));
        let before = tree.clone();

        assert!(!tree.place_beside(PaneId(1), PaneId(1), SplitSide::Second, SplitDir::Vertical));

        assert_eq!(tree, before, "the pane is still arranged where it was");
    }

    /// Splitting the SOLE pane is the ordinary first split of a fresh window, and it is the one
    /// case with no surrounding structure to preserve.
    #[test]
    fn a_split_divides_the_sole_pane() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(1));

        assert!(tree.place_beside(PaneId(1), PaneId(0), SplitSide::Second, SplitDir::Vertical));

        assert_eq!(tree.panes(), vec![PaneId(0), PaneId(1)]);
        assert_eq!(
            tree.leaf_home(PaneId(1)),
            Some(LeafHome::beside(
                PaneId(0),
                SplitSide::Second,
                SplitDir::Vertical
            )),
        );
    }

    /// The plain row, and the half of the claim that is easy to forget: the EDGE is not a second
    /// answer, it is the absent neighbour. Pane 0 has nothing to its left and pane 2 nothing to its
    /// right, and every pane in a row has nothing above or below.
    #[test]
    fn a_row_reports_each_neighbour_and_states_an_edge_as_no_neighbour() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(3));

        assert_eq!(
            around(&tree, 0),
            vec![
                ("left", None),
                ("right", Some(1)),
                ("up", None),
                ("down", None)
            ],
        );
        assert_eq!(
            around(&tree, 1),
            vec![
                ("left", Some(0)),
                ("right", Some(2)),
                ("up", None),
                ("down", None)
            ],
        );
        assert_eq!(
            around(&tree, 2),
            vec![
                ("left", Some(1)),
                ("right", None),
                ("up", None),
                ("down", None)
            ],
        );
    }

    /// Looking INTO a sub-tree: the pane against the boundary wins, never the one behind it.
    /// `(0|1) | 2` — pane 2's left neighbour is 1, and 0 is not a candidate at all.
    #[test]
    fn the_half_of_a_subtree_against_the_boundary_is_the_neighbour() {
        let tree = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1)),
            leaf(2),
        ));

        assert_eq!(tree.neighbor(PaneId(2), PaneDir::Left), Some(PaneId(1)));
        assert_eq!(tree.neighbor(PaneId(1), PaneDir::Right), Some(PaneId(2)));
        assert_eq!(tree.neighbor(PaneId(0), PaneDir::Right), Some(PaneId(1)));
    }

    /// Several panes face the boundary, so the ratios pick — and the CONTROL is the same
    /// arrangement with the share moved, which must move the answer. A test that only asserted the
    /// `0.25` case would pass equally on "return the first candidate in paint order".
    #[test]
    fn the_candidate_that_overlaps_most_wins_and_the_share_moves_the_answer() {
        // `0 | (1 over 2)`, the column's divider a quarter of the way down: pane 2 covers three
        // quarters of pane 0's height, so it is what a move to the right lands on.
        let low = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            leaf(0),
            split(SplitDir::Vertical, 0.25, leaf(1), leaf(2)),
        ));
        assert_eq!(low.neighbor(PaneId(0), PaneDir::Right), Some(PaneId(2)));

        let high = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            leaf(0),
            split(SplitDir::Vertical, 0.75, leaf(1), leaf(2)),
        ));
        assert_eq!(
            high.neighbor(PaneId(0), PaneDir::Right),
            Some(PaneId(1)),
            "THE CONTROL: only the share differs, so an implementation that ignored it would \
             answer the same pane twice",
        );

        // Dead even: neither covers more, so paint order decides rather than the float noise.
        let even = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            leaf(0),
            split(SplitDir::Vertical, 0.5, leaf(1), leaf(2)),
        ));
        assert_eq!(even.neighbor(PaneId(0), PaneDir::Right), Some(PaneId(1)));
    }

    /// The property the whole structural walk exists for. A share this extreme lays the pane out
    /// under one cell wide in any real window, and a neighbour walk that ran over a client's
    /// ROUNDED rectangles would lose it — the rival's does, because its candidate filter needs a
    /// positive overlap of `u16` extents. Nothing here has a size to round.
    #[test]
    fn a_share_too_small_to_round_to_a_cell_still_has_its_neighbours() {
        let tree = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            leaf(0),
            split(SplitDir::Vertical, 0.001, leaf(1), leaf(2)),
        ));

        assert_eq!(
            tree.neighbor(PaneId(1), PaneDir::Left),
            Some(PaneId(0)),
            "a sliver pane still borders the pane beside it",
        );
        assert_eq!(tree.neighbor(PaneId(1), PaneDir::Down), Some(PaneId(2)));
        assert_eq!(
            tree.neighbor(PaneId(0), PaneDir::Right),
            Some(PaneId(2)),
            "and the sliver does not WIN the reverse question — the pane covering the rest does",
        );
    }

    /// Up and down are the same walk on the other axis, checked on a shape where the two disagree:
    /// a column of rows, where the row above pane 2 is a ROW, not the pane that happens to be first.
    #[test]
    fn the_vertical_walk_crosses_a_row_the_same_way() {
        // (0 | 1) over (2 | 3), the row divider a third of the way across, so 0 is narrow.
        let tree = tree_of(split(
            SplitDir::Vertical,
            0.5,
            split(SplitDir::Horizontal, 0.33, leaf(0), leaf(1)),
            split(SplitDir::Horizontal, 0.33, leaf(2), leaf(3)),
        ));

        assert_eq!(tree.neighbor(PaneId(2), PaneDir::Up), Some(PaneId(0)));
        assert_eq!(tree.neighbor(PaneId(3), PaneDir::Up), Some(PaneId(1)));
        assert_eq!(tree.neighbor(PaneId(0), PaneDir::Down), Some(PaneId(2)));
        assert_eq!(
            around(&tree, 0),
            vec![
                ("left", None),
                ("right", Some(1)),
                ("up", None),
                ("down", Some(2))
            ],
        );
    }

    /// **The arrangement a client is HANDED answers adjacency exactly as the tree it came from.**
    ///
    /// The property the shared derivation exists for. A reader holding a [`LayoutSnapshot`] — the
    /// MCP layout tool, any client that draws nothing — asks [`LayoutWire::neighbor`] and gets the
    /// answer `select-pane -L` would give, because it IS that answer rather than a second one that
    /// agrees today.
    ///
    /// Checked over every pane and all four directions of four shapes, including the pair whose
    /// answer only the RATIO decides — which is exactly where a re-derivation drifts, since the
    /// tree's shape alone does not settle it. The two explicit tables are what stop this from
    /// passing vacuously: an equality assertion holds when both sides answer nothing.
    ///
    /// REVERT-PROOF: `LayoutWire::neighbor` returning `None` fails this test and no other.
    #[test]
    fn the_published_arrangement_answers_adjacency_exactly_as_its_tree_does() {
        /// `pane`'s four neighbours as a client reads them off the PUBLISHED form.
        fn around_wire(wire: &LayoutWire, pane: u64) -> Vec<(&'static str, Option<u64>)> {
            PaneDir::ALL
                .iter()
                .map(|dir| {
                    (
                        dir.wire_str(),
                        wire.neighbor(PaneId(pane), *dir).map(|id| id.0),
                    )
                })
                .collect()
        }

        // `0 | (1 over 2)`, the column's divider a quarter of the way down, and the SAME shape with
        // the share moved. Only the ratio differs, so a walk that ignored it would answer alike.
        let low = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            leaf(0),
            split(SplitDir::Vertical, 0.25, leaf(1), leaf(2)),
        ));
        let high = tree_of(split(
            SplitDir::Horizontal,
            0.5,
            leaf(0),
            split(SplitDir::Vertical, 0.75, leaf(1), leaf(2)),
        ));

        assert_eq!(
            around_wire(&LayoutWire::from(&low), 0),
            vec![
                ("left", None),
                ("right", Some(2)),
                ("up", None),
                ("down", None)
            ],
            "the published form ranks by the share it carries",
        );
        assert_eq!(
            around_wire(&LayoutWire::from(&high), 0),
            vec![
                ("left", None),
                ("right", Some(1)),
                ("up", None),
                ("down", None)
            ],
            "THE CONTROL: the same shape with the share moved answers a different pane",
        );

        let shapes = [
            low,
            high,
            // Looking INTO a sub-tree, and a column of rows on the other axis.
            tree_of(split(
                SplitDir::Horizontal,
                0.5,
                split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1)),
                leaf(2),
            )),
            tree_of(split(
                SplitDir::Vertical,
                0.5,
                split(SplitDir::Horizontal, 0.33, leaf(0), leaf(1)),
                split(SplitDir::Horizontal, 0.33, leaf(2), leaf(3)),
            )),
        ];
        for (shape, tree) in shapes.iter().enumerate() {
            let wire = LayoutWire::from(tree);
            for pane in tree.panes() {
                assert_eq!(
                    around_wire(&wire, pane.0),
                    around(tree, pane.0),
                    "shape {shape}, pane {pane:?}: the two forms are one derivation",
                );
            }
        }
    }

    /// The two panes with no answer at all: the SOLE tiled pane (no division to cross) and one the
    /// tiling does not hold — a pane that exited, or one a client has FLOATED out. A floating pane
    /// can still be the ACTIVE pane; it is simply not in the arrangement adjacency is about.
    #[test]
    fn a_sole_pane_and_an_untiled_pane_have_no_neighbours() {
        let mut tree = LayoutTree::new();
        heal(&mut tree, &ids(1));

        assert_eq!(
            around(&tree, 0),
            vec![
                ("left", None),
                ("right", None),
                ("up", None),
                ("down", None)
            ],
        );
        assert_eq!(
            around(&tree, 7),
            vec![
                ("left", None),
                ("right", None),
                ("up", None),
                ("down", None)
            ],
            "a pane this tree holds no leaf for is not at an edge — it is not in the tiling",
        );
        assert_eq!(
            around(&LayoutTree::new(), 0),
            vec![
                ("left", None),
                ("right", None),
                ("up", None),
                ("down", None)
            ],
            "and an empty window answers rather than panicking",
        );
    }

    /// The direction vocabulary has ONE definition, so a word the wire accepts is a word this walk
    /// understands and back again.
    #[test]
    fn the_direction_words_round_trip_and_a_fifth_spelling_is_refused() {
        for dir in PaneDir::ALL {
            assert_eq!(PaneDir::from_wire(dir.wire_str()), Some(dir));
        }
        assert_eq!(PaneDir::from_wire("Left"), None);
        assert_eq!(PaneDir::from_wire("west"), None);
        assert_eq!(PaneDir::from_wire(""), None);
        assert_eq!(
            PaneDir::ALL.map(PaneDir::axis),
            [
                SplitDir::Horizontal,
                SplitDir::Horizontal,
                SplitDir::Vertical,
                SplitDir::Vertical
            ],
        );
    }
}
