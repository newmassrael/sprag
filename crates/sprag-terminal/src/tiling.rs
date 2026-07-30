//! The host's logical arrangement, in CHARACTER CELLS — the ONE authority on how big a pane is.
//!
//! [`tile`] is a pure function of an arrangement and a WINDOW, and every client runs the same one:
//! a pane's `(cols, rows)` is decided here, by the tree and the window size the daemon arbitrated,
//! and by nothing a single client happens to be. That is the whole point of the function living in
//! this crate rather than in a frontend.
//!
//! # Why it is shared, and what it was before
//!
//! It used to live in `sprag-tui`, opposite a `sprag-gui` that derived each pane's cell size from
//! that pane's own measured PIXEL rect. Two clients then answered "how wide is this pane" with two
//! numbers, and the pane took whichever one was written last — a size the other client was never
//! told about, because a resize does not bump the scene. The loser went on painting a grid it had
//! the wrong dimensions for. One pane cannot have two sizes, so there cannot be two functions that
//! decide it.
//!
//! What each client still owns is where the result goes on ITS surface: `sprag-gui` places the
//! panes as pixels through pinion's dock, `sprag-tui` places them as cells on a terminal. Neither
//! places them in a size of its own choosing.
//!
//! Nor does either APPLY the result any more. A pane's size is `tile(tree, window)` and both
//! arguments are the daemon's, so the daemon derives it (`sprag_host::window::retile`) and a client
//! sizes a pane only when the daemon has no window to derive one from. What a client contributes is
//! the other direction: how many cells IT can give the arrangement, which is [`fit_window`].
//!
//! # The window is not the client's screen
//!
//! The area passed in is the session's arbitrated window (`sprag_host::WindowSize`, tmux's
//! `window-size` — named in prose because the policy lives a layer up, in the crate that reads the
//! user's file), which is a fact about every attached client rather than about the caller. A
//! client with more room than the window paints the tiling into part of its surface and leaves the
//! rest as background; a client with less sees only what fits. Both are showing the same
//! arrangement at the same size, which is what makes two clients of a session agree.
//!
//! # Rounding is the whole difficulty, and it is not a detail here
//!
//! Pixels round at sub-cell granularity and nobody notices. Character cells round at the
//! granularity the user counts: a boundary that lands one column left of where it did last frame is
//! a whole screen redraw, and a user watching a 40/41 split flip to 41/40 as they type is watching a
//! bug. So the division here is INTEGER and total — one private `divide` states it once, every
//! split goes through it, and the same inputs always yield the same cells.
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
//! The arrangement is the host's and can hold more panes than the window has rows. A split whose
//! axis cannot give both children a cell and a divider one gives the whole region to `first`, and
//! the second child (with everything under it) is OMITTED from the tiling: it has no rectangle, it
//! is not painted, and it is not resized. Dropping it is the honest answer — a zero-column pane is
//! not a pane, and the host is right to refuse a resize to one — but the pane is still ALIVE and
//! still the session's, so a window too small for its arrangement loses the view, never the work.
//!
//! Because the input is the arbitrated window, that loss is now the same for every client of a
//! session instead of per-terminal: a pane the window cannot hold is unpainted everywhere, rather
//! than present on the large client and missing on the small one. A client whose own surface is
//! smaller than the window still shows the panes it has room for — it is cropping a tiling it
//! agrees with, which is a different thing from computing a smaller one.

use crate::{LayoutNodeWire, LayoutWire, PaneId, SplitDir, SplitId};

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

    /// The part of this rectangle that also lies inside `bounds`, or `None` when they do not
    /// overlap at all.
    ///
    /// What a client paints with when its own surface is smaller than the arbitrated window: the
    /// tiling is computed over the WINDOW, so a rectangle can extend past the screen showing it, and
    /// drawing it whole would write cells the terminal does not have. Clipping here keeps the two
    /// facts separate — a pane is RESIZED to its share of the window and PAINTED to what fits —
    /// which is what lets a small client show part of a large window instead of disagreeing about
    /// how large the window is.
    #[must_use]
    pub fn intersect(&self, bounds: Rect) -> Option<Rect> {
        let col = self.col.max(bounds.col);
        let row = self.row.max(bounds.row);
        let right = self
            .col
            .saturating_add(self.cols)
            .min(bounds.col.saturating_add(bounds.cols));
        let bottom = self
            .row
            .saturating_add(self.rows)
            .min(bounds.row.saturating_add(bounds.rows));
        let clipped = Self::new(
            col,
            row,
            right.saturating_sub(col),
            bottom.saturating_sub(row),
        );
        (!clipped.is_empty()).then_some(clipped)
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
    /// The split's durable identity, or `None` for one the host has not named yet.
    ///
    /// Carried so a client can act on the divider it is POINTING AT rather than on a position in a
    /// walk: the tree is re-read every frame, and an index into it means a different node the
    /// moment anything splits or closes. The wire's own docs call this out — a client "keys its
    /// live drag ratio on them" — so identity was always the intended handle, and this is the
    /// reader that finally needs it.
    pub id: Option<SplitId>,
    /// The whole region this split divides, which is what a new ratio is measured AGAINST.
    ///
    /// Not derivable from [`Divider::area`]: the divider is one cell thick and says nothing about
    /// how far its region extends on either side. A drag needs both — where the pointer is, and
    /// what fraction of what.
    pub region: Rect,
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
    /// [`LayoutTree::panes`](crate::LayoutTree::panes) reports, so a client cycling
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
    /// `sprag_input::MouseInput` is defined in: a child knows only its own grid, and
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

    /// The divider on screen cell `(col, row)`, if the cell is one.
    ///
    /// The other half of [`Tiling::pane_at`], and the reason both can be total: the tiling is an
    /// exact partition, so a cell answers one of these two and never both.
    #[must_use]
    pub fn divider_at(&self, col: u16, row: u16) -> Option<Divider> {
        self.dividers
            .iter()
            .copied()
            .find(|line| line.area.holds(col, row))
    }
}

impl Divider {
    /// The ratio that would put this divider's cell at `(col, row)` — what a drag to that cell
    /// means, or `None` when the cell is off the axis or the region cannot hold the move.
    ///
    /// # It is defined as the INVERSE of the layouter's division, not as a fraction of the region
    ///
    /// The obvious spelling — the pointer's distance along the region over the region's extent — is
    /// wrong by up to a cell, because the division floors and reserves the divider's own column. The
    /// ratio computed here is the one that places the divider exactly where the
    /// pointer is, which is the only definition under which a drag TRACKS the pointer rather than
    /// drifting away from it a cell at a time.
    ///
    /// The half-cell is what makes the inverse robust: the layouter computes `floor(avail * ratio)`, so
    /// asking for `(near + 0.5) / avail` lands strictly inside the interval that floors to `near`
    /// rather than on its edge, where a float's last bit decides the answer.
    ///
    /// Both sides keep at least one cell, so a drag to the region's own edge stops at the last
    /// arrangement that is still two panes rather than collapsing one to nothing.
    #[must_use]
    pub fn ratio_at(&self, col: u16, row: u16) -> Option<f32> {
        let (extent, along, origin) = match self.dir {
            SplitDir::Horizontal => (self.region.cols, col, self.region.col),
            SplitDir::Vertical => (self.region.rows, row, self.region.row),
        };
        // One cell for the divider and at least one for each child: below three there is no move
        // to make, and `divide` would already be refusing to show two panes.
        let avail = extent.checked_sub(1).filter(|avail| *avail >= 2)?;
        let near = along.checked_sub(origin)?.clamp(1, avail - 1);
        Some((f32::from(near) + 0.5) / f32::from(avail))
    }
}

/// `tree` with the split identified by `id` set to `ratio`, or `None` when no node carries that id.
///
/// Answering `None` rather than returning the tree unchanged is what keeps a caller honest: a drag
/// that found no node has lost the divider it was moving — the arrangement changed under it — and
/// writing an unmodified tree back would be a WRITE that looks like a successful move.
///
/// Pure, and a copy rather than an in-place edit, because the tree a client holds is the host's
/// last answer: mutating it would leave the client believing an arrangement the host may refuse.
#[must_use]
pub fn with_ratio(tree: &LayoutWire, id: SplitId, ratio: f32) -> Option<LayoutWire> {
    fn edit(node: &LayoutNodeWire, id: SplitId, ratio: f32) -> Option<LayoutNodeWire> {
        let LayoutNodeWire::Split {
            id: node_id,
            dir,
            ratio: was,
            first,
            second,
        } = node
        else {
            return None;
        };
        if *node_id == Some(id) {
            return Some(LayoutNodeWire::Split {
                id: *node_id,
                dir: *dir,
                ratio,
                first: first.clone(),
                second: second.clone(),
            });
        }
        let rebuild = |first: Box<LayoutNodeWire>, second: Box<LayoutNodeWire>| {
            Some(LayoutNodeWire::Split {
                id: *node_id,
                dir: *dir,
                ratio: *was,
                first,
                second,
            })
        };
        if let Some(edited) = edit(first, id, ratio) {
            return rebuild(Box::new(edited), second.clone());
        }
        edit(second, id, ratio).and_then(|edited| rebuild(first.clone(), Box::new(edited)))
    }

    let root = tree.root.as_ref()?;
    Some(LayoutWire {
        root: Some(edit(root, id, ratio)?),
    })
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
            id,
            dir,
            ratio,
            first,
            second,
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
                id: *id,
                region: area,
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

/// The largest window whose tiling gives every pane of `tree` no more cells than `measured` says
/// that pane's surface can hold — what a client REPORTS as the area it has to give an arrangement.
///
/// `None` when the tree holds no pane, when a pane has no measurement yet, or when no window at all
/// satisfies the tree (a surface too small to show every pane).
///
/// # Why a client cannot just report its surface
///
/// A terminal can: its whole screen is cells, so the arrangement gets the screen and the only
/// subtraction is the divider [`tile`] already reserves. A GUI cannot, because its chrome is PER
/// PANE — every dock panel carries a header, and a scrollbar, and the number of those along an axis
/// depends on the shape of the tiling rather than on the window. So the cells a GUI has to give the
/// arrangement are not its surface minus a constant, and reporting the surface would size every
/// pane's grid larger than the widget drawn for it and clip what does not fit.
///
/// What such a client CAN state is what each pane's own surface measured. This folds those back
/// into the one number the arbitration takes, and it does so through [`tile`] itself: the answer is
/// defined by the same function that will consume it, so there is no second geometry model here to
/// drift out of step with the first.
///
/// # The search, and why it is a walk DOWN rather than a halving
///
/// One dimension at a time, because [`tile`] keeps them independent: a horizontal split divides
/// the columns and passes the rows through, and a vertical one the reverse. So the columns are
/// searched with the rows held at the bound, and then the rows at the columns that answered.
///
/// The obvious halving is WRONG here, and the reason is a property of [`tile`] worth stating: a
/// window too small to hold both children of a split does not shrink them, it DROPS one and gives
/// its region to the survivor — which then holds more cells than it did at the larger window. So
/// "every pane fits its measurement" is not downward-closed; it is true at the answer, false just
/// below it where a pane is dropped and its sibling swells, and true again at the very bottom. A
/// bisection can land in that hole. Walking down from the bound returns the first window that
/// satisfies the property, which IS the largest one, with no monotonicity assumed at all — and the
/// bound is a few hundred cells, so the walk costs less than the layout that triggered it.
///
/// The upper bound is the fold of the measurements themselves — a window that hands each pane
/// exactly what it measured, plus a cell per divider. Nothing above it can fit: the panes and
/// dividers partition the window exactly, so one more cell in the window is one more cell in some
/// pane than its surface has.
#[must_use]
pub fn fit_window(tree: &LayoutWire, measured: &[(PaneId, (u16, u16))]) -> Option<(u16, u16)> {
    let root = tree.root.as_ref()?;
    let cells = |pane: PaneId| {
        measured
            .iter()
            .find(|(held, _)| *held == pane)
            .map(|(_, cells)| *cells)
    };
    let bound = fold_window(root, &cells)?;
    let panes = tree.panes().len();
    // A pane the window cannot hold is absent from the tiling, so "every pane is present" is part
    // of the question rather than a check to run after it: a window that shows two of three panes
    // is not an area this client can give the arrangement, however well those two fit.
    let holds = |window: (u16, u16), axis: fn(Rect) -> u16, limit: fn((u16, u16)) -> u16| {
        let tiling = tile(tree, Rect::screen(window.0, window.1));
        tiling.panes.len() == panes
            && tiling
                .panes
                .iter()
                .all(|held| cells(held.pane).is_some_and(|c| axis(held.area) <= limit(c)))
    };
    let cols = walk_down(bound.0, |cols| {
        holds((cols, bound.1), |area| area.cols, |cells| cells.0)
    })?;
    let rows = walk_down(bound.1, |rows| {
        holds((cols, rows), |area| area.rows, |cells| cells.1)
    })?;
    Some((cols, rows))
}

/// The largest `value` in `1..=bound` for which `fits` holds, or `None` when none does.
///
/// Downward, one at a time, and deliberately not a halving — see [`fit_window`] for the property
/// of [`tile`] that puts a hole in this predicate.
fn walk_down(bound: u16, fits: impl Fn(u16) -> bool) -> Option<u16> {
    (1..=bound).rev().find(|value| fits(*value))
}

/// A window that hands every pane under `node` exactly what it measured — [`fit_window`]'s upper
/// bound, and `None` if any pane has no measurement.
///
/// A split's window is its children's along the axis it divides, plus the one cell [`tile`] reserves
/// for the divider; across that axis it is the SMALLER of the two, since a window wider than the
/// narrower child would over-fill it.
fn fold_window(
    node: &LayoutNodeWire,
    cells: &impl Fn(PaneId) -> Option<(u16, u16)>,
) -> Option<(u16, u16)> {
    match node {
        LayoutNodeWire::Leaf(pane) => cells(*pane),
        LayoutNodeWire::Split {
            dir, first, second, ..
        } => {
            let first = fold_window(first, cells)?;
            let second = fold_window(second, cells)?;
            Some(match dir {
                SplitDir::Horizontal => (
                    first.0.saturating_add(1).saturating_add(second.0),
                    first.1.min(second.1),
                ),
                SplitDir::Vertical => (
                    first.0.min(second.0),
                    first.1.saturating_add(1).saturating_add(second.1),
                ),
            })
        }
    }
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

    /// `tree` with its root split carrying `id` — the shape a HOST-sent tree has, since a client
    /// mints nodes without one and the host names them.
    fn identified(tree: &LayoutWire, id: SplitId) -> LayoutWire {
        let Some(LayoutNodeWire::Split {
            dir,
            ratio,
            first,
            second,
            ..
        }) = tree.root.clone()
        else {
            panic!("the fixture's root is a split");
        };
        LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: Some(id),
                dir,
                ratio,
                first,
                second,
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
                id: None,
                region: Rect::screen(81, 24),
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
                id: None,
                region: Rect::screen(80, 25),
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

    /// A drag to a cell yields the ratio that puts the divider ON that cell — the property that
    /// makes a drag TRACK the pointer instead of drifting away from it a cell at a time.
    ///
    /// Asserted by ROUND TRIP through [`tile`] rather than against a number, because the number is
    /// not the claim: any ratio is "correct" until you ask where it lands. Every reachable column
    /// is checked, so an off-by-one at either end has nowhere to hide.
    #[test]
    fn a_drag_puts_the_divider_where_the_pointer_is() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let screen = Rect::screen(21, 5);
        let divider = tile(&tree, screen).dividers[0];
        let id = SplitId(7);

        for column in 1..=19 {
            let ratio = divider
                .ratio_at(column, 0)
                .expect("a column inside the region has a ratio");
            let moved = tile(
                &with_ratio(&identified(&tree, id), id, ratio).expect("the split is there"),
                screen,
            );
            assert_eq!(
                moved.dividers[0].area.col, column,
                "a drag to column {column} lands there (ratio {ratio})",
            );
        }
    }

    /// A drag past either edge stops at the last arrangement that is still TWO panes: the clamp is
    /// what keeps a careless gesture from collapsing a pane to nothing, which the layouter would
    /// then drop from the tiling entirely.
    #[test]
    fn a_drag_off_the_end_stops_at_one_cell() {
        let tree = split(SplitDir::Horizontal, 0.5, leaf(0), leaf(1));
        let screen = Rect::screen(21, 5);
        let divider = tile(&tree, screen).dividers[0];
        let id = SplitId(7);
        let landed = |col| {
            let ratio = divider.ratio_at(col, 0).expect("a ratio");
            tile(
                &with_ratio(&identified(&tree, id), id, ratio).expect("the split is there"),
                screen,
            )
            .dividers[0]
                .area
                .col
        };
        assert_eq!(landed(0), 1, "the far left keeps the first pane a column");
        assert_eq!(landed(20), 19, "and the far right keeps the second one");
    }

    /// The edit finds the split by IDENTITY, and answers `None` when the tree does not carry it —
    /// which is what tells a caller its divider is gone rather than letting it write an unchanged
    /// tree back and read that as a successful move.
    #[test]
    fn a_ratio_is_written_by_identity_or_not_at_all() {
        let id = SplitId(3);
        let tree = identified(&split(SplitDir::Vertical, 0.5, leaf(0), leaf(1)), id);
        let moved = with_ratio(&tree, id, 0.25).expect("the split is there");
        let Some(LayoutNodeWire::Split { ratio, .. }) = moved.root else {
            panic!("the root is still a split");
        };
        assert!((ratio - 0.25).abs() < f32::EPSILON, "the ratio moved");
        assert!(
            with_ratio(&tree, SplitId(99), 0.25).is_none(),
            "a split this tree does not carry is not silently a no-op",
        );
    }

    /// A NESTED split is reachable too — the edit walks, it does not only look at the root.
    #[test]
    fn a_ratio_reaches_a_nested_split() {
        let inner = SplitId(11);
        let tree = LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: Some(SplitId(10)),
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(leaf(0)),
                second: Box::new(LayoutNodeWire::Split {
                    id: Some(inner),
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    first: Box::new(leaf(1)),
                    second: Box::new(leaf(2)),
                }),
            }),
        };
        let moved = with_ratio(&tree, inner, 0.8).expect("the nested split is there");
        let Some(LayoutNodeWire::Split { first, second, .. }) = moved.root else {
            panic!("the root is still a split");
        };
        assert_eq!(*first, leaf(0), "the untouched sibling is carried through");
        let LayoutNodeWire::Split { ratio, .. } = *second else {
            panic!("the second child is still a split");
        };
        assert!((ratio - 0.8).abs() < f32::EPSILON);
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

    #[test]
    fn intersect_crops_to_the_bounds_and_reports_no_overlap() {
        let screen = Rect::screen(80, 24);
        // Wholly inside: unchanged.
        let inside = Rect::new(10, 5, 20, 10);
        assert_eq!(inside.intersect(screen), Some(inside));
        // Straddling the right and bottom edges: cropped to what the screen has, ORIGIN kept — a
        // clip that moved the rectangle would paint the pane in the wrong place.
        assert_eq!(
            Rect::new(70, 20, 30, 10).intersect(screen),
            Some(Rect::new(70, 20, 10, 4))
        );
        // Wholly outside (a pane the window gave a rectangle this terminal cannot reach at all):
        // absent rather than empty, so a caller cannot paint a zero-cell rectangle by accident.
        assert_eq!(Rect::new(80, 0, 10, 10).intersect(screen), None);
        assert_eq!(Rect::new(0, 24, 10, 10).intersect(screen), None);
        // A window SMALLER than the screen leaves the tiling untouched: the caller's bounds are the
        // screen, and cropping never grows anything.
        assert_eq!(
            Rect::new(0, 0, 40, 12).intersect(screen),
            Some(Rect::new(0, 0, 40, 12))
        );
    }

    /// What every `fit_window` case must be able to say: lay the tree out over the answer and no
    /// pane got more cells than its surface measured, and none was dropped.
    fn every_pane_fits(tree: &LayoutWire, measured: &[(PaneId, (u16, u16))], window: (u16, u16)) {
        let tiling = tile(tree, Rect::screen(window.0, window.1));
        assert_eq!(
            tiling.panes.len(),
            measured.len(),
            "a window that drops a pane is not one this client can give the arrangement"
        );
        for held in &tiling.panes {
            let (cols, rows) = measured
                .iter()
                .find(|(pane, _)| *pane == held.pane)
                .expect("the tiling holds only panes of the tree")
                .1;
            assert!(
                held.area.cols <= cols && held.area.rows <= rows,
                "pane {:?} was given {}x{} for a surface measuring {cols}x{rows}",
                held.pane,
                held.area.cols,
                held.area.rows
            );
        }
    }

    #[test]
    fn a_lone_pane_reports_exactly_what_it_measured() {
        // No divider, no sibling to be the smaller of: the one pane's surface IS the window, which
        // is the case that must not move a solo user's panes.
        let tree = LayoutWire {
            root: Some(leaf(1)),
        };
        let measured = [(PaneId(1), (38, 17))];
        assert_eq!(fit_window(&tree, &measured), Some((38, 17)));
    }

    #[test]
    fn two_side_by_side_report_the_divider_between_them() {
        // The measured GUI: two 380x510 grids at a 10x30 cell, a 28px header each, side by side.
        // 38 + 38 is not the answer — the divider's cell is part of the window, and a client that
        // forgot it would be handed back 37 columns for a pane it measured at 38.
        let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
        let measured = [(PaneId(1), (38, 17)), (PaneId(2), (38, 17))];
        assert_eq!(fit_window(&tree, &measured), Some((77, 17)));
        // And laying that back out returns each pane to EXACTLY what it measured, which is the
        // round trip that makes a solo GUI's panes identical to what its own pixels gave them.
        let tiling = tile(&tree, Rect::screen(77, 17));
        assert_eq!(tiling.panes[0].area, Rect::new(0, 0, 38, 17));
        assert_eq!(tiling.panes[1].area, Rect::new(39, 0, 38, 17));
    }

    #[test]
    fn the_smaller_child_decides_the_axis_the_split_does_not_divide() {
        // A horizontal split shares the columns and PASSES THE ROWS THROUGH, so a window taller
        // than the shorter pane would over-fill it. 17 and 9 is 9, not 17 and not 13.
        let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
        let measured = [(PaneId(1), (38, 17)), (PaneId(2), (38, 9))];
        assert_eq!(fit_window(&tree, &measured), Some((77, 9)));
        every_pane_fits(&tree, &measured, (77, 9));
    }

    #[test]
    fn a_vertical_split_stacks_the_rows_the_same_way() {
        // The case R241 gave as the reason a GUI cannot report its surface: two stacked panes lose
        // TWO headers, and the window's rows are the two grids plus the divider — a number that
        // depends on the shape of the tiling, which is why it is folded rather than subtracted.
        let tree = split(SplitDir::Vertical, 0.5, leaf(1), leaf(2));
        let measured = [(PaneId(1), (38, 7)), (PaneId(2), (38, 7))];
        assert_eq!(fit_window(&tree, &measured), Some((38, 15)));
        every_pane_fits(&tree, &measured, (38, 15));
    }

    #[test]
    fn the_answer_is_the_largest_window_that_fits() {
        // "Fits" alone would be satisfied by 1x1. The report is the LARGEST such window, because a
        // client that under-reports gives the session fewer cells than it can actually show — so
        // one more cell in either dimension must break the property.
        let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
        let measured = [(PaneId(1), (38, 17)), (PaneId(2), (38, 17))];
        let (cols, rows) = fit_window(&tree, &measured).expect("both panes are measured");
        every_pane_fits(&tree, &measured, (cols, rows));
        for bigger in [(cols + 1, rows), (cols, rows + 1)] {
            let tiling = tile(&tree, Rect::screen(bigger.0, bigger.1));
            assert!(
                tiling.panes.iter().any(|held| {
                    let (c, r) = measured
                        .iter()
                        .find(|(pane, _)| *pane == held.pane)
                        .expect("the tiling holds only panes of the tree")
                        .1;
                    held.area.cols > c || held.area.rows > r
                }),
                "{bigger:?} fits too, so {cols}x{rows} was not the largest"
            );
        }
    }

    #[test]
    fn every_ratio_and_every_shape_round_trips() {
        // The property, over the rounding that is this module's whole difficulty: whatever the
        // ratio and whatever the measurements, the reported window never hands a pane more cells
        // than its surface has. A fold that trusted its own arithmetic (rather than asking `tile`)
        // is off by one wherever `floor` lands, and this is the case that says so.
        for ratio in [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9] {
            for (first, second) in [
                ((38, 17), (38, 17)),
                ((12, 17), (64, 17)),
                ((64, 9), (12, 40)),
                ((1, 1), (80, 24)),
            ] {
                for dir in [SplitDir::Horizontal, SplitDir::Vertical] {
                    let tree = split(dir, ratio, leaf(1), leaf(2));
                    let measured = [(PaneId(1), first), (PaneId(2), second)];
                    let window = fit_window(&tree, &measured)
                        .unwrap_or_else(|| panic!("{dir:?} at {ratio} over {first:?} {second:?}"));
                    every_pane_fits(&tree, &measured, window);
                }
            }
        }
    }

    #[test]
    fn a_nested_tree_folds_through_both_axes() {
        // Three panes: one tall pane beside two stacked ones — the shape where a single subtraction
        // could not work, since the right column loses two headers and the left loses one.
        let tree = LayoutWire {
            root: Some(LayoutNodeWire::Split {
                id: None,
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(leaf(1)),
                second: Box::new(LayoutNodeWire::Split {
                    id: None,
                    dir: SplitDir::Vertical,
                    ratio: 0.5,
                    first: Box::new(leaf(2)),
                    second: Box::new(leaf(3)),
                }),
            }),
        };
        let measured = [
            (PaneId(1), (38, 17)),
            (PaneId(2), (38, 7)),
            (PaneId(3), (38, 7)),
        ];
        // Columns: 38 + divider + 38. Rows: the LEFT pane can show 17, but the right column can
        // only stack 7 + divider + 7, so the window is 15 — the shorter side of the tree decides.
        assert_eq!(fit_window(&tree, &measured), Some((77, 15)));
        every_pane_fits(&tree, &measured, (77, 15));
    }

    #[test]
    fn an_unmeasured_pane_has_no_answer_at_all() {
        // pinion publishes (0, 0) for a pane before the first layout. A report is a CLAIM about
        // what this client can show, and a client that has not been laid out has none to make —
        // reporting the panes it does know would name a window missing a pane's worth of cells.
        let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
        assert_eq!(fit_window(&tree, &[(PaneId(1), (38, 17))]), None);
        assert_eq!(fit_window(&tree, &[]), None);
        // An empty arrangement is the same answer for the same reason: nothing to give cells to.
        assert_eq!(fit_window(&LayoutWire { root: None }, &[]), None);
    }

    #[test]
    fn a_surface_too_small_to_show_every_pane_reports_nothing() {
        // A pane whose surface holds no cells at all (a panel collapsed to its chrome): there is no
        // window in which all three are present, and the honest answer is to stay out of the
        // arbitration rather than to report a window that would drop one.
        let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
        assert_eq!(
            fit_window(&tree, &[(PaneId(1), (0, 17)), (PaneId(2), (38, 17))]),
            None
        );
    }
}
