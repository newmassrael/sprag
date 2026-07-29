//! The host's logical arrangement, in CHARACTER CELLS.
//!
//! `sprag-gui` turns the same [`LayoutWire`] into pixel rectangles through pinion's dock surface;
//! this is its terminal peer, and the two are deliberately separate. The host owns WHICH panes are
//! split, in what order, at what proportion — so a detach preserves it — and each client owns the
//! unit that arrangement becomes. That is why [`sprag_client`] hands out logical trees and knows
//! about neither pixels nor cells.
//!
//! # Rounding is the whole difficulty, and it is not a detail here
//!
//! Pixels round at sub-cell granularity and nobody notices. Character cells round at the
//! granularity the user counts: a boundary that lands one column left of where it did last frame is
//! a whole screen redraw, and a user watching a 40/41 split flip to 41/40 as they type is watching a
//! bug. So the division here is INTEGER and total — [`divide`] states it once, every split goes
//! through it, and the same inputs always yield the same cells.
//!
//! # What a divider costs, and why it is a cell rather than a hint
//!
//! Two panes side by side need something between them or they read as one pane with strange
//! content. In pixels that is a hairline a client can draw between two rectangles; in cells there is
//! no between, so the divider OCCUPIES a column (or a row) and the panes get what is left. This is
//! what makes the design's example exact: a 0.5 ratio over 81 columns is 40 and 40 with a divider
//! column, not 40 and 41.
//!
//! # The bound: a region too small to hold both children shows ONE
//!
//! The arrangement is the host's and can hold more panes than this terminal has rows — a GUI on a
//! large display splitting a session an 80x24 ssh client is also attached to. A split whose axis
//! cannot give both children a cell and a divider one gives the whole region to `first`, and the
//! second child (with everything under it) is OMITTED from the tiling: it has no rectangle, it is
//! not painted, and it is not resized. Dropping it is the honest answer — a zero-column pane is not
//! a pane, and the host is right to refuse a resize to one — but the pane is still ALIVE and still
//! the session's, so a client that shrinks below its arrangement loses the view, never the work.

use sprag_terminal::{LayoutNodeWire, LayoutWire, PaneId, SplitDir};

/// A rectangle of character cells in the local terminal's coordinates, `col`/`row` counted from the
/// top-left of the screen.
///
/// Its own type rather than a tuple because a `(u16, u16, u16, u16)` at a call site says nothing
/// about which pair is the origin, and the two are not interchangeable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    /// The leftmost column, inclusive.
    pub col: u16,
    /// The topmost row, inclusive.
    pub row: u16,
    /// How many columns wide.
    pub cols: u16,
    /// How many rows tall.
    pub rows: u16,
}

impl Rect {
    /// A rectangle of `cols` x `rows` cells with its top-left corner at `(col, row)`.
    #[must_use]
    pub const fn new(col: u16, row: u16, cols: u16, rows: u16) -> Self {
        Self {
            col,
            row,
            cols,
            rows,
        }
    }

    /// A rectangle of `cols` x `rows` cells at the screen's origin — the whole of a terminal.
    #[must_use]
    pub const fn screen(cols: u16, rows: u16) -> Self {
        Self::new(0, 0, cols, rows)
    }

    /// Whether this rectangle holds no cells at all.
    ///
    /// Either dimension being zero is enough, and both are reachable: a terminal that reports no
    /// size, and a region a split had nothing left to give.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cols == 0 || self.rows == 0
    }

    /// Whether the cell at `(col, row)` lies inside this rectangle.
    #[must_use]
    pub const fn holds(&self, col: u16, row: u16) -> bool {
        col >= self.col
            && row >= self.row
            && col < self.col.saturating_add(self.cols)
            && row < self.row.saturating_add(self.rows)
    }
}

/// A pane and the rectangle of cells it was given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaneRect {
    /// The pane, by its registry-global id.
    pub pane: PaneId,
    /// Where it goes on this terminal.
    pub area: Rect,
}

/// The line of cells between the two halves of one split.
///
/// One cell thick on the axis it divides and spanning its region on the other — so a `Horizontal`
/// split (panes side by side, the host's and tmux's `-h` vocabulary) yields a one-COLUMN divider,
/// and a `Vertical` one a single ROW.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Divider {
    /// The cells the divider occupies. No pane holds them.
    pub area: Rect,
    /// The split this divider belongs to, which is what decides the glyph drawn in those cells.
    pub dir: SplitDir,
}

/// Where every pane of an arrangement goes on this terminal, and what separates them.
///
/// **The panes and the dividers PARTITION the tiled area exactly**: every cell belongs to one pane
/// or one divider, and none to both. That is what lets the client repaint without clearing — each
/// cell has exactly one author, so nothing it draws can leave a hole for a previous frame to show
/// through. The one way the property weakens is a pane the area was too small to hold, which is
/// absent from the tiling entirely rather than present with an empty rectangle (see the module
/// docs); the region it would have occupied belongs to its surviving sibling.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tiling {
    /// The panes that fit, in PAINT ORDER (left-to-right, top-to-bottom) — the same order
    /// [`LayoutTree::panes`](sprag_terminal::LayoutTree::panes) reports, so a client cycling
    /// through them moves the way the arrangement reads.
    pub panes: Vec<PaneRect>,
    /// The dividers between them, in the order their splits were walked.
    pub dividers: Vec<Divider>,
}

impl Tiling {
    /// Where `pane` goes, or `None` when this arrangement does not show it — it is floating, it
    /// belongs to another window, or the terminal was too small to hold it.
    #[must_use]
    pub fn area_of(&self, pane: PaneId) -> Option<Rect> {
        self.panes
            .iter()
            .find(|held| held.pane == pane)
            .map(|held| held.area)
    }

    /// The first pane in paint order, or `None` when nothing is tiled.
    ///
    /// The client's focus fallback: a total answer to "which pane, then?" that does not depend on
    /// what the focus was before it became unshowable.
    #[must_use]
    pub fn first_pane(&self) -> Option<PaneId> {
        self.panes.first().map(|held| held.pane)
    }

    /// The pane after `pane` in paint order, WRAPPING at the end — tmux's `select-pane -t :.+`,
    /// which is what its `prefix o` runs.
    ///
    /// A `pane` this tiling does not show yields the first one rather than nothing, because the
    /// caller's question is "where should focus go next" and there is always an answer while
    /// anything is tiled. `None` only when nothing is.
    #[must_use]
    pub fn next_after(&self, pane: PaneId) -> Option<PaneId> {
        let at = self.panes.iter().position(|held| held.pane == pane);
        match at {
            Some(at) => self.panes.get(at + 1).or_else(|| self.panes.first()),
            None => self.panes.first(),
        }
        .map(|held| held.pane)
    }

    /// Which pane holds screen cell `(col, row)`, and where that cell is INSIDE it.
    ///
    /// The inverse of the whole module: [`tile`] answers where a pane goes, and a pointer arrives
    /// as a screen cell needing the question asked backwards. `None` for a cell that belongs to no
    /// pane — a DIVIDER column, or a cell outside every rectangle — and those are not the same
    /// thing to the caller only in that neither may be forwarded to a child.
    ///
    /// The answer is unambiguous because the tiling is an exact PARTITION (see [`tile`]): every
    /// cell has one author. That is the same property that lets a repaint skip clearing, used here
    /// for a different purpose, and it is why this can return the first match rather than having to
    /// define a stacking order the way a pixel client must.
    ///
    /// The returned cell is pane-LOCAL and 0-based, which is the coordinate space
    /// [`MouseInput`](sprag_input::MouseInput) is defined in: a child knows only its own grid, and
    /// handing it a screen coordinate would put every click in the wrong place by exactly the
    /// pane's origin — invisibly so for the pane at (0, 0), which is the one a single-pane test
    /// would use.
    #[must_use]
    pub fn pane_at(&self, col: u16, row: u16) -> Option<(PaneId, u16, u16)> {
        self.panes
            .iter()
            .find(|held| held.area.holds(col, row))
            .map(|held| (held.pane, col - held.area.col, row - held.area.row))
    }
}

/// Lay `tree` out over `area` — the whole of the character-cell projection.
///
/// Pure, and deliberately so: it takes an arrangement and a rectangle and returns where things go,
/// which is a claim about geometry that can be asserted without a terminal, a host, or a socket.
/// Every property this module promises — the exact partition, the stable rounding, the reserved
/// divider — is a test over this function.
#[must_use]
pub fn tile(tree: &LayoutWire, area: Rect) -> Tiling {
    let mut tiling = Tiling::default();
    if let Some(root) = tree.root.as_ref() {
        tile_node(root, area, &mut tiling);
    }
    tiling
}

/// Lay one node out over `area`, appending what it holds to `tiling`.
///
/// Recursion order is `first` then `second`, which is why [`Tiling::panes`] comes out in paint
/// order: the tree's `first` is the LEFT or TOP child on both axes.
fn tile_node(node: &LayoutNodeWire, area: Rect, tiling: &mut Tiling) {
    // An empty region can hold nothing, and saying so here is what keeps every rectangle below
    // non-degenerate: a leaf never lands with zero cells, so a client can resize every pane it is
    // handed without checking.
    if area.is_empty() {
        return;
    }
    match node {
        LayoutNodeWire::Leaf(pane) => tiling.panes.push(PaneRect { pane: *pane, area }),
        LayoutNodeWire::Split {
            dir,
            ratio,
            first,
            second,
            ..
        } => {
            let extent = match dir {
                SplitDir::Horizontal => area.cols,
                SplitDir::Vertical => area.rows,
            };
            let Some((near, far)) = divide(extent, *ratio) else {
                // Too small for two: the region goes to `first` whole, and `second` is dropped
                // (module docs). Recursing rather than pushing a leaf keeps the rule uniform — a
                // sub-tree that fits nothing still resolves to whatever single pane it can show.
                tile_node(first, area, tiling);
                return;
            };
            let (first_area, divider, second_area) = match dir {
                SplitDir::Horizontal => (
                    Rect::new(area.col, area.row, near, area.rows),
                    Rect::new(area.col + near, area.row, 1, area.rows),
                    Rect::new(area.col + near + 1, area.row, far, area.rows),
                ),
                SplitDir::Vertical => (
                    Rect::new(area.col, area.row, area.cols, near),
                    Rect::new(area.col, area.row + near, area.cols, 1),
                    Rect::new(area.col, area.row + near + 1, area.cols, far),
                ),
            };
            tile_node(first, first_area, tiling);
            // Pushed BETWEEN the two recursions so the dividers of a sub-tree that is drawn first
            // come first — the order the caller paints in, though nothing depends on it: the
            // partition means no two dividers ever contend for a cell.
            tiling.dividers.push(Divider {
                area: divider,
                dir: *dir,
            });
            tile_node(second, second_area, tiling);
        }
    }
}

/// The share a split opens at when its recorded one cannot be used — the same even default the
/// host mints a fresh divider with.
const EVEN: f32 = 0.5;

/// Divide `extent` cells between a split's two children, reserving one cell for the divider.
///
/// `None` when the region cannot hold both: fewer than three cells leaves nothing for one side once
/// the divider is taken. The caller gives the whole region to `first` (module docs).
///
/// # The rounding rule, stated once
///
/// `first` takes `floor(avail * ratio)` and `second` takes the REMAINDER — a fixed side, so the
/// odd cell always lands in the same place and a recomputation of the same tree at the same size
/// cannot move a boundary. Both are then clamped to at least one cell, because a pane with no
/// columns is not a pane: an arrangement whose ratio rounds a side to nothing still shows it,
/// one cell wide, rather than silently dropping a pane the region had room for.
///
/// A ratio that is not a share — negative, past one, or `NaN` — falls back to [`EVEN`]. The host
/// validates ratios before it stores them, so this is unreachable through the wire; it is here
/// because the function must be total over `f32`, and a `NaN` that reached the cast would produce
/// a zero-width pane instead of a visible refusal.
fn divide(extent: u16, ratio: f32) -> Option<(u16, u16)> {
    // The divider's own cell, taken before either child is measured.
    let avail = extent.checked_sub(1)?;
    if avail < 2 {
        return None;
    }
    let ratio = if ratio.is_finite() && (0.0..=1.0).contains(&ratio) {
        ratio
    } else {
        EVEN
    };
    // Saturating by construction: `ratio` is within `0.0..=1.0` and `avail` fits a `u16`, so the
    // product cannot exceed one either, and `floor` cannot lift it.
    let near = (f32::from(avail) * ratio).floor() as u16;
    let near = near.clamp(1, avail - 1);
    Some((near, avail - near))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A leaf node for pane `id`.
    fn leaf(id: u64) -> LayoutNodeWire {
        LayoutNodeWire::Leaf(PaneId(id))
    }

    /// A split of `first` and `second` on `dir` at `ratio`, with no divider identity — which is
    /// what a client-minted node carries, and irrelevant to geometry either way.
    fn split(
        dir: SplitDir,
        ratio: f32,
        first: LayoutNodeWire,
        second: LayoutNodeWire,
    ) -> LayoutWire {
        LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: None,
                dir,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
        }
    }

    /// One pane's rectangle, panicking with the whole tiling when it is absent — so a failure names
    /// what was laid out instead of just `None`.
    fn area(tiling: &Tiling, id: u64) -> Rect {
        tiling
            .area_of(PaneId(id))
            .unwrap_or_else(|| panic!("pane {id} is not in {tiling:?}"))
    }

    /// Every cell of `area` and who owns it: `Some(pane)`, `None` for a divider, and a panic for a
    /// cell claimed twice — which is the partition property expressed as a function rather than as
    /// an argument.
    fn owners(tiling: &Tiling, area: Rect) -> Vec<Vec<Option<PaneId>>> {
        let mut owners = vec![vec![None; usize::from(area.cols)]; usize::from(area.rows)];
        let mut claimed = vec![vec![false; usize::from(area.cols)]; usize::from(area.rows)];
        let mut claim = |rect: Rect, owner: Option<PaneId>| {
            for row in rect.row..rect.row + rect.rows {
                for col in rect.col..rect.col + rect.cols {
                    let (r, c) = (usize::from(row), usize::from(col));
                    assert!(!claimed[r][c], "cell ({col}, {row}) claimed twice");
                    claimed[r][c] = true;
                    owners[r][c] = owner;
                }
            }
        };
        for pane in &tiling.panes {
            claim(pane.area, Some(pane.pane));
        }
        for divider in &tiling.dividers {
            claim(divider.area, None);
        }
        for (row, line) in claimed.iter().enumerate() {
            for (col, held) in line.iter().enumerate() {
                assert!(held, "cell ({col}, {row}) belongs to nothing");
            }
        }
        owners
    }

    /// A window tiling nothing lays out nothing — the honest zero-pane state, not an error.
    #[test]
    fn an_empty_arrangement_tiles_nothing() {
        let tiling = tile(&LayoutWire::default(), Rect::screen(80, 24));
        assert!(tiling.panes.is_empty());
        assert!(tiling.dividers.is_empty());
    }

    /// One pane takes the whole terminal, with no divider drawn for a split that is not there.
    #[test]
    fn a_sole_pane_takes_the_whole_area() {
        let tree = LayoutWire {
            root: Some(leaf(7)),
        };
        let tiling = tile(&tree, Rect::screen(80, 24));
        assert_eq!(area(&tiling, 7), Rect::screen(80, 24));
        assert!(tiling.dividers.is_empty());
    }

    /// **THE DESIGN'S OWN EXAMPLE.** A 0.5 ratio over 81 columns is 40 and 40 with a divider
    /// column — not 40 and 41, and not 41 and 40.
    ///
    /// REVERT-PROOF for the divider's reservation, MEASURED: drop the `checked_sub(1)` in
    /// [`divide`] and the second half comes out 41 columns wide starting at column 41 — a pane
    /// running one column PAST the screen's own edge, because the two halves and the divider then
    /// claim 82 cells of an 81-column terminal. (Ten of this module's tests fail with it removed;
    /// this is the one that names the number.)
    #[test]
    fn an_even_split_of_an_odd_width_is_forty_and_forty_with_a_divider() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(81, 24));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 40, 24));
        assert_eq!(area(&tiling, 1), Rect::new(41, 0, 40, 24));
        assert_eq!(
            tiling.dividers,
            vec![Divider {
                area: Rect::new(40, 0, 1, 24),
                dir: SplitDir::Horizontal,
            }],
        );
    }

    /// The odd cell goes to a FIXED side, so the same tree at the same size always lays out the
    /// same way. An 80-column screen leaves 79 to divide: 39 and 40, with the remainder on the
    /// second — never alternating.
    #[test]
    fn the_odd_cell_always_lands_on_the_second_side() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let first = tile(&tree, Rect::screen(80, 24));
        assert_eq!(area(&first, 0), Rect::new(0, 0, 39, 24));
        assert_eq!(area(&first, 1), Rect::new(40, 0, 40, 24));
        // Re-run it: a boundary that moved between two identical calls would redraw the screen.
        assert_eq!(tile(&tree, Rect::screen(80, 24)), first);
    }

    /// A vertical split divides ROWS and reserves a row, mirroring the horizontal case on the other
    /// axis. Asserted separately because a layouter that read `cols` for both axes passes every
    /// horizontal test there is.
    #[test]
    fn a_vertical_split_divides_rows_and_reserves_one() {
        let tree = split(SplitDir::Vertical, 0.5, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(80, 25));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 80, 12));
        assert_eq!(area(&tiling, 1), Rect::new(0, 13, 80, 12));
        assert_eq!(
            tiling.dividers,
            vec![Divider {
                area: Rect::new(0, 12, 80, 1),
                dir: SplitDir::Vertical,
            }],
        );
    }

    /// The host's ratio is honoured rather than being an even split with extra steps — a 0.25 share
    /// puts a quarter of the divisible columns on the left.
    #[test]
    fn the_ratio_decides_the_share() {
        let tree = split(SplitDir::Horizontal, 0.25, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(41, 10));
        // 41 columns, one for the divider, 40 to divide: floor(40 * 0.25) = 10.
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 10, 10));
        assert_eq!(area(&tiling, 1), Rect::new(11, 0, 30, 10));
    }

    /// **THE PARTITION, over a nested tree on both axes.** Every cell of the terminal belongs to
    /// exactly one pane or one divider — which is what lets the client repaint without clearing.
    ///
    /// REVERT-PROOF, measured: give the second child of a horizontal split `area.col + near` as its
    /// origin (forgetting the divider) and `owners` panics on a cell claimed twice.
    #[test]
    fn panes_and_dividers_partition_the_area_exactly() {
        let tree = LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: None,
                dir: SplitDir::Vertical,
                ratio: 0.6,
                first: Box::new(LayoutNodeWire::Split {
                    id: None,
                    dir: SplitDir::Horizontal,
                    ratio: 0.5,
                    first: Box::new(leaf(0)),
                    second: Box::new(leaf(1)),
                }),
                second: Box::new(leaf(2)),
            }),
        };
        let area = Rect::screen(37, 19);
        let tiling = tile(&tree, area);
        let owners = owners(&tiling, area);
        // The assertions `owners` makes are the point; this one pins that it saw a real tiling
        // rather than an empty one it could vacuously agree with.
        assert_eq!(tiling.panes.len(), 3);
        assert_eq!(owners[0][0], Some(PaneId(0)));
    }

    /// Panes come out in PAINT ORDER — the order the arrangement reads, which is what makes
    /// cycling focus with `prefix o` move the way a user expects.
    #[test]
    fn panes_come_out_in_paint_order() {
        let tree = LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: None,
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(leaf(0)),
                second: Box::new(LayoutNodeWire::Split {
                    id: None,
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    first: Box::new(leaf(1)),
                    second: Box::new(leaf(2)),
                }),
            }),
        };
        let tiling = tile(&tree, Rect::screen(80, 24));
        assert_eq!(
            tiling
                .panes
                .iter()
                .map(|held| held.pane)
                .collect::<Vec<_>>(),
            vec![PaneId(0), PaneId(1), PaneId(2)],
        );
    }

    /// A ratio that rounds a side to nothing still shows it, one cell wide. Without the clamp a
    /// 0.02 share over 20 columns would give the first pane zero, and a zero-column pane is not a
    /// pane — the host refuses to resize one, so it would paint nothing and never reflow.
    #[test]
    fn a_share_that_rounds_to_nothing_still_gets_a_cell() {
        let tree = split(SplitDir::Horizontal, 0.02, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(20, 5));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 1, 5));
        assert_eq!(area(&tiling, 1), Rect::new(2, 0, 18, 5));
    }

    /// ...and the same at the other end of the range.
    #[test]
    fn a_share_that_rounds_to_everything_leaves_the_other_side_a_cell() {
        let tree = split(SplitDir::Horizontal, 1.0, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(20, 5));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 18, 5));
        assert_eq!(area(&tiling, 1), Rect::new(19, 0, 1, 5));
    }

    /// Three cells is the smallest region that can hold two panes: one each and one divider.
    #[test]
    fn three_cells_is_exactly_enough_for_two_panes() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(3, 4));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 1, 4));
        assert_eq!(area(&tiling, 1), Rect::new(2, 0, 1, 4));
    }

    /// Two cells is not: the region goes to the first child whole and the second is OMITTED — no
    /// rectangle, so the client neither paints nor resizes it, and the pane stays alive.
    #[test]
    fn a_region_too_small_for_two_shows_the_first_and_drops_the_second() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(2, 4));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 2, 4));
        assert_eq!(tiling.area_of(PaneId(1)), None);
        assert!(tiling.dividers.is_empty(), "and no divider is drawn");
    }

    /// The drop recurses: a first child that is itself a split resolves to whichever single pane
    /// its own region can show, rather than to nothing.
    #[test]
    fn the_too_small_rule_recurses_into_the_surviving_child() {
        let tree = LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: None,
                dir: SplitDir::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNodeWire::Split {
                    id: None,
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    first: Box::new(leaf(0)),
                    second: Box::new(leaf(1)),
                }),
                second: Box::new(leaf(2)),
            }),
        };
        let tiling = tile(&tree, Rect::screen(10, 2));
        assert_eq!(area(&tiling, 0), Rect::new(0, 0, 10, 2));
        assert_eq!(tiling.panes.len(), 1, "the other two do not fit");
    }

    /// A terminal that reports no size lays out nothing rather than a pane with no cells — the
    /// zero-dimension the host's resize refuses.
    #[test]
    fn a_zero_area_tiles_nothing() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        assert_eq!(tile(&tree, Rect::screen(0, 24)), Tiling::default());
        assert_eq!(tile(&tree, Rect::screen(80, 0)), Tiling::default());
    }

    /// A ratio outside `0.0..=1.0` — or `NaN`, which no comparison catches — falls back to an even
    /// share instead of producing a pane with no columns. Unreachable through the host, which
    /// validates ratios; the function is total over `f32` regardless.
    #[test]
    fn a_ratio_that_is_not_a_share_falls_back_to_even() {
        for ratio in [f32::NAN, -1.0, 2.0, f32::INFINITY] {
            let tree = split(SplitDir::Horizontal, ratio, leaf(0), leaf(1));
            let tiling = tile(&tree, Rect::screen(21, 5));
            assert_eq!(area(&tiling, 0), Rect::new(0, 0, 10, 5), "ratio {ratio}");
            assert_eq!(area(&tiling, 1), Rect::new(11, 0, 10, 5), "ratio {ratio}");
        }
    }

    /// Cycling wraps, and an unknown pane lands on the first — the total answer to "where does
    /// focus go next" that a client needs when the pane it was on has just left the tiling.
    #[test]
    fn cycling_wraps_and_an_unknown_pane_lands_on_the_first() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(4), leaf(9));
        let tiling = tile(&tree, Rect::screen(80, 24));
        assert_eq!(tiling.next_after(PaneId(4)), Some(PaneId(9)));
        assert_eq!(tiling.next_after(PaneId(9)), Some(PaneId(4)), "wraps");
        assert_eq!(tiling.next_after(PaneId(99)), Some(PaneId(4)));
        assert_eq!(Tiling::default().next_after(PaneId(4)), None);
    }

    /// `holds` answers for the cells inside a rectangle and refuses the ones past its edges — the
    /// half-open bound a client clipping a pane's cells depends on.
    #[test]
    fn a_rect_holds_its_own_cells_and_no_others() {
        let rect = Rect::new(10, 5, 3, 2);
        assert!(rect.holds(10, 5) && rect.holds(12, 6));
        assert!(!rect.holds(13, 6), "one past the right edge");
        assert!(!rect.holds(12, 7), "one past the bottom edge");
        assert!(!rect.holds(9, 5) && !rect.holds(10, 4));
    }

    /// A screen cell resolves to a pane AND to that pane's own coordinates — and the second half is
    /// only visible off the origin.
    ///
    /// **The pane at (0, 0) cannot fail this**, which is exactly why the assertion is made against
    /// the SECOND one: there, screen and pane coordinates coincide, so a client that forwarded the
    /// screen cell unchanged would look correct in every single-pane arrangement and put every
    /// click in the wrong place the moment a split appeared. The subtraction is the whole content
    /// of [`Tiling::pane_at`] beyond the lookup.
    #[test]
    fn a_cell_names_its_pane_and_its_place_inside_it() {
        // 21 columns: 10 | divider | 10, the partition the rounding test above pins.
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(21, 5));

        assert_eq!(
            tiling.pane_at(0, 0),
            Some((PaneId(0), 0, 0)),
            "the origin is the first pane's own origin",
        );
        assert_eq!(
            tiling.pane_at(9, 4),
            Some((PaneId(0), 9, 4)),
            "the first pane's far corner",
        );
        assert_eq!(
            tiling.pane_at(11, 0),
            Some((PaneId(1), 0, 0)),
            "the SECOND pane's first column is screen column 11, and it must arrive as 0",
        );
        assert_eq!(
            tiling.pane_at(20, 3),
            Some((PaneId(1), 9, 3)),
            "and its far corner is pane-local (9, 3), not screen (20, 3)",
        );
    }

    /// A divider column belongs to no pane, and neither does a cell off the arrangement. Both
    /// answer `None` because neither can be forwarded: there is no child whose grid holds them.
    ///
    /// The divider is the one worth pinning. It sits BETWEEN two rectangles, so a lookup written as
    /// "the last pane starting at or before this column" would hand it to the pane on the left and
    /// deliver a click one column outside that child's own grid.
    #[test]
    fn a_divider_cell_belongs_to_no_pane() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let tiling = tile(&tree, Rect::screen(21, 5));

        assert_eq!(tiling.pane_at(10, 2), None, "the divider column");
        assert_eq!(tiling.pane_at(21, 0), None, "one past the right edge");
        assert_eq!(tiling.pane_at(0, 5), None, "one past the bottom edge");
        assert_eq!(Tiling::default().pane_at(0, 0), None, "nothing is tiled");
    }
}
