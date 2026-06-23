//! Pane arrangement (R38 dividers, R40 grid): fold the docked panes into a
//! draggable-divider layout that fills the window. Two modes, selected by
//! `SPRAG_GUI_LAYOUT` ([`layout_mode`]):
//!
//! * **`row`** (default) — the R38 1D layout: panes left-to-right, `N-1` nested
//!   [`view_splitter`]s, all [`SplitterOrientation::Horizontal`]. [`view_split_row`].
//! * **`grid`** (R40) — a balanced 2D layout: an outer Vertical splitter stack of
//!   rows, each row an inner Horizontal splitter stack of panes ([`view_grid`],
//!   driven by [`grid_plan`]).
//!
//! Orthogonal to `dock` (which window) and `terminal` (which panes) — this owns
//! the divider RATIOS and the splitter-handle tags + orientations.
//!
//! ## The seam (pinion-widget-paint)
//!
//! `view_splitter(left, right, &ratio, theme, style, dragging)` emits a flex
//! Container (Row for [`SplitterOrientation::Horizontal`], Column for `Vertical`)
//! with `left` (flex_grow = ratio), a tagged handle, `right` (flex_grow =
//! 1 - ratio). A [`SplitterExternal`](pinion_widget_paint::splitter::SplitterExternal)
//! registered at the handle tag (in [`create_extra_externals`](crate::TerminalViewer))
//! mutates the SAME ratio `Signal` on a pointer drag — the shell's pointer router
//! delivers press/move/release to it automatically (no `WidgetCore` pointer
//! method). The ratio is `Owner::cache`-shared between the read side ([`fold`]) and
//! the write side (the External), so a drag re-weights the painted panes.
//!
//! ## N panes -> N-1 dividers; one [`fold`] for both axes
//!
//! `view_splitter` is binary, so N parts nest N-1 splitters ([`fold`], right-
//! nested: divider `k` separates part `k` from everything to its right). A binary
//! split-tree of N leaves always has exactly N-1 internal dividers, so both row
//! mode (one row of N) and grid mode (rows + row-dividers) fit the
//! [`SPLITTER_COUNT`] = [`MAX_PANES`]`-1` handle table. [`fold`] handles a single
//! part (returns it un-wrapped, no divider), so a 1-pane row never panics.
//!
//! ## Divider identity is POSITION-keyed (a v1 bound — shared by both modes)
//!
//! Ratios + handle tags are keyed by docked POSITION / grid divider-id, not by
//! pane identity, so undock/dock (which re-compacts the docked set, R37) reshuffles
//! which panes a remembered divider separates — a visible layout jump. This is a
//! deliberate v1 bound, NOT the terminal model: the correct long-term shape is a
//! real split-tree with ratios keyed to the boundary between two pane IDENTITIES
//! (so a divider follows its panes across dock/undock and across row<->grid). It is
//! deferred on purpose — premature at [`MAX_PANES`] = 8 with no dynamic panes yet,
//! and its natural consumer is dynamic split/close (gated on pinion PR-9). The
//! asymmetry that panes get identity keys ([`pane_tag`](crate::terminal::pane_tag))
//! while dividers get position keys is the marker for that work.
//!
//! ## Grid mode holds slots on undock (the row<->grid asymmetry, by necessity)
//!
//! Row mode COMPACTS on undock (a floated pane leaves the row; the panes to its
//! right slide over) — safe because every row divider is Horizontal regardless of
//! count, so the boot-registered Externals still match the painted dividers.
//!
//! A grid CANNOT compact: a balanced grid's divider ORIENTATIONS depend on the
//! pane COUNT (`grid_plan(4)` = a 2x2 with a Vertical row-divider; `grid_plan(3)` =
//! a 2-then-1 with a Vertical row-divider at a DIFFERENT id). Externals are
//! registered ONCE at boot ([`create_extra_externals`](crate::TerminalViewer),
//! a pinion constraint —
//! runtime focusable/External refresh is gated on PR-9, no workaround), so a
//! divider's drag-axis is welded at boot by tag. If grid mode reshaped on undock,
//! a divider that was Vertical at boot could be painted Horizontal after a pane
//! floats, and its boot External would drive the wrong axis. So grid mode arranges
//! by the FIXED boot shape `grid_plan(pane_count)` and renders a floated pane's
//! slot as an UNTAGGED empty cell (held, see [`crate::view`]); the scaffold (tags +
//! orientations) never changes, so externals always match, and dock-back refills
//! the slot in place. This is the better-behaved option — grid happens to avoid the
//! row-mode position-keyed "jump" above — forced by the boot-only-External rule.

use crate::terminal::MAX_PANES;
use pinion_core::Scene;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::ContainerNode;
use pinion_core::theme::Theme;
use pinion_widget_paint::splitter::{SplitterOrientation, SplitterStyle, view_splitter};
use std::rc::Rc;

/// The number of dividers for N panes is N-1, so the handle table is one shorter
/// than the pane table.
const SPLITTER_COUNT: usize = MAX_PANES - 1;

/// The splitter-handle tags (`sprag_gui.split.<j>`), one per possible divider
/// (global divider id `j`). Static — the `&'static str` pinion External-tag
/// constraint (the handles are pointer-only, never marked focusable, so they stay
/// out of R1020's scene-derived Tab order). Reached only via [`splitter_tag`]; the
/// `SplitterExternal` registered at each
/// ([`create_extra_externals`](crate::TerminalViewer)) shares the ratio Signal
/// [`use_splitter_ratio`]`(j)` the fold reads. Both row mode (ids 0..N-1, all
/// Horizontal) and grid mode ([`grid_plan`]: column ids then row ids) draw their
/// dividers from this one table.
///
/// This is the per-DIVIDER identity table, a SEPARATE axis from the per-PANE
/// identity SSOT ([`PaneSlot`](crate::terminal) / `pane_tag` etc.): there are `N-1`
/// dividers for `N` panes and a divider is position-keyed (it separates whatever
/// panes sit on either side), so it deliberately does not fold into the per-pane
/// struct (see the "Divider identity is POSITION-keyed" section above).
const SPLITTER_TAGS: [&str; SPLITTER_COUNT] = [
    "sprag_gui.split.0",
    "sprag_gui.split.1",
    "sprag_gui.split.2",
    "sprag_gui.split.3",
    "sprag_gui.split.4",
    "sprag_gui.split.5",
    "sprag_gui.split.6",
];

/// The handle tag of divider `j` (`j < `[`SPLITTER_COUNT`]).
pub(crate) fn splitter_tag(j: usize) -> &'static str {
    SPLITTER_TAGS[j]
}

/// The default divider ratio — even (left/top share `0.5`), matching the former
/// even tiling so the boot layout is unchanged.
const SPLIT_RATIO_DEFAULT: f32 = 0.5;

/// `Owner::cache` key for divider `j`'s ratio.
fn ratio_key(j: usize) -> String {
    format!("sprag_gui.split.ratio.{j}")
}

/// Divider `j`'s ratio `Signal` (left/top share, `[0, 1]`), an `Owner::cache`-backed
/// `Rc<Signal<f32>>` SHARED between the read side ([`fold`] -> `view_splitter`) and
/// the write side (`SplitterExternal::attach_ratio`) — both resolve the same
/// root-owner slot, so a drag (`set`) re-weights the painted panes. Per-divider
/// (keyed by global divider id).
pub(crate) fn use_splitter_ratio(j: usize) -> Rc<Signal<f32>> {
    Owner::current()
        .expect("use_splitter_ratio() requires an active Owner scope")
        .cache(ratio_key(j), || Signal::new(SPLIT_RATIO_DEFAULT))
}

/// The pane-arrangement mode (`SPRAG_GUI_LAYOUT`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LayoutMode {
    /// The R38 1D row: panes left-to-right, all-Horizontal dividers.
    Row,
    /// The R40 balanced 2D grid: rows of panes, Vertical row-dividers.
    Grid,
}

/// The default layout when `SPRAG_GUI_LAYOUT` is unset — `Row`, so the boot layout
/// is byte-identical to R38/R39 (zero regression; grid is opt-in).
const LAYOUT_DEFAULT: LayoutMode = LayoutMode::Row;

/// Parse a `SPRAG_GUI_LAYOUT` spec into a [`LayoutMode`]. Case-insensitive;
/// absent / malformed falls back to `default`. Pure (no env) so it is testable.
fn parse_layout(spec: Option<&str>, default: LayoutMode) -> LayoutMode {
    match spec.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("grid") => LayoutMode::Grid,
        Some("row") => LayoutMode::Row,
        _ => default,
    }
}

/// The layout mode: `SPRAG_GUI_LAYOUT=grid` selects the 2D grid; anything else
/// (incl. unset) is the 1D row. Env-read (no `Owner` scope), so the view side
/// ([`crate::view`]) and the External-registration side
/// ([`create_extra_externals`](crate::TerminalViewer)) read the ONE source and
/// agree on the divider count + orientations — mirrors
/// [`pane_count`](crate::terminal::pane_count).
pub(crate) fn layout_mode() -> LayoutMode {
    parse_layout(
        std::env::var("SPRAG_GUI_LAYOUT").ok().as_deref(),
        LAYOUT_DEFAULT,
    )
}

/// One row of a [`GridPlan`]: the absolute cell (pane) indices it holds (left-to-
/// right) and the global divider ids for its internal column gaps
/// (`len == cells.len() - 1`).
struct RowPlan {
    cells: std::ops::Range<usize>,
    column_divider_ids: Vec<usize>,
}

/// The balanced 2D arrangement of `n` cells: rows of cell-index ranges, with every
/// divider's global id pre-assigned. The ONE place divider ids are allocated, so
/// the view fold ([`view_grid`]) and the External registration
/// ([`divider_orientations`]) iterate the SAME structure and a divider's tag,
/// ratio, and orientation can never disagree — orientation is a CONSEQUENCE of
/// which list an id lives in (a `column_divider_ids` id is Horizontal, a
/// [`Self::row_divider_ids`] id is Vertical), not a parallel array kept in sync.
struct GridPlan {
    rows: Vec<RowPlan>,
    /// The Vertical divider ids between consecutive rows (`len == rows.len() - 1`).
    row_divider_ids: Vec<usize>,
}

/// Build the balanced grid plan for `n` cells (`n >= 1`). The most-square grid:
/// `cols = ceil(sqrt(n))` (via an INTEGER search — `(n as f64).sqrt().ceil()` mis-
/// rounds at perfect squares), `rows = ceil(n / cols)`, then `n` cells spread as
/// evenly as possible across the rows (the first `n % rows` rows get one extra).
/// Divider ids: every column gap first (row by row, ids `0..n-rows`), then the
/// row gaps (`n-rows..n-1`), so the ids are gapless over `0..n-1` (asserted) and
/// total `n-1`. Pure.
fn grid_plan(n: usize) -> GridPlan {
    debug_assert!(n >= 1, "grid_plan needs >= 1 cell");
    let n = n.max(1);
    // cols = smallest c with c*c >= n. Integer search (NOT f64 sqrt — its rounding
    // can place a perfect square one column off, silently mis-shaping the grid).
    let cols = (1usize..)
        .find(|&c| c * c >= n)
        .expect("c*c grows without bound");
    let rows = n.div_ceil(cols);
    let base = n / rows;
    let extra = n % rows;

    let mut next_cell = 0usize;
    let mut next_div = 0usize; // column dividers take ids 0..(n-rows)
    let mut row_plans = Vec::with_capacity(rows);
    for r in 0..rows {
        // Spread the remainder over the first `extra` rows so the grid stays
        // balanced (e.g. 7 -> [3, 2, 2], not [3, 3, 1]).
        let count = base + usize::from(r < extra);
        let cells = next_cell..next_cell + count;
        next_cell += count;
        let column_divider_ids = (0..count.saturating_sub(1))
            .map(|_| {
                let id = next_div;
                next_div += 1;
                id
            })
            .collect();
        row_plans.push(RowPlan {
            cells,
            column_divider_ids,
        });
    }
    // The row gaps take the remaining ids; together with the column ids they cover
    // 0..n-1 with no gap or overlap.
    let row_divider_ids: Vec<usize> = (next_div..n.saturating_sub(1)).collect();
    debug_assert_eq!(next_cell, n, "every cell placed");
    debug_assert_eq!(
        row_divider_ids.len(),
        rows.saturating_sub(1),
        "row dividers = rows - 1"
    );
    GridPlan {
        rows: row_plans,
        row_divider_ids,
    }
}

impl GridPlan {
    /// The orientation of each divider id `0..n-1`, derived from list membership
    /// (a `column_divider_ids` id -> Horizontal, a [`Self::row_divider_ids`] id ->
    /// Vertical). The registration side ([`divider_orientations`]) reads this; the
    /// view side ([`view_grid`]) reads the same plan's per-list ids — same struct,
    /// so orientation and placement cannot drift.
    fn orientations(&self) -> Vec<SplitterOrientation> {
        let count = self
            .rows
            .iter()
            .map(|r| r.column_divider_ids.len())
            .sum::<usize>()
            + self.row_divider_ids.len();
        let mut out: Vec<Option<SplitterOrientation>> = vec![None; count];
        for row in &self.rows {
            for &j in &row.column_divider_ids {
                out[j] = Some(SplitterOrientation::Horizontal);
            }
        }
        for &j in &self.row_divider_ids {
            out[j] = Some(SplitterOrientation::Vertical);
        }
        out.into_iter()
            .map(|o| o.expect("every divider id 0..n-1 assigned an orientation"))
            .collect()
    }
}

/// The orientation of each divider (`splitter_tag(j)`, `j` in index order) the boot
/// Externals must match the painted dividers with, for the current
/// [`layout_mode`] + `pane_count`. Row mode -> all Horizontal (the 1D row). Grid
/// mode -> the [`grid_plan`] mix. The registration side
/// ([`create_extra_externals`](crate::TerminalViewer)) and the view side
/// ([`view_split_row`] / [`view_grid`]) derive their dividers from the SAME source
/// (this mode + plan), so they agree on count and orientation. The returned length
/// is `pane_count - 1` in both modes.
pub(crate) fn divider_orientations(pane_count: usize) -> Vec<SplitterOrientation> {
    match layout_mode() {
        LayoutMode::Row => vec![SplitterOrientation::Horizontal; pane_count.saturating_sub(1)],
        LayoutMode::Grid => grid_plan(pane_count.max(1)).orientations(),
    }
}

/// Right-nested fold of `parts` into `view_splitter`s along one axis: the rightmost
/// part seeds the accumulator; each step to its left wraps
/// `view_splitter(left, acc, ratio[id], orientation, ..)`, so `divider_ids[k]`
/// governs part `k` vs everything to its right. `divider_ids.len()` must be
/// `parts.len() - 1`. A single part (no divider) returns un-wrapped — so a 1-pane
/// row never panics; `parts` must be non-empty.
fn fold(
    parts: Vec<Scene>,
    divider_ids: &[usize],
    orientation: SplitterOrientation,
    theme: &Theme,
) -> Scene {
    debug_assert_eq!(
        divider_ids.len(),
        parts.len().saturating_sub(1),
        "one divider per gap"
    );
    let mut iter = parts.into_iter().enumerate().rev();
    let (_, mut acc) = iter.next().expect("fold needs >= 1 part");
    for (k, left) in iter {
        let j = divider_ids[k];
        acc = view_splitter(
            left,
            acc,
            &use_splitter_ratio(j),
            theme,
            &SplitterStyle::m3_default(orientation, splitter_tag(j)),
            false, // dragging: no visual handle-highlight (cosmetic; drag still works)
        );
    }
    acc
}

/// Arrange the `panes` left-to-right (row mode) with draggable dividers. `0` panes
/// -> an empty root; otherwise a single Horizontal [`fold`] over ids
/// `0..panes.len()-1`. The result carries no explicit size — `compose`
/// applies the definite [`fill_definite`] extent the flex layout needs (the single
/// enforcement point), so this returns the bare arrangement.
pub(crate) fn view_split_row(panes: Vec<Scene>, theme: &Theme) -> Scene {
    if panes.is_empty() {
        Scene::Container(ContainerNode::new(Vec::new()))
    } else {
        let ids: Vec<usize> = (0..panes.len().saturating_sub(1)).collect();
        fold(panes, &ids, SplitterOrientation::Horizontal, theme)
    }
}

/// Arrange `cells` in a balanced 2D grid (grid mode) with draggable dividers.
/// `cells[i]` is cell `i` of the FIXED boot shape `grid_plan(cells.len())` (a docked
/// pane's projection, or an UNTAGGED held slot for a floated pane — [`crate::view`]).
/// Each row is an inner Horizontal [`fold`] over its `column_divider_ids`; the rows
/// are an outer Vertical [`fold`] over `row_divider_ids`, so the scene is a Column of
/// Rows. Empty -> an empty root. Carries no explicit size — `compose`
/// applies the [`fill_definite`] extent (the single enforcement point).
pub(crate) fn view_grid(cells: Vec<Scene>, theme: &Theme) -> Scene {
    if cells.is_empty() {
        Scene::Container(ContainerNode::new(Vec::new()))
    } else {
        let plan = grid_plan(cells.len());
        // Take each cell by absolute index into its row (each is taken exactly
        // once — the plan's row ranges partition 0..cells.len()).
        let mut slots: Vec<Option<Scene>> = cells.into_iter().map(Some).collect();
        let rows: Vec<Scene> = plan
            .rows
            .iter()
            .map(|row| {
                let row_cells: Vec<Scene> = row
                    .cells
                    .clone()
                    .map(|i| slots[i].take().expect("each cell taken once"))
                    .collect();
                fold(
                    row_cells,
                    &row.column_divider_ids,
                    SplitterOrientation::Horizontal,
                    theme,
                )
            })
            .collect();
        fold(
            rows,
            &plan.row_divider_ids,
            SplitterOrientation::Vertical,
            theme,
        )
    }
}

/// Give a Container a definite `Percent(100)` size (via the one
/// [`crate::view::fill_size`] authority) so it fills compose's surface root. The
/// `view_splitter` / lone-pane root sets no size of its own; this supplies the
/// definite extent its flex layout needs (avoiding the intrinsic-collapse the
/// splitter's own R685 fix documents). Applied to the OUTERMOST node only — every
/// nested splitter gets a definite extent from its parent's flex distribution, so
/// interior nodes keep `Auto` cross-axes for `AlignItems::Stretch` to fill.
///
/// The contract this enforces: **every content node handed to `compose` must carry
/// a definite extent**, or the sizeless flex child collapses to its content's
/// intrinsic size on the main axis (the cross axis still stretches).
/// `compose` is the SOLE caller — it applies this to whatever
/// content it wraps (docked tiling OR a lone undock pane), so the invariant lives
/// in one place and no caller can forget it. (Forgetting it was the R55 undock bug:
/// the pane reflowed only its width, its height pinned to the grid's content rows.)
pub(crate) fn fill_definite(scene: Scene) -> Scene {
    match scene {
        Scene::Container(c) => {
            Scene::Container(c.map_layout(|l| l.with_size(crate::view::fill_size())))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;
    use pinion_core::style::FlexDirection;
    use pinion_core::theme::use_theme;

    fn stub_pane(tag: &'static str) -> Scene {
        Scene::Container(ContainerNode::new(Vec::new()).with_tag(tag))
    }

    fn stub_panes(n: usize) -> Vec<Scene> {
        const TAGS: [&str; 8] = ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7"];
        (0..n).map(|i| stub_pane(TAGS[i])).collect()
    }

    // ----- row mode (R38, unchanged behavior) -----

    #[test]
    fn two_panes_get_one_draggable_divider() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_split_row(vec![stub_pane("p0"), stub_pane("p1")], &theme)
        });
        assert!(
            scene.contains_tag("p0") && scene.contains_tag("p1"),
            "both panes present"
        );
        assert!(
            scene.contains_tag(splitter_tag(0)),
            "divider 0 handle present"
        );
        assert!(
            !scene.contains_tag(splitter_tag(1)),
            "only N-1 = 1 divider for 2 panes"
        );
    }

    #[test]
    fn three_panes_nest_two_dividers() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_split_row(
                vec![stub_pane("p0"), stub_pane("p1"), stub_pane("p2")],
                &theme,
            )
        });
        assert!(
            scene.contains_tag(splitter_tag(0)) && scene.contains_tag(splitter_tag(1)),
            "two dividers"
        );
        assert!(
            scene.contains_tag("p0") && scene.contains_tag("p1") && scene.contains_tag("p2"),
            "all three panes present",
        );
    }

    #[test]
    fn one_pane_has_no_divider() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_split_row(vec![stub_pane("solo")], &theme)
        });
        assert!(scene.contains_tag("solo"));
        assert!(
            !scene.contains_tag(splitter_tag(0)),
            "a lone pane has no divider"
        );
    }

    #[test]
    fn splitter_tag_encodes_its_index() {
        // Pin the hand-typed table against index drift (the digits are not
        // compiler-checked the way the array length is).
        for j in 0..SPLITTER_COUNT {
            assert!(
                splitter_tag(j).ends_with(&format!(".{j}")),
                "tag {j} = {}",
                splitter_tag(j)
            );
        }
    }

    #[test]
    fn ratio_signal_is_per_divider_and_defaults_even() {
        let owner = Owner::new();
        owner.run(|| {
            assert!((use_splitter_ratio(0).get() - SPLIT_RATIO_DEFAULT).abs() < f32::EPSILON);
            use_splitter_ratio(0).set(0.7);
            assert!(
                (use_splitter_ratio(0).get() - 0.7).abs() < f32::EPSILON,
                "drag re-weights"
            );
            assert!(
                (use_splitter_ratio(1).get() - SPLIT_RATIO_DEFAULT).abs() < f32::EPSILON,
                "divider 1 is independent",
            );
        });
    }

    // ----- layout mode parsing -----

    #[test]
    fn parse_layout_selects_grid_else_row() {
        assert_eq!(parse_layout(None, LAYOUT_DEFAULT), LayoutMode::Row);
        assert_eq!(parse_layout(Some("grid"), LAYOUT_DEFAULT), LayoutMode::Grid);
        assert_eq!(
            parse_layout(Some(" GRID "), LAYOUT_DEFAULT),
            LayoutMode::Grid
        ); // trim + case
        assert_eq!(parse_layout(Some("row"), LAYOUT_DEFAULT), LayoutMode::Row);
        assert_eq!(parse_layout(Some("tiled"), LAYOUT_DEFAULT), LayoutMode::Row); // malformed
        assert_eq!(parse_layout(Some(""), LAYOUT_DEFAULT), LayoutMode::Row);
    }

    // ----- grid_plan: the divider-id SSOT -----

    /// Every count `1..=MAX_PANES` allocates a gapless, total set of divider ids
    /// `0..n-1` (so `splitter_tag(j)` / `use_splitter_ratio(j)` never collide or
    /// gap), and the cells partition `0..n`.
    #[test]
    fn grid_plan_allocates_gapless_dividers_and_partitions_cells() {
        for n in 1..=MAX_PANES {
            let plan = grid_plan(n);
            // Cells partition 0..n.
            let mut cells: Vec<usize> = plan.rows.iter().flat_map(|r| r.cells.clone()).collect();
            cells.sort_unstable();
            assert_eq!(
                cells,
                (0..n).collect::<Vec<_>>(),
                "n={n}: cells partition 0..n"
            );
            // Divider ids (column + row) are exactly 0..n-1.
            let mut ids: Vec<usize> = plan
                .rows
                .iter()
                .flat_map(|r| r.column_divider_ids.clone())
                .chain(plan.row_divider_ids.clone())
                .collect();
            ids.sort_unstable();
            assert_eq!(
                ids,
                (0..n.saturating_sub(1)).collect::<Vec<_>>(),
                "n={n}: divider ids gapless 0..n-1"
            );
            // Orientations cover every id with no panic.
            assert_eq!(
                plan.orientations().len(),
                n - 1,
                "n={n}: one orientation per divider"
            );
        }
    }

    /// The balanced shape: most-square, remainder on the first rows.
    #[test]
    fn grid_plan_shapes_are_balanced() {
        let shape = |n: usize| {
            grid_plan(n)
                .rows
                .iter()
                .map(|r| r.cells.len())
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(1), vec![1]);
        assert_eq!(shape(2), vec![2]); // == row mode at the default pane count
        assert_eq!(shape(3), vec![2, 1]);
        assert_eq!(shape(4), vec![2, 2]);
        assert_eq!(shape(5), vec![3, 2]);
        assert_eq!(shape(6), vec![3, 3]);
        assert_eq!(shape(7), vec![3, 2, 2]);
        assert_eq!(shape(8), vec![3, 3, 2]);
    }

    /// A 2x2 grid's three dividers: two Horizontal (column) then one Vertical (row).
    #[test]
    fn grid_plan_orientations_mix_column_then_row() {
        assert_eq!(
            grid_plan(4).orientations(),
            vec![
                SplitterOrientation::Horizontal,
                SplitterOrientation::Horizontal,
                SplitterOrientation::Vertical,
            ]
        );
    }

    // ----- view_grid: the 2D fold -----

    /// `view_grid` of 4 cells builds a Column of Rows: the outer splitter is
    /// Vertical (the row divider), and it nests Horizontal row splitters. All four
    /// pane tags + three divider tags are present; the fourth divider tag is not.
    #[test]
    fn view_grid_builds_a_column_of_rows() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_grid(stub_panes(4), &theme)
        });
        for i in 0..4 {
            assert!(
                scene.contains_tag(["p0", "p1", "p2", "p3"][i]),
                "pane {i} present"
            );
        }
        assert!(
            scene.contains_tag(splitter_tag(0))
                && scene.contains_tag(splitter_tag(1))
                && scene.contains_tag(splitter_tag(2)),
            "three dividers present"
        );
        assert!(
            !scene.contains_tag(splitter_tag(3)),
            "only N-1 = 3 dividers for 4 panes"
        );

        // The outermost arrangement node (inside the Percent(100) fill wrapper is
        // the same node — fill_definite maps the outer splitter's own layout) is a
        // Vertical splitter => flex_direction Column.
        let dir = match &scene {
            Scene::Container(c) => c.layout.flex_direction,
            other => unreachable!("view_grid returns a Container, got {other:?}"),
        };
        assert_eq!(
            dir,
            FlexDirection::Column,
            "the grid stacks rows vertically"
        );
    }

    /// At the default pane count (2) grid mode == row mode: one Horizontal divider,
    /// same panes, same single splitter. So defaulting users see no change.
    #[test]
    fn grid_equals_row_at_two_panes() {
        let owner = Owner::new();
        owner.run(|| {
            let theme = use_theme("app").theme_animated();
            let grid = view_grid(vec![stub_pane("p0"), stub_pane("p1")], &theme);
            assert!(grid.contains_tag("p0") && grid.contains_tag("p1"));
            assert!(grid.contains_tag(splitter_tag(0)), "one divider");
            assert!(!grid.contains_tag(splitter_tag(1)));
            let dir = match &grid {
                Scene::Container(c) => c.layout.flex_direction,
                other => unreachable!("got {other:?}"),
            };
            assert_eq!(
                dir,
                FlexDirection::Row,
                "2 panes => a single horizontal row"
            );
        });
    }

    /// Row mode is the default, so `divider_orientations` (which reads the env) is
    /// all-Horizontal with no `SPRAG_GUI_LAYOUT` set — the grid branch is covered
    /// purely by `grid_plan(..).orientations()` above (no env mutation).
    #[test]
    fn divider_orientations_default_is_all_horizontal() {
        // No SPRAG_GUI_LAYOUT in the test env => row mode.
        assert_eq!(
            divider_orientations(4),
            vec![SplitterOrientation::Horizontal; 3]
        );
        assert_eq!(divider_orientations(1), Vec::new(), "one pane, no divider");
    }
}
