//! `GridBuffer` -> a real terminal, as [`Change`]s.
//!
//! This is the terminal frontend's UNIT half: the pure function that turns the cells a
//! [`HostClient`](sprag_host::HostClient) serves into the escape-sequence vocabulary a terminal
//! understands. It is the character-cell peer of `sprag-gui`'s pixel work, and it lives here for
//! the reason `sprag_client`'s crate docs give: the shared client hands out `GridBuffer`s and
//! knows nothing about the unit either frontend measures them in.
//!
//! # Why `termwiz::surface::Surface` and not hand-written ANSI
//!
//! A bespoke writer would have to be right about terminal capabilities across a dozen emulators,
//! which is exactly the class of thing a mature renderer already encodes and a fresh one gets
//! wrong on someone else's box. Emitting [`Change`]s into a [`Surface`](termwiz::surface::Surface)
//! means the diffing and the terminfo-correct output are termwiz's problem, and the work left here
//! is a pure mapping — testable with no terminal at all, which is how every test in this module
//! runs.
//!
//! # What the mapping cost, measured
//!
//! The H1 design carried a HYPOTHESIS: that pinion's cell vocabulary maps onto termwiz's losslessly.
//! Measured, it is one-to-one on every axis but two, and both exceptions are stated where they are
//! made:
//!
//! * **Intensity.** pinion carries `bold` and `dim` as independent flags; termwiz carries one
//!   [`Intensity`] axis. See [`cell_attributes`] — the combination is unreachable through sprag's
//!   own emulator, which is what makes the collapse lossless in practice rather than in principle.
//! * **Column width.** pinion's [`CellWidth`] and termwiz's own measurement of a cluster are
//!   computed by DIFFERENT unicode tables (`unicode-width` in `sprag-vt`, termwiz's `widechar_width`
//!   here), so they can disagree. [`grid_changes`] does not assume they agree — it checks, and
//!   re-anchors the cursor when they do not.
//!
//! Everything else — the six underline styles, the three colour forms, the six cursor shapes,
//! OSC 8 hyperlinks — has an exact counterpart, which is unsurprising: `sprag-vt` builds pinion's
//! cells FROM termwiz's in the first place, so this module is closing a circle rather than
//! crossing a border.

use std::sync::Arc;

use pinion_core::style::Color as PinColor;
use pinion_core::{
    CellWidth, CursorShape as PinCursorShape, GridBuffer, Hyperlink as PinHyperlink, HyperlinkId,
    TermCell, TermColor, UnderlineStyle as PinUnderlineStyle,
};
use termwiz::cell::{Blink, CellAttributes, Intensity, Underline, unicode_column_width};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::hyperlink::Hyperlink;
use termwiz::surface::{Change, CursorShape, CursorVisibility, Position};

/// The whole of `grid` as terminal changes, ready for
/// [`Surface::add_changes`](termwiz::surface::Surface::add_changes).
///
/// Every cell of the buffer is written, so no clear is needed and none is emitted: a
/// [`Change::ClearScreen`] would fight the surface's own diffing, repainting rows that did not
/// change. **The changes cover the buffer's OWN extent and nothing outside it** — a surface larger
/// than the grid keeps whatever it already held there, because only the caller knows what belongs
/// in the remainder (another pane, once the layouter of slice 4 exists; a blank margin until then).
///
/// The cursor is emitted last, so the terminal's real cursor comes to rest where the PANE's cursor
/// is — which is what makes the view read as a terminal rather than a picture of one. A cursor
/// position outside the buffer is reported HIDDEN rather than clamped: pinion's `GridCursor` docs
/// say the producer's position may briefly fall outside during an in-flight resize, and a clamped
/// cursor would draw an authoritative-looking block in a cell the producer never named.
///
/// # Runs, and the column check inside them
///
/// Cells are batched into runs of equal attributes — one [`Change::AllAttributes`] and one
/// [`Change::Text`] per run — because a per-cell change list would be `cols * rows` entries for a
/// screen that is mostly one style.
///
/// A run also ENDS when termwiz's measurement of the text so far stops matching the columns those
/// cells were supposed to occupy, and that check is the load-bearing part. `sprag-vt` computes
/// [`CellWidth`] with the `unicode-width` crate while termwiz measures with its own
/// `widechar_width` tables; the two are independent implementations of an evolving standard and
/// they can disagree (emoji sequences and ambiguous-width characters are where they historically
/// do). If they ever disagree here, every remaining cell of the row would shift — a whole line of
/// garbage from one character. Closing the run and re-anchoring at an absolute column confines the
/// disagreement to the single cell that caused it: the corrective write comes AFTER the wide
/// cluster that clobbered its neighbour, so the surface ends up right either way.
#[must_use]
pub fn grid_changes(grid: &GridBuffer) -> Vec<Change> {
    let (cols, rows) = (grid.cols(), grid.rows());
    if cols == 0 || rows == 0 {
        return Vec::new();
    }

    let mut changes = Vec::new();
    // The interned hyperlink last resolved, kept so a run of linked cells builds ONE `Arc` rather
    // than one per cell — the table is interned on the producer's side for the same reason.
    let mut interned: Option<(HyperlinkId, Arc<Hyperlink>)> = None;

    for row in 0..rows {
        // Every row is anchored absolutely rather than by trusting where the previous row's text
        // left the cursor: a row whose cells fill the last column would otherwise depend on the
        // terminal's autowrap setting, which is not this crate's to assume.
        let mut run = Run::new(row, 0);
        // Whether the previous cell was a wide HEAD, which is what makes the next cell's
        // `Trailer` its second column rather than a column of its own. Per row, because a wide
        // cluster never straddles a row boundary.
        let mut after_wide = false;
        for col in 0..cols {
            let Some(cell) = grid.cell(col, row) else {
                continue;
            };
            let follows_wide = std::mem::replace(&mut after_wide, cell.width == CellWidth::Wide);
            let Some((text, columns)) = printed(cell, follows_wide) else {
                // A trailer behind its own head: the head's cluster already occupies this column,
                // and its attributes are the head's by construction (pinion's `TermCell::trailer`
                // copies them), so writing anything here would draw a glyph nobody asked for.
                continue;
            };
            let link = cell
                .hyperlink
                .and_then(|id| grid.hyperlink(id).map(|link| (id, link)));
            let attrs = cell_attributes(cell, resolve_link(&mut interned, link));
            if run.attrs.as_ref() != Some(&attrs) {
                run.flush(&mut changes);
                run.restart(col, attrs);
            }
            run.text.push_str(text);
            run.span += columns;
            // The width cross-check (see the doc comment): if termwiz will not advance by the
            // columns these cells claim, the next cell must be re-anchored. The next cell is at
            // `col + columns` — one past a narrow cell, two past a wide one, whose trailer is
            // skipped.
            if unicode_column_width(&run.text, None) != run.span {
                let attrs = run.attrs.clone();
                run.flush(&mut changes);
                run.restart(
                    col.saturating_add(u16::try_from(columns).unwrap_or(1)),
                    attrs.unwrap_or_default(),
                );
            }
        }
        run.flush(&mut changes);
    }

    changes.extend(cursor_changes(grid));
    changes
}

/// What a cell prints and how many columns that print is supposed to occupy, or `None` when the
/// cell prints nothing because a preceding wide cluster already covers it.
///
/// The column count is what the width cross-check in [`grid_changes`] compares termwiz's own
/// measurement against, so it must be the count pinion INTENDED, never a measurement of the text:
/// a wide head claims BOTH of its columns here, which is why its trailer claims none.
///
/// # The orphan trailer
///
/// A `Trailer` that follows no head is a malformed buffer — nothing sprag's projection produces,
/// but constructible (`GridBuffer`'s row builder and its wire `TryFrom` both validate cell COUNTS,
/// not width coherence). It is rendered as the blank column it occupies rather than skipped,
/// because skipping it would shift every remaining cell of the row one column left: the failure
/// this whole accounting exists to prevent, arriving through the one door left open.
///
/// A cell with an empty cluster is blanked for the same reason — it occupies its column either
/// way, and printing nothing there moves the rest of the row.
fn printed(cell: &TermCell, follows_wide: bool) -> Option<(&str, usize)> {
    let cluster = if cell.cluster.is_empty() {
        " "
    } else {
        &cell.cluster
    };
    match cell.width {
        CellWidth::Trailer if follows_wide => None,
        CellWidth::Trailer => Some((" ", 1)),
        CellWidth::Wide => Some((cluster, 2)),
        CellWidth::Narrow => Some((cluster, 1)),
    }
}

/// One batch of same-attribute cells: where it starts, what it will print, and how many columns
/// that print is supposed to occupy.
///
/// The `span` is what makes the width cross-check possible at all — without it there is nothing to
/// compare termwiz's measurement against, and a disagreement would be discovered by the user
/// looking at a broken screen.
struct Run {
    /// The row every cell of this run is on (runs never span rows).
    row: u16,
    /// The column the run's text starts at, emitted as an absolute cursor position.
    col: u16,
    /// The attributes every cell in the run shares. `None` before the first cell, which is what
    /// makes an empty run flush to nothing.
    attrs: Option<CellAttributes>,
    /// The clusters, concatenated.
    text: String,
    /// How many columns `text` is SUPPOSED to occupy — the sum of the cells' declared widths, not
    /// a measurement of the string.
    span: usize,
}

impl Run {
    /// An empty run at `(col, row)`, carrying no attributes yet.
    fn new(row: u16, col: u16) -> Self {
        Self {
            row,
            col,
            attrs: None,
            text: String::new(),
            span: 0,
        }
    }

    /// Emit this run, if it has anything to say. An absolute cursor position precedes every run
    /// because a run may begin anywhere: after an attribute change, after a width disagreement, or
    /// at the start of a row.
    fn flush(&self, changes: &mut Vec<Change>) {
        let Some(attrs) = self.attrs.as_ref() else {
            return;
        };
        if self.text.is_empty() {
            return;
        }
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(self.col)),
            y: Position::Absolute(usize::from(self.row)),
        });
        changes.push(Change::AllAttributes(attrs.clone()));
        changes.push(Change::Text(self.text.clone()));
    }

    /// Begin a fresh run at `col` with `attrs`, keeping the row.
    fn restart(&mut self, col: u16, attrs: CellAttributes) {
        self.col = col;
        self.attrs = Some(attrs);
        self.text.clear();
        self.span = 0;
    }
}

/// Resolve an interned link to a shared termwiz one, reusing the previous `Arc` when the id has
/// not moved.
///
/// A run of linked cells all name the same [`HyperlinkId`] — that is what interning is for — so
/// building a fresh `Arc<Hyperlink>` per cell would allocate once per character of a hyperlinked
/// path for no gain: the attributes compare equal either way, so the run would not even split.
fn resolve_link<'a>(
    interned: &'a mut Option<(HyperlinkId, Arc<Hyperlink>)>,
    link: Option<(HyperlinkId, &PinHyperlink)>,
) -> Option<&'a Arc<Hyperlink>> {
    let (id, link) = link?;
    if interned.as_ref().is_none_or(|(seen, _)| *seen != id) {
        let built = match link.id.as_deref() {
            // The OSC-8 `id=` param is the GROUPING key: two runs sharing a non-empty id are one
            // logical link. It is carried across rather than dropped, so a terminal that groups
            // (a soft-wrapped path highlighting as one link) still can.
            Some(group) => Hyperlink::new_with_id(link.uri.clone(), group.to_owned()),
            None => Hyperlink::new(link.uri.clone()),
        };
        *interned = Some((id, Arc::new(built)));
    }
    interned.as_ref().map(|(_, link)| link)
}

/// One pinion cell's style as termwiz attributes.
///
/// # The one lossy axis, and why it is not lossy here
///
/// pinion models SGR 1 (bold) and SGR 2 (dim) as INDEPENDENT booleans; termwiz models them as one
/// [`Intensity`] with three values, so `bold && dim` has no representation. That combination is
/// **unreachable in the buffers this crate paints**: sprag's own emulator makes the two mutually
/// exclusive at the SGR — `Sgr::Intensity(Bold)` clears `dim` and `Sgr::Intensity(Half)` clears
/// `bold` (`sprag_vt`'s emulator) — which is also what termwiz's own parser does, because it is the
/// same one-axis model. So the collapse loses nothing that can arrive.
///
/// It is still given a deterministic rule, because the function must be total over a type that
/// admits the pair: **bold wins**. The rule exists so a hand-built buffer cannot produce
/// unpredictable output, not because the case occurs.
///
/// Every other axis is exact: the six underline styles are the same six, blink folds to
/// [`Blink::Slow`] (pinion folds SGR 5 and 6 into one flag on the way in, so there is no rapid
/// blink to lose), `hidden` is termwiz's `invisible`, and the three colour forms map through this
/// module's `term_color` — resolved by the projection before they arrive, so what a host sends is
/// always the truecolor arm.
#[must_use]
pub fn cell_attributes(cell: &TermCell, link: Option<&Arc<Hyperlink>>) -> CellAttributes {
    let mut attrs = CellAttributes::default();
    attrs
        .set_foreground(term_color(cell.fg))
        .set_background(term_color(cell.bg))
        .set_intensity(if cell.attrs.bold {
            Intensity::Bold
        } else if cell.attrs.dim {
            Intensity::Half
        } else {
            Intensity::Normal
        })
        .set_underline(underline(cell.attrs.underline))
        .set_italic(cell.attrs.italic)
        .set_blink(if cell.attrs.blink {
            Blink::Slow
        } else {
            Blink::None
        })
        .set_reverse(cell.attrs.reverse)
        .set_invisible(cell.attrs.hidden)
        .set_strikethrough(cell.attrs.strikethrough)
        .set_underline_color(
            cell.underline_color
                .map_or(ColorAttribute::Default, term_color),
        )
        .set_hyperlink(link.cloned());
    attrs
}

/// A pinion terminal colour as a termwiz one — the three closed forms, each with an exact peer.
///
/// **Note what actually arrives.** `sprag-grid`'s projection resolves every colour against the
/// pane's live palette, so a `GridBuffer` from a host carries [`TermColor::Rgb`] and nothing else:
/// an OSC 4 palette redefinition restains cells at the PRODUCER, which is what lets a theme change
/// recolour a screen that was printed before it. The other two arms are therefore not dead code
/// but not the live path either — they are what a hand-built buffer (`GridBuffer::new` blanks
/// carry [`TermColor::Default`]) needs to render correctly.
///
/// One consequence worth stating because it is a real behavioural fact rather than a detail: a
/// terminal client paints the colours the HOST resolved, so a remote session does not re-theme
/// itself against the local terminal's palette. That is the same thing `sprag-gui` does, and it is
/// the projection's design (the producer owns authoritative state), not an omission here.
fn term_color(color: TermColor) -> ColorAttribute {
    match color {
        TermColor::Default => ColorAttribute::Default,
        TermColor::Indexed(index) => ColorAttribute::PaletteIndex(index),
        TermColor::Rgb(PinColor { r, g, b, a }) => {
            ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((r, g, b, a)))
        }
    }
}

/// A pinion underline style as termwiz's — the same six, in the same order, because both are the
/// SGR 4:x vocabulary.
fn underline(style: PinUnderlineStyle) -> Underline {
    match style {
        PinUnderlineStyle::None => Underline::None,
        PinUnderlineStyle::Single => Underline::Single,
        PinUnderlineStyle::Double => Underline::Double,
        PinUnderlineStyle::Curly => Underline::Curly,
        PinUnderlineStyle::Dotted => Underline::Dotted,
        PinUnderlineStyle::Dashed => Underline::Dashed,
    }
}

/// The buffer's cursor as changes: colour, shape, visibility, then position.
///
/// pinion splits the cursor's SHAPE from its BLINK mode (`shape` + `blink`); termwiz folds the two
/// into one seven-variant enum. Three shapes times two modes is exactly six of those variants, so
/// the fold is a bijection — the seventh, `Default`, is the "whatever the terminal prefers" value
/// no producer-reported cursor means.
fn cursor_changes(grid: &GridBuffer) -> Vec<Change> {
    let cursor = grid.cursor();
    // Outside the buffer the cursor is not a position this client can honour — see the note in
    // [`grid_changes`]. Reporting it hidden is the truthful rendering of "the producer has not
    // told us where it is yet".
    let visible = cursor.visible && cursor.col < grid.cols() && cursor.row < grid.rows();
    let mut changes = vec![
        Change::CursorColor(
            cursor
                .cursor_color
                .map_or(ColorAttribute::Default, |color| {
                    term_color(TermColor::Rgb(color))
                }),
        ),
        Change::CursorShape(match (cursor.shape, cursor.blink) {
            (PinCursorShape::Block, true) => CursorShape::BlinkingBlock,
            (PinCursorShape::Block, false) => CursorShape::SteadyBlock,
            (PinCursorShape::Bar, true) => CursorShape::BlinkingBar,
            (PinCursorShape::Bar, false) => CursorShape::SteadyBar,
            (PinCursorShape::Underline, true) => CursorShape::BlinkingUnderline,
            (PinCursorShape::Underline, false) => CursorShape::SteadyUnderline,
        }),
        Change::CursorVisibility(if visible {
            CursorVisibility::Visible
        } else {
            CursorVisibility::Hidden
        }),
    ];
    if visible {
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(cursor.col)),
            y: Position::Absolute(usize::from(cursor.row)),
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::{CellAttrs, GridCursor};
    use termwiz::surface::Surface;

    /// Paint a buffer onto a surface of its own size — the composition every test here asserts
    /// through, so none of them assert on the change LIST when what matters is the screen.
    fn painted(grid: &GridBuffer) -> Surface {
        let mut surface = Surface::new(usize::from(grid.cols()), usize::from(grid.rows()));
        surface.add_changes(grid_changes(grid));
        surface
    }

    /// A cell with a cluster and default everything else.
    fn cell(cluster: impl Into<std::borrow::Cow<'static, str>>) -> TermCell {
        TermCell::new(cluster, TermColor::Default, TermColor::Default)
    }

    /// A one-row buffer of `cells`, padded with blanks to `cols`.
    fn row(cols: u16, cells: Vec<TermCell>) -> GridBuffer {
        GridBuffer::new(cols, 1).with_row(0, cells)
    }

    /// The base case, and the one every other test rests on: clusters land in their own columns.
    #[test]
    fn text_lands_in_its_own_columns() {
        let grid = row(6, "hi".chars().map(|c| cell(c.to_string())).collect());
        assert_eq!(painted(&grid).screen_chars_to_string().trim_end(), "hi");
    }

    /// The colour vocabulary round-trips through all three forms. `Rgb` is what a host actually
    /// sends (the projection resolves the palette), so it is the one that must be exact; the other
    /// two are what a hand-built buffer carries.
    #[test]
    fn the_three_colour_forms_each_have_an_exact_peer() {
        assert_eq!(term_color(TermColor::Default), ColorAttribute::Default);
        assert_eq!(
            term_color(TermColor::Indexed(9)),
            ColorAttribute::PaletteIndex(9)
        );
        assert_eq!(
            term_color(TermColor::Rgb(PinColor::rgb(0xcd, 0x00, 0x00))),
            ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((
                0xcd_u8, 0x00_u8, 0x00_u8, 0xff_u8
            ))),
        );
    }

    /// Bold and dim collapse onto ONE termwiz axis, deterministically. The pair is unreachable
    /// through sprag's emulator (which clears one when it sets the other), but the function is
    /// total over a type that admits it, so the rule is pinned rather than left to argument order.
    #[test]
    fn intensity_collapses_with_bold_winning() {
        let bold = cell("x").with_attrs(CellAttrs::empty().with_bold(true));
        let dim = cell("x").with_attrs(CellAttrs::empty().with_dim(true));
        let both = cell("x").with_attrs(CellAttrs::empty().with_bold(true).with_dim(true));
        assert_eq!(cell_attributes(&bold, None).intensity(), Intensity::Bold);
        assert_eq!(cell_attributes(&dim, None).intensity(), Intensity::Half);
        assert_eq!(cell_attributes(&both, None).intensity(), Intensity::Bold);
        assert_eq!(
            cell_attributes(&cell("x"), None).intensity(),
            Intensity::Normal
        );
    }

    /// All six SGR 4:x styles survive. A backend that folded them to one "underlined" bit would
    /// pass a test that only checked `Single`, so every variant is named.
    #[test]
    fn every_underline_style_survives() {
        for (pin, tw) in [
            (PinUnderlineStyle::None, Underline::None),
            (PinUnderlineStyle::Single, Underline::Single),
            (PinUnderlineStyle::Double, Underline::Double),
            (PinUnderlineStyle::Curly, Underline::Curly),
            (PinUnderlineStyle::Dotted, Underline::Dotted),
            (PinUnderlineStyle::Dashed, Underline::Dashed),
        ] {
            assert_eq!(underline(pin), tw, "{pin:?} must map to {tw:?}");
        }
    }

    /// A wide cluster occupies two COLUMNS and its trailer prints nothing — so the cell after the
    /// pair lands where the producer said it would. This is the alignment invariant the whole
    /// width cross-check exists to protect.
    ///
    /// Asserted through `screen_cells` rather than `screen_chars_to_string`, and the distinction
    /// is not pedantry: the string form emits ONE char for a wide cluster, so `"한!"` and a
    /// correctly-placed `!` look identical to a `!` wrongly written at column 1. Only the cell
    /// grid distinguishes them, which is the same reason `sprag-grid` asserts cells.
    #[test]
    fn a_wide_cluster_occupies_two_columns_and_its_trailer_prints_nothing() {
        let wide = TermCell::new("한", TermColor::Default, TermColor::Default).wide();
        let grid = row(6, vec![wide.clone(), wide.trailer(), cell("!")]);
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].str(), "한");
        assert_eq!(
            cells[0][2].str(),
            "!",
            "column 2, not the trailer's column 1"
        );
    }

    /// Two wide clusters in a ROW keep their columns. The single-wide case cannot catch a
    /// double-counted span — the error only displaces from the SECOND cluster on — and the
    /// emulator-driven battery caught exactly that while this module's single-cluster test passed.
    /// Pinned here so the cheap suite is not blind to it either.
    #[test]
    fn consecutive_wide_clusters_keep_their_columns() {
        let wide = |cluster: &'static str| {
            TermCell::new(cluster, TermColor::Default, TermColor::Default).wide()
        };
        let (first, second) = (wide("가"), wide("나"));
        let grid = row(
            8,
            vec![
                first.clone(),
                first.trailer(),
                second.clone(),
                second.trailer(),
                cell("|"),
            ],
        );
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].str(), "가");
        assert_eq!(cells[0][2].str(), "나", "the second head, not column 1");
        assert_eq!(cells[0][4].str(), "|");
    }

    /// **THE WIDTH-DISAGREEMENT GUARD, proven.** A cell declaring itself `Narrow` while carrying a
    /// cluster termwiz measures as two columns IS the disagreement the cross-check exists for,
    /// expressed directly — the same shape a divergence between `unicode-width` and
    /// `widechar_width` would take, without having to find a character the two currently disagree
    /// on (the emulator-driven battery found none, including ZWJ families and ambiguous-width
    /// characters).
    ///
    /// REVERT-PROOF, measured: delete the `unicode_column_width` check in [`grid_changes`] and
    /// this fails — `c` lands at column 2 instead of column 1, and in a full row every remaining
    /// cell would follow it. That is the failure the guard buys, and it is a whole line of garbage
    /// from one character.
    #[test]
    fn a_cluster_wider_than_its_declared_width_does_not_displace_the_rest_of_the_row() {
        // Declared Narrow (the default), cluster two columns wide.
        let grid = row(8, vec![cell("한"), cell("c"), cell("d")]);
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(
            cells[0][1].str(),
            "c",
            "re-anchored, not pushed to column 2"
        );
        assert_eq!(cells[0][2].str(), "d");
    }

    /// A trailer with no head before it is a malformed buffer — nothing the projection makes, but
    /// constructible, and skipping it would shift the rest of the row left. It is rendered as the
    /// blank column it occupies.
    ///
    /// **The orphan must sit INSIDE a run, not at the head of one.** The first version of this
    /// test put it at column 0 and passed with the guard removed, because a skipped FIRST cell
    /// leaves the run empty and the next cell restarts it at its own absolute column — masking the
    /// displacement completely. A cell of the same attributes before it is what makes the run
    /// continue across the gap, which is the only arrangement in which the bug exists.
    ///
    /// REVERT-PROOF, measured: make the orphan arm of [`printed`] return `None` and this fails
    /// with `b` at column 1.
    #[test]
    fn an_orphan_trailer_holds_its_column_instead_of_vanishing() {
        let orphan = TermCell::new("", TermColor::Default, TermColor::Default).trailer();
        let grid = row(6, vec![cell("a"), orphan, cell("b")]);
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(cells[0][1].str(), " ", "the orphan's own column, blanked");
        assert_eq!(cells[0][2].str(), "b", "and nothing after it moved");
    }

    /// A run ends when the attributes change, so two differently-styled halves of a row keep their
    /// own styles instead of the first winning.
    #[test]
    fn attribute_changes_split_a_row_into_runs() {
        let plain = cell("a");
        let bold = cell("b").with_attrs(CellAttrs::empty().with_bold(true));
        let mut surface = painted(&row(4, vec![plain, bold]));
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].attrs().intensity(), Intensity::Normal);
        assert_eq!(cells[0][1].attrs().intensity(), Intensity::Bold);
    }

    /// An empty cluster still occupies its column. Without the blank substitution the rest of the
    /// row would silently shift left, which is the failure mode hardest to spot in a screenshot
    /// and trivial to spot here.
    #[test]
    fn an_empty_cluster_still_occupies_its_column() {
        let grid = row(4, vec![cell("a"), cell(""), cell("c")]);
        let screen = painted(&grid).screen_chars_to_string();
        assert_eq!(screen.chars().nth(2), Some('c'));
    }

    /// The cursor's shape and blink mode fold onto termwiz's single enum without loss, and the
    /// position is the pane's own.
    #[test]
    fn the_cursor_shape_and_blink_fold_onto_one_enum() {
        for (shape, blink, expected) in [
            (PinCursorShape::Block, false, CursorShape::SteadyBlock),
            (PinCursorShape::Block, true, CursorShape::BlinkingBlock),
            (PinCursorShape::Bar, false, CursorShape::SteadyBar),
            (PinCursorShape::Bar, true, CursorShape::BlinkingBar),
            (
                PinCursorShape::Underline,
                false,
                CursorShape::SteadyUnderline,
            ),
            (
                PinCursorShape::Underline,
                true,
                CursorShape::BlinkingUnderline,
            ),
        ] {
            let cursor = GridCursor::new(1, 0, shape, true).with_blink(blink);
            let grid = GridBuffer::new(4, 1).with_cursor(cursor);
            let surface = painted(&grid);
            assert_eq!(surface.cursor_shape(), Some(expected));
            assert_eq!(surface.cursor_position(), (1, 0));
            assert_eq!(surface.cursor_visibility(), CursorVisibility::Visible);
        }
    }

    /// A cursor the producer has placed outside the buffer — which pinion's docs say happens
    /// during an in-flight resize — is reported HIDDEN rather than clamped, so no authoritative
    /// block is drawn in a cell nobody named.
    #[test]
    fn an_out_of_bounds_cursor_is_hidden_not_clamped() {
        let cursor = GridCursor::new(9, 0, PinCursorShape::Block, true);
        let surface = painted(&GridBuffer::new(4, 1).with_cursor(cursor));
        assert_eq!(surface.cursor_visibility(), CursorVisibility::Hidden);
    }

    /// A zero-sized buffer paints nothing at all — not even a cursor change, which would be a
    /// claim about a screen that does not exist.
    #[test]
    fn an_empty_buffer_paints_nothing() {
        assert!(grid_changes(&GridBuffer::new(0, 0)).is_empty());
    }
}
